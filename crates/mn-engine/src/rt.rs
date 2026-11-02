//! Estado y procesamiento del hilo de audio.
//!
//! Nada de lo que hay aquí bloquea ni reserva memoria durante `render`: los
//! buffers se dimensionan al crear el estado y los objetos retirados vuelven al
//! hilo de control por la cola de basura.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use cpal::{FromSample, SizedSample};
use mn_fx::loudness::LoudnessMeter;
use mn_fx::Chain;
use mn_stretch::Stretch;

use crate::click::Clicks;
use crate::input::Monitor;
use crate::{BeatGrid, Command, EngineTrack, Garbage, Shared};

/// Frames de salida que se procesan de una vez.
pub const BLOCK: usize = 512;
/// Tempo máximo admitido (2×).
pub const MAX_TEMPO: f64 = 2.0;
const MIN_TEMPO: f64 = 0.25;
/// Golpes de metrónomo pendientes (incluye la cuenta previa).
const MAX_PENDING_CLICKS: usize = 32;
/// Frames de entrada máximos por bloque (con el tempo al máximo).
const MAX_IN_FRAMES: usize = (BLOCK as f64 * MAX_TEMPO) as usize + 4;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClickKind {
    Normal,
    Accent,
    /// Barrido de calibración de latencia.
    Chirp,
}

impl ClickKind {
    fn beat(accent: bool) -> Self {
        if accent {
            Self::Accent
        } else {
            Self::Normal
        }
    }
}

#[derive(Clone, Copy)]
struct PendingClick {
    /// Momento absoluto en frames de salida.
    at: u64,
    kind: ClickKind,
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
    voice: Option<(ClickKind, usize)>,
    count_in_until: Option<u64>,
    output_clock: u64,
    /// Golpes detectados en el bloque actual: (frame de entrada, acento).
    block_beats: [(usize, bool); 16],
    block_beats_len: usize,

    scratch_in: Vec<f32>,
    scratch_out: Vec<f32>,
    /// Posición de origen de cada frame del bloque (`u64::MAX` = silencio).
    src_pos: Vec<u64>,
    /// Audio de una pista para procesar su cadena de efectos.
    track_buf: Vec<f32>,
    master_chain: Option<Box<Chain>>,
    meter: LoudnessMeter,

