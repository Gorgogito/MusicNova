//! Estado y procesamiento del hilo de audio.
//!
//! Nada de lo que hay aquí bloquea ni reserva memoria durante `render`: los
//! buffers se dimensionan al crear el estado y los objetos retirados vuelven al
//! hilo de control por la cola de basura.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use cpal::{FromSample, SizedSample};
use mn_stretch::Stretch;

use crate::click::Clicks;
use crate::{BeatGrid, Command, EngineTrack, Garbage, Shared};

/// Frames de salida que se procesan de una vez.
pub const BLOCK: usize = 512;
/// Tempo máximo admitido (2×).
pub const MAX_TEMPO: f64 = 2.0;
const MIN_TEMPO: f64 = 0.25;
/// Golpes de metrónomo pendientes (incluye la cuenta previa).
const MAX_PENDING_CLICKS: usize = 32;

#[derive(Clone, Copy)]
struct PendingClick {
    /// Momento absoluto en frames de salida.
    at: u64,
    accent: bool,
}

pub struct RtState {
    pub tracks: Vec<Box<EngineTrack>>,
    pub position: u64,
    pub playing: bool,
    pub master: f32,
    pub commands: rtrb::Consumer<Command>,
    pub garbage: rtrb::Producer<Garbage>,
    pub shared: Arc<Shared>,

    // Tempo y tono.
    stretch: Stretch,
    tempo: f64,
    semitones: f32,
    stretching: bool,
    in_accum: f64,

    // Bucle A-B.
    loop_on: bool,
    loop_start: u64,
    loop_end: u64,
    loop_count: u64,

    // Metrónomo y cuenta previa.
    clicks: Clicks,
    beats: Option<Box<BeatGrid>>,
    next_beat: usize,
    metronome_on: bool,
    metronome_gain: f32,
    pending: [PendingClick; MAX_PENDING_CLICKS],
    pending_len: usize,
    voice: Option<(bool, usize)>,
    count_in_until: Option<u64>,
    output_clock: u64,
    /// Golpes detectados en el bloque actual: (frame de entrada, acento).
    block_beats: [(usize, bool); 16],
    block_beats_len: usize,

    scratch_in: Vec<f32>,
    scratch_out: Vec<f32>,
}

impl RtState {
    pub fn new(
        sample_rate: u32,
        commands: rtrb::Consumer<Command>,
        garbage: rtrb::Producer<Garbage>,
        shared: Arc<Shared>,
        max_tracks: usize,
    ) -> Self {
        Self {
            tracks: Vec::with_capacity(max_tracks),
            position: 0,
            playing: false,
            master: 1.0,
            commands,
            garbage,
            shared,
            stretch: Stretch::new(sample_rate),
            tempo: 1.0,
            semitones: 0.0,
            stretching: false,
            in_accum: 0.0,
            loop_on: false,
            loop_start: 0,
            loop_end: 0,
            loop_count: 0,
            clicks: Clicks::new(sample_rate),
            beats: None,
            next_beat: 0,
            metronome_on: false,
            metronome_gain: 0.8,
            pending: [PendingClick { at: 0, accent: false }; MAX_PENDING_CLICKS],
            pending_len: 0,
            voice: None,
            count_in_until: None,
            output_clock: 0,
            block_beats: [(0, false); 16],
            block_beats_len: 0,
            scratch_in: vec![0.0; (BLOCK as f64 * MAX_TEMPO) as usize * 2 + 8],
            scratch_out: vec![0.0; BLOCK * 2],
        }
    }

    fn length(&self) -> u64 {
        self.tracks.iter().map(|t| t.frames).max().unwrap_or(0)
    }

    fn discard(&mut self, g: Garbage) {
        // Si la cola de basura está llena se libera aquí; solo ocurre en casos extremos.
        let _ = self.garbage.push(g);
    }

    /// Retraso del estirador, en frames de salida.
    fn stretch_delay_out(&self) -> u64 {
        if !self.stretching {
            return 0;
        }
        (self.stretch.output_latency() as f64 + self.stretch.input_latency() as f64 / self.tempo) as u64
    }

    fn update_stretching(&mut self) {
        let active = (self.tempo - 1.0).abs() > 1e-6 || self.semitones.abs() > 1e-6;
        if active && !self.stretching {
            self.stretch.reset();
            self.in_accum = 0.0;
        }
        self.stretching = active;
    }

    fn resync_beats(&mut self) {
        self.next_beat = match &self.beats {
            Some(g) => g.frames.partition_point(|&b| b < self.position),
            None => 0,
        };
    }

    fn after_jump(&mut self) {
        if self.stretching {
            self.stretch.reset();
            self.in_accum = 0.0;
        }
        self.resync_beats();
    }

    fn push_click(&mut self, at: u64, accent: bool) {
        if self.pending_len < MAX_PENDING_CLICKS {
            self.pending[self.pending_len] = PendingClick { at, accent };
            self.pending_len += 1;
        }
    }

    pub fn apply(&mut self, cmd: Command) {
        fn find(tracks: &mut [Box<EngineTrack>], id: u64) -> Option<&mut Box<EngineTrack>> {
            tracks.iter_mut().find(|t| t.id == id)
        }
        match cmd {
            Command::AddTrack(t) => {
                if self.tracks.len() < self.tracks.capacity() {
                    self.tracks.push(t);
                } else {
                    self.discard(Garbage::Track(t));
                }
            }
            Command::RemoveTrack(id) => {
                if let Some(i) = self.tracks.iter().position(|t| t.id == id) {
                    let t = self.tracks.swap_remove(i);
                    self.discard(Garbage::Track(t));
                }
            }
            Command::Clear => {
                while let Some(t) = self.tracks.pop() {
                    self.discard(Garbage::Track(t));
                }
                if let Some(g) = self.beats.take() {
                    self.discard(Garbage::Beats(g));
                }
                self.position = 0;
                self.playing = false;
                self.loop_on = false;
                self.count_in_until = None;
                self.pending_len = 0;
            }
            Command::Play => {
                self.playing = true;
                self.count_in_until = None;
            }
            Command::PlayWithCountIn { beats, interval } => {
                let start = self.output_clock;
                self.pending_len = 0;
                for k in 0..beats as u64 {
                    self.push_click(start + (k as f64 * interval) as u64, k == 0);
                }
                self.count_in_until = Some(start + (beats as f64 * interval) as u64);
            }
            Command::Pause => {
                self.playing = false;
                self.count_in_until = None;
                self.pending_len = 0;
            }
            Command::Seek(f) => {
                self.position = f;
                self.after_jump();
            }
            Command::SetGain(id, g) => {
                if let Some(t) = find(&mut self.tracks, id) {
                    t.gain = g;
                    t.update_gains();
                }
            }
            Command::SetPan(id, p) => {
                if let Some(t) = find(&mut self.tracks, id) {
                    t.pan = p.clamp(-1.0, 1.0);
                    t.update_gains();
                }
            }
            Command::SetMute(id, m) => {
                if let Some(t) = find(&mut self.tracks, id) {
                    t.mute = m;
                }
            }
            Command::SetSolo(id, s) => {
                if let Some(t) = find(&mut self.tracks, id) {
                    t.solo = s;
                }
            }
            Command::SetMasterGain(g) => self.master = g,
            Command::SetTempo(t) => {
                self.tempo = t.clamp(MIN_TEMPO, MAX_TEMPO);
                self.update_stretching();
            }
            Command::SetPitch(st) => {
                self.semitones = st.clamp(-24.0, 24.0);
                self.stretch.set_transpose_semitones(self.semitones);
                self.update_stretching();
            }
            Command::SetLoop { enabled, start, end } => {
                self.loop_on = enabled && end > start;
                self.loop_start = start;
                self.loop_end = end;
                if self.loop_on && (self.position < start || self.position >= end) {
                    self.position = start;
                    self.after_jump();
                }
            }
            Command::SetBeats(grid) => {
                let old = std::mem::replace(&mut self.beats, Some(grid));
                if let Some(g) = old {
                    self.discard(Garbage::Beats(g));
                }
                self.resync_beats();
            }
            Command::SetMetronome { enabled, gain } => {
                self.metronome_on = enabled;
                self.metronome_gain = gain;
            }
        }
    }

    /// Mezcla `frames` frames de las pistas a partir de la posición actual,
    /// respetando el bucle, y anota los pulsos que se cruzan.
    fn mix_source(&mut self, frames: usize, into_scratch_in: bool) {
        let any_solo = self.tracks.iter().any(|t| t.solo);
        let length = self.length();
        self.block_beats_len = 0;
        for f in 0..frames {
            if self.loop_on && self.position >= self.loop_end {
                self.position = self.loop_start;
                self.loop_count += 1;
                self.resync_beats();
            }
            let p = self.position;
            if let Some(g) = &self.beats {
                if self.next_beat < g.frames.len() && g.frames[self.next_beat] <= p {
                    if self.block_beats_len < self.block_beats.len() {
                        let accent = g.is_downbeat(self.next_beat);
                        self.block_beats[self.block_beats_len] = (f, accent);
                        self.block_beats_len += 1;
                    }
                    self.next_beat += 1;
                }
            }
            let (mut l, mut r) = (0.0f32, 0.0f32);
            if self.playing && p < length {
                for t in &self.tracks {
                    if t.mute || (any_solo && !t.solo) || p >= t.frames {
                        continue;
                    }
                    let i = (p * 2) as usize;
                    l += t.data[i] * t.gain_l;
                    r += t.data[i + 1] * t.gain_r;
                }
                self.position += 1;
            }
            let buf = if into_scratch_in { &mut self.scratch_in } else { &mut self.scratch_out };
            buf[f * 2] = l;
            buf[f * 2 + 1] = r;
        }
        if !self.loop_on && self.position >= length {
            self.position = length;
            self.playing = false;
        }
    }