    // Grabación.
    /// El transporte no se detiene al final de las pistas.
    free_run: bool,
    /// Publicar el próximo arranque del transporte (frame de salida y posición).
    announce_start: bool,
    pub(crate) monitor: Option<Box<Monitor>>,
    /// Desfase filtrado (ns) entre el reloj de salida y el instante en que suena.
    play_offset_ns: Option<i128>,
    sample_rate: u32,
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
            pending: [PendingClick { at: 0, kind: ClickKind::Normal }; MAX_PENDING_CLICKS],
            pending_len: 0,
            voice: None,
            count_in_until: None,
            output_clock: 0,
            block_beats: [(0, false); 16],
            block_beats_len: 0,
            scratch_in: vec![0.0; MAX_IN_FRAMES * 2],
            scratch_out: vec![0.0; BLOCK * 2],
            src_pos: vec![u64::MAX; MAX_IN_FRAMES],
            track_buf: vec![0.0; MAX_IN_FRAMES * 2],
            master_chain: None,
            meter: LoudnessMeter::new(sample_rate),
            free_run: false,
            announce_start: false,
            monitor: None,
            play_offset_ns: None,
            sample_rate,
        }
    }

    fn length(&self) -> u64 {
        self.tracks.iter().map(|t| t.end()).max().unwrap_or(0)
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

    fn push_click(&mut self, at: u64, kind: ClickKind) {
        if self.pending_len < MAX_PENDING_CLICKS {
            self.pending[self.pending_len] = PendingClick { at, kind };
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
                self.meter.reset();
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
                self.announce_start = true;
                self.count_in_until = None;
            }
            Command::PlayWithCountIn { beats, interval } => {
                let start = self.output_clock;
                self.pending_len = 0;
                for k in 0..beats as u64 {
                    self.push_click(start + (k as f64 * interval) as u64, ClickKind::beat(k == 0));
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
            Command::SetChain(target, chain) => {
                let old = match target {
                    None => self.master_chain.replace(chain),
                    Some(id) => match find(&mut self.tracks, id) {
                        Some(t) => t.chain.replace(chain),
                        None => Some(chain),
                    },
                };
                if let Some(c) = old {
                    self.discard(Garbage::Chain(c));
                }
            }
            Command::SetEffectParams(target, index, params) => {
                if let Some(c) = self.chain_mut(target) {
                    c.set_params(index, &params);
                }
            }
            Command::SetEffectEnabled(target, index, enabled) => {
                if let Some(c) = self.chain_mut(target) {
                    c.set_enabled(index, enabled);
                }
            }
            Command::SetFreeRun(on) => self.free_run = on,
            Command::AttachMonitor(m) => {
                if let Some(old) = self.monitor.replace(m) {
                    self.discard(Garbage::Monitor(old));
                }
            }
            Command::DetachMonitor => {
                if let Some(old) = self.monitor.take() {
                    self.discard(Garbage::Monitor(old));
                }
            }
            Command::SetMonitor { enabled, gain } => {
                if let Some(m) = self.monitor.as_deref_mut() {
                    m.enabled = enabled;
                    m.gain = gain;
                }
            }
            Command::CalibrationClicks { count, interval, lead } => {
                let first = self.output_clock + lead;
                for k in 0..count as u64 {
                    self.push_click(first + k * interval, ClickKind::Chirp);
                }
                self.shared.calib.store(first, interval);
            }
        }
    }

    fn chain_mut(&mut self, target: Option<u64>) -> Option<&mut Chain> {
        match target {
            None => self.master_chain.as_deref_mut(),
            Some(id) => self.tracks.iter_mut().find(|t| t.id == id).and_then(|t| t.chain.as_deref_mut()),
        }
    }

    /// Mezcla `frames` frames de las pistas a partir de la posición actual,
    /// respetando el bucle, y anota los pulsos que se cruzan. Cada pista pasa
    /// por su cadena de efectos antes del volumen y el panorama.
    fn mix_source(&mut self, frames: usize, into_scratch_in: bool) {
        let frames = frames.min(MAX_IN_FRAMES);
        let any_solo = self.tracks.iter().any(|t| t.solo);
        let length = self.length();
        self.block_beats_len = 0;
        // 1. Posiciones de origen del bloque (bucle y pulsos incluidos).
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
            if self.playing && (p < length || self.free_run) {
                self.src_pos[f] = p;
                self.position += 1;
            } else {
                self.src_pos[f] = u64::MAX;
            }
        }
        // 2. Cada pista: lectura, efectos y suma con volumen y panorama.
        let out = if into_scratch_in { &mut self.scratch_in } else { &mut self.scratch_out };
        out[..frames * 2].fill(0.0);
        let track_buf = &mut self.track_buf[..frames * 2];
        for t in &mut self.tracks {
            if t.mute || (any_solo && !t.solo) {
                continue;
            }
            for (f, &p) in self.src_pos[..frames].iter().enumerate() {
                let (l, r) = if p >= t.offset && p - t.offset < t.frames {
                    let i = ((p - t.offset) * 2) as usize;
                    (t.data[i], t.data[i + 1])
                } else {
                    (0.0, 0.0)
                };
                track_buf[f * 2] = l;
                track_buf[f * 2 + 1] = r;
            }
            if let Some(chain) = t.chain.as_deref_mut() {
                chain.process(track_buf);
            }
            for (o, s) in out[..frames * 2].chunks_exact_mut(2).zip(track_buf.chunks_exact(2)) {
                o[0] += s[0] * t.gain_l;
                o[1] += s[1] * t.gain_r;
            }
        }
        if !self.loop_on && !self.free_run && self.position >= length {
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
        if self.announce_start {
            self.announce_start = false;
            self.shared.play_start.store(self.output_clock, self.position);
        }
        let delay = self.stretch_delay_out();
        if !self.stretching {
            self.mix_source(n, false);
            for k in 0..self.block_beats_len {
                let (f, accent) = self.block_beats[k];
                if self.metronome_on {
                    self.push_click(self.output_clock + f as u64, ClickKind::beat(accent));
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
                    self.push_click(self.output_clock + offset + delay, ClickKind::beat(accent));
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
                    self.voice = Some((self.pending[i].kind, 0));
                    self.pending_len -= 1;
                    self.pending[i] = self.pending[self.pending_len];
                } else {
                    i += 1;
                }
            }
            if let Some((kind, pos)) = self.voice {
                let (wave, gain) = match kind {
                    ClickKind::Accent => (&self.clicks.accent, self.metronome_gain.max(0.3)),
                    ClickKind::Normal => (&self.clicks.normal, self.metronome_gain.max(0.3)),
                    ClickKind::Chirp => (&self.clicks.chirp, 1.0),
                };
                if pos < wave.len() {
                    let s = wave[pos] * gain;
                    self.scratch_out[f * 2] += s;
                    self.scratch_out[f * 2 + 1] += s;
                    self.voice = Some((kind, pos + 1));
                } else {
                    self.voice = None;
                }
            }
        }
        if let Some(start) = self.count_in_until {
            if start <= block_end {
                self.count_in_until = None;
                self.playing = true;
                self.announce_start = true;
            }
        }
    }

    /// Procesa un bloque de salida en `scratch_out` y avanza el reloj.
    pub fn process_block(&mut self, n: usize) {
        while let Ok(cmd) = self.commands.pop() {
            self.apply(cmd);
        }
        self.render_music(n);
        // Bus máster: efectos, ganancia y medición (el metrónomo va después y no se mide).
        if let Some(chain) = self.master_chain.as_deref_mut() {
            if self.playing {
                chain.process(&mut self.scratch_out[..n * 2]);
            }
        }
        for s in &mut self.scratch_out[..n * 2] {
            *s *= self.master;
        }
        if self.playing {
            self.meter.process(&self.scratch_out[..n * 2]);
        }
        self.render_clicks(n);
        // La monitorización va al final: no pasa por el máster ni se mide.
        if let Some(m) = self.monitor.as_deref_mut() {
            m.mix_into(&mut self.scratch_out[..n * 2]);
        }
        self.output_clock += n as u64;
    }

    #[cfg(test)]
    pub fn render<T: SizedSample + FromSample<f32>>(&mut self, out: &mut [T], channels: usize) {
        self.render_at(out, channels, None)
    }

    /// Filtra el instante de reproducción que estima el controlador. Un callback
    /// que llega tarde solo puede sumar error (WASAPI lo estima como «ahora +
    /// hueco libre del buffer»), así que se conserva el desfase mínimo entre el
    /// reloj de salida y el reloj del sistema, dejándolo subir muy despacio
    /// (≈0,1 ms/s) para seguir la deriva entre ambos relojes.
    fn anchor_playback(&mut self, playback_nanos: u64, frames: usize) -> u64 {
        let clock_ns = self.output_clock as i128 * 1_000_000_000 / self.sample_rate as i128;
        let raw = playback_nanos as i128 - clock_ns;
        // 100 µs por segundo de audio.
        let creep = 100_000 * frames as i128 / self.sample_rate as i128;
        let off = match self.play_offset_ns {
            Some(prev) => raw.min(prev + creep),
            None => raw,
        };
        self.play_offset_ns = Some(off);
        (clock_ns + off).max(0) as u64
    }

    /// Rellena el buffer del dispositivo. `playback_nanos` es el instante en que
    /// sonará su primer frame (para alinear las grabaciones).
    pub fn render_at<T: SizedSample + FromSample<f32>>(&mut self, out: &mut [T], channels: usize, playback_nanos: Option<u64>) {
        if let Some(t) = playback_nanos {
            let at = self.anchor_playback(t, out.len() / channels.max(1));
            self.shared.out_anchor.store(self.output_clock, at);
        }
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
        let length = self.length();
        sh.length.store(if self.free_run { length.max(self.position) } else { length }, Ordering::Relaxed);
        sh.playing.store(self.playing, Ordering::Relaxed);
        sh.counting_in.store(self.count_in_until.is_some(), Ordering::Relaxed);
        sh.loop_count.store(self.loop_count, Ordering::Relaxed);
        // Retiene el pico máximo hasta que el control lo lea.
        sh.peak_l.fetch_max(pk_l.to_bits(), Ordering::Relaxed);
        sh.peak_r.fetch_max(pk_r.to_bits(), Ordering::Relaxed);
        sh.lufs_m.store(self.meter.momentary().to_bits(), Ordering::Relaxed);
        sh.lufs_s.store(self.meter.short_term().to_bits(), Ordering::Relaxed);
    }
}