    /// Genera `n` frames estéreo de música en `scratch_out` (n <= BLOCK).
    fn render_music(&mut self, n: usize) {
        if !self.playing {
            self.scratch_out[..n * 2].fill(0.0);
            self.block_beats_len = 0;
            return;
        }
        let delay = self.stretch_delay_out();
        if !self.stretching {
            self.mix_source(n, false);
            for k in 0..self.block_beats_len {
                let (f, accent) = self.block_beats[k];
                if self.metronome_on {
                    self.push_click(self.output_clock + f as u64, accent);
                }
            }
        } else {
            self.in_accum += n as f64 * self.tempo;
            let input = self.in_accum.floor() as usize;
            self.in_accum -= input as f64;
            self.mix_source(input, true);
            let (inp, out) = (&self.scratch_in[..input * 2], &mut self.scratch_out[..n * 2]);
            self.stretch.process(inp, out);
            for k in 0..self.block_beats_len {
                let (f, accent) = self.block_beats[k];
                if self.metronome_on {
                    let offset = if input > 0 { (f as f64 / input as f64 * n as f64) as u64 } else { 0 };
                    // Los golpes se retrasan lo mismo que el audio estirado.
                    self.push_click(self.output_clock + offset + delay, accent);
                }
            }
        }
    }

    /// Suma los golpes de metrónomo que caen en este bloque.
    fn render_clicks(&mut self, n: usize) {
        let block_end = self.output_clock + n as u64;
        for f in 0..n {
            let now = self.output_clock + f as u64;
            let mut i = 0;
            while i < self.pending_len {
                if self.pending[i].at <= now {
                    self.voice = Some((self.pending[i].accent, 0));
                    self.pending_len -= 1;
                    self.pending[i] = self.pending[self.pending_len];
                } else {
                    i += 1;
                }
            }
            if let Some((accent, pos)) = self.voice {
                let wave = if accent { &self.clicks.accent } else { &self.clicks.normal };
                if pos < wave.len() {
                    let s = wave[pos] * self.metronome_gain.max(0.3);
                    self.scratch_out[f * 2] += s;
                    self.scratch_out[f * 2 + 1] += s;
                    self.voice = Some((accent, pos + 1));
                } else {
                    self.voice = None;
                }
            }
        }
        if let Some(start) = self.count_in_until {
            if start <= block_end {
                self.count_in_until = None;
                self.playing = true;
            }
        }
    }

    /// Procesa un bloque de salida en `scratch_out` y avanza el reloj.
    pub fn process_block(&mut self, n: usize) {
        while let Ok(cmd) = self.commands.pop() {
            self.apply(cmd);
        }
        self.render_music(n);
        self.render_clicks(n);
        for s in &mut self.scratch_out[..n * 2] {
            *s *= self.master;
        }
        self.output_clock += n as u64;
    }

    pub fn render<T: SizedSample + FromSample<f32>>(&mut self, out: &mut [T], channels: usize) {
        let frames = out.len() / channels;
        let silence = T::from_sample(0.0f32);
        let (mut pk_l, mut pk_r) = (0.0f32, 0.0f32);
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(BLOCK);
            self.process_block(n);
            for f in 0..n {
                let (l, r) = (self.scratch_out[f * 2], self.scratch_out[f * 2 + 1]);
                pk_l = pk_l.max(l.abs());
                pk_r = pk_r.max(r.abs());
                let frame = &mut out[(done + f) * channels..(done + f + 1) * channels];
                if channels == 1 {
                    frame[0] = T::from_sample(0.5 * (l + r));
                } else {
                    frame[0] = T::from_sample(l);
                    frame[1] = T::from_sample(r);
                    for s in &mut frame[2..] {
                        *s = silence;
                    }
                }
            }
            done += n;
        }

        let sh = &self.shared;
        // Posición audible: el estirador retrasa la salida respecto a la lectura.
        let lag = (self.stretch_delay_out() as f64 * self.tempo) as u64;
        let mut heard = self.position.saturating_sub(lag);
        if self.loop_on && heard < self.loop_start {
            heard = self.loop_start;
        }
        sh.position.store(heard, Ordering::Relaxed);
        sh.length.store(self.length(), Ordering::Relaxed);
        sh.playing.store(self.playing, Ordering::Relaxed);
        sh.counting_in.store(self.count_in_until.is_some(), Ordering::Relaxed);
        sh.loop_count.store(self.loop_count, Ordering::Relaxed);
        // Retiene el pico máximo hasta que el control lo lea.
        sh.peak_l.fetch_max(pk_l.to_bits(), Ordering::Relaxed);
        sh.peak_r.fetch_max(pk_r.to_bits(), Ordering::Relaxed);
    }
}
