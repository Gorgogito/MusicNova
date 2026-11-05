//! Estado y procesamiento del hilo de audio.
//!
//! Nada de lo que hay aquí bloquea ni reserva memoria durante `render`: los
//! buffers se dimensionan al crear el estado y los objetos retirados vuelven al
//! hilo de control por la cola de basura.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use cpal::{FromSample, SizedSample};
use mn_fx::loudness::LoudnessMeter;
use mn_fx::{Chain, Instrument, LiveControl, LiveNote};
use mn_stretch::Stretch;

use crate::click::Clicks;
use crate::input::Monitor;
use crate::live::{MidiEvent, MidiMessage, RecEvent};
use crate::{BeatGrid, Command, EngineTrack, Garbage, Shared, TrackId};

/// Notas del instrumento en vivo por bloque como máximo.
const MAX_LIVE_NOTES: usize = 256;

/// Frames de salida que se procesan de una vez.
pub const BLOCK: usize = 512;
/// Tempo máximo admitido (2×).
pub const MAX_TEMPO: f64 = 2.0;
const MIN_TEMPO: f64 = 0.25;
/// Golpes de metrónomo pendientes (incluye la cuenta previa).
const MAX_PENDING_CLICKS: usize = 32;
/// Frames de entrada máximos por bloque (con el tempo al máximo).
pub(crate) const MAX_IN_FRAMES: usize = (BLOCK as f64 * MAX_TEMPO) as usize + 4;

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
    /// Latencia compensada entre pistas (frames): la de la pista con más latencia.
    pdc_tracks: usize,
    master_automation: Option<Box<mn_core::automation::ChannelAutomation>>,
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

    // Instrumento en vivo.
    midi_rx: rtrb::Consumer<MidiEvent>,
    rec_tx: rtrb::Producer<RecEvent>,
    record_midi: bool,
    live: Option<Box<dyn Instrument>>,
    live_track: TrackId,
    live_buf: Vec<f32>,
    live_notes: Vec<LiveNote>,
    sustain: bool,
    sustained: [bool; 128],
    /// Este bloque el sonido en vivo entra en su pista antes de los efectos.
    live_inject: bool,
    /// La pista ya recibió el sonido en vivo en este bloque.
    live_mixed: bool,
    /// (reloj de salida, instante en que suena en ns) del último callback.
    anchor: Option<(u64, u64)>,
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
            pdc_tracks: 0,
            master_automation: None,
            meter: LoudnessMeter::new(sample_rate),
            free_run: false,
            announce_start: false,
            monitor: None,
            play_offset_ns: None,
            sample_rate,
            midi_rx: rtrb::RingBuffer::new(1).1,
            rec_tx: rtrb::RingBuffer::new(1).0,
            record_midi: false,
            live: None,
            live_track: 0,
            live_buf: vec![0.0; BLOCK * 2],
            live_notes: Vec::with_capacity(MAX_LIVE_NOTES),
            sustain: false,
            sustained: [false; 128],
            live_inject: false,
            live_mixed: false,
            anchor: None,
        }
    }

    /// Conecta la cola de eventos MIDI y la de notas grabadas.
    pub(crate) fn attach_midi(&mut self, midi: rtrb::Consumer<MidiEvent>, rec: rtrb::Producer<RecEvent>) {
        self.midi_rx = midi;
        self.rec_tx = rec;
    }

    /// Posición de la canción que se oía en el instante `time_ns` (0 = ahora):
    /// la del bloque actual menos lo que tarda en sonar (dispositivo, efectos
    /// con latencia y estirador).
    fn heard_frame(&self, time_ns: u64) -> u64 {
        let mut lag = self.output_latency() as f64;
        if self.stretching {
            lag += self.stretch_delay_out() as f64;
        }
        if let (true, Some((clock, ns))) = (time_ns > 0, self.anchor) {
            let rate = self.sample_rate as f64;
            let plays_at = ns as f64 + (self.output_clock as f64 - clock as f64) * 1e9 / rate;
            lag += ((plays_at - time_ns as f64) * rate / 1e9).max(0.0);
        }
        let tempo = if self.stretching { self.tempo } else { 1.0 };
        (self.position as f64 - lag * tempo).max(0.0) as u64
    }

    fn live_note(&mut self, key: u8, velocity: u8, on: bool, time_ns: u64) {
        let key = key.min(127);
        if self.live_notes.len() < self.live_notes.capacity() {
            self.live_notes.push(LiveNote { time: 0, key, velocity: if on { velocity.max(1) } else { 0 }, on });
        }
        if self.record_midi && self.playing {
            let frame = self.heard_frame(time_ns);
            let _ = self.rec_tx.push(RecEvent { frame, key, velocity, on });
        }
    }

    /// Lee los eventos MIDI pendientes: pedal de sostenido y grabación incluidos.
    fn collect_midi(&mut self) {
        self.live_notes.clear();
        while let Ok(ev) = self.midi_rx.pop() {
            match ev.msg {
                MidiMessage::NoteOn { key, velocity } => {
                    let k = key.min(127) as usize;
                    if self.sustained[k] {
                        // Se vuelve a pulsar una nota sostenida: la anterior termina aquí.
                        self.sustained[k] = false;
                        self.live_note(key, 0, false, ev.time_ns);
                    }
                    self.live_note(key, velocity, true, ev.time_ns);
                }
                MidiMessage::NoteOff { key } => {
                    if self.sustain {
                        self.sustained[key.min(127) as usize] = true;
                    } else {
                        self.live_note(key, 0, false, ev.time_ns);
                    }
                }
                MidiMessage::Sustain(down) => {
                    self.sustain = down;
                    if !down {
                        for k in 0..128u8 {
                            if std::mem::take(&mut self.sustained[k as usize]) {
                                self.live_note(k, 0, false, ev.time_ns);
                            }
                        }
                    }
                }
                MidiMessage::AllNotesOff => {
                    self.sustain = false;
                    for k in 0..128 {
                        if std::mem::take(&mut self.sustained[k]) {
                            self.live_note(k as u8, 0, false, ev.time_ns);
                        }
                    }
                    if let Some(l) = self.live.as_deref_mut() {
                        l.all_notes_off();
                    }
                }
                MidiMessage::PitchBend(v) => self.live_control(LiveControl::PitchBend(pitch_bend_amount(v))),
                MidiMessage::Control { cc, value } => self.live_control(LiveControl::Control { cc: cc.min(127), value: value.min(127) }),
                MidiMessage::Pressure(v) => self.live_control(LiveControl::Pressure(v.min(127))),
            }
        }
    }

    /// Gestos continuos: van directos al instrumento armado (no se graban).
    fn live_control(&mut self, c: LiveControl) {
        if let Some(l) = self.live.as_deref_mut() {
            l.control(c);
        }
    }

    /// Sonido en vivo que no entró en su pista durante la mezcla (transporte
    /// parado o tempo cambiado): va por su volumen y panorama y, parado, también
    /// por sus efectos.
    fn mix_live_fallback(&mut self, n: usize, playing: bool) {
        if self.live.is_none() || self.live_mixed {
            return;
        }
        let any_solo = self.tracks.iter().any(|t| t.solo);
        let Some(t) = self.tracks.iter_mut().find(|t| t.id == self.live_track) else { return };
        if t.mute || (any_solo && !t.solo) {
            return;
        }
        let buf = &mut self.live_buf[..n * 2];
        if !playing {
            if let Some(chain) = t.chain.as_deref_mut() {
                chain.process_with_key(buf, None);
            }
        }
        mn_core::mix::mix_ramp(&mut self.scratch_out[..n * 2], buf, (t.gain_l, t.gain_r), (t.gain_l, t.gain_r));
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
            Command::SetSidechain(id, source) => {
                if let Some(t) = find(&mut self.tracks, id) {
                    t.sidechain = source.filter(|s| *s != id);
                }
            }
            Command::SetLive(live) => {
                if let Some(mut old) = self.live.take() {
                    old.all_notes_off();
                    self.discard(Garbage::Instrument(old));
                }
                self.sustain = false;
                self.sustained = [false; 128];
                if let Some((id, inst)) = live {
                    self.live_track = id;
                    self.live = Some(inst);
                }
            }
            Command::SetMidiRecord(on) => self.record_midi = on,
            Command::SetChain(target, chain) => {
                // Cadena nueva (quizá con plugins recién cargados): la automatización
                // vuelve a enviarles sus valores.
                let old = match target {
                    None => {
                        if let Some(a) = self.master_automation.as_deref() {
                            a.invalidate();
                        }
                        self.master_chain.replace(chain)
                    }
                    Some(id) => match find(&mut self.tracks, id) {
                        Some(t) => {
                            if let Some(a) = t.automation.as_deref() {
                                a.invalidate();
                            }
                            t.chain.replace(chain)
                        }
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
            Command::SetData(id, data, clips) => {
                if let Some(t) = find(&mut self.tracks, id) {
                    let old_data = std::mem::replace(&mut t.data, data);
                    let old_clips = std::mem::replace(&mut t.clips, *clips);
                    t.end = mn_core::mix::clips_end(&t.clips);
                    self.discard(Garbage::Data(old_data));
                    self.discard(Garbage::Clips(old_clips));
                }
            }
            Command::SetClips(id, clips) => {
                if let Some(t) = find(&mut self.tracks, id) {
                    let old = std::mem::replace(&mut t.clips, *clips);
                    t.end = mn_core::mix::clips_end(&t.clips);
                    self.discard(Garbage::Clips(old));
                }
            }
            Command::SetAutomation(target, automation) => {
                let old = match target {
                    None => std::mem::replace(&mut self.master_automation, automation),
                    Some(id) => match find(&mut self.tracks, id) {
                        Some(t) => std::mem::replace(&mut t.automation, automation),
                        None => automation,
                    },
                };
                if let Some(a) = old {
                    self.discard(Garbage::Automation(a));
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

    /// Retardo total de la música por los efectos: la pista con más latencia
    /// más el bus máster (frames).
    pub(crate) fn output_latency(&self) -> usize {
        self.pdc_tracks + self.master_chain.as_deref().map_or(0, Chain::latency)
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
        // Posiciones de la línea de tiempo al principio y al final del bloque (automatización).
        let src = &self.src_pos[..frames];
        let span = src.iter().position(|&p| p != u64::MAX).map(|i| {
            let last = src.iter().rposition(|&p| p != u64::MAX).unwrap_or(i);
            (src[i], src[last] + 1)
        });
        // Compensación de latencia: cada pista se retrasa lo que le falta para
        // llegar a la de la pista con más latencia (también las silenciadas, para
        // que la alineación no cambie al silenciar).
        let max_lat = self.tracks.iter().map(|t| t.latency()).max().unwrap_or(0);
        // Sidechain: audio de la pista fuente (sin efectos ni volumen) en las mismas posiciones.
        for i in 0..self.tracks.len() {
            let Some(source) = self.tracks[i].sidechain else { continue };
            let Some(j) = self.tracks.iter().position(|t| t.id == source) else {
                self.tracks[i].key_valid = false;
                continue;
            };
            let mut key = std::mem::take(&mut self.tracks[i].key);
            key[..frames * 2].fill(0.0);
            mn_core::mix::render_clips(&self.tracks[j].data, &self.tracks[j].clips, src, &mut key[..frames * 2]);
            self.tracks[i].key = key;
            self.tracks[i].key_valid = true;
        }
        self.pdc_tracks = max_lat;
        // Instrumento en vivo: entra en su pista antes de los efectos (mismo tamaño de bloque).
        let live = (self.live_inject && self.live.is_some() && frames * 2 <= self.live_buf.len())
            .then(|| (self.live_track, &self.live_buf[..frames * 2]));
        for t in &mut self.tracks {
            if live.is_some_and(|(id, _)| id == t.id) {
                self.live_mixed = true;
            }
            let comp = max_lat - t.latency();
            if t.mute || (any_solo && !t.solo) {
                if comp > 0 {
                    t.delay_silence(frames);
                }
                continue;
            }
            track_buf.fill(0.0);
            mn_core::mix::render_clips(&t.data, &t.clips, src, track_buf);
            if let Some((_, l)) = live.filter(|(id, _)| *id == t.id) {
                for (d, s) in track_buf.iter_mut().zip(l) {
                    *d += *s;
                }
            }
            let auto = t.automation.as_deref().zip(span);
            if let Some(chain) = t.chain.as_deref_mut() {
                if let Some((a, (p0, _))) = auto {
                    a.apply_effects(chain, p0);
                }
                let key = t.key_valid.then(|| &t.key[..frames * 2]);
                chain.process_with_key(track_buf, key);
            }
            crate::delay_line(&mut t.pdc, &mut t.pdc_pos, track_buf, comp);
            let out = &mut out[..frames * 2];
            match auto.filter(|(a, _)| a.has_gain()) {
                Some((a, (p0, p1))) => {
                    mn_core::mix::mix_ramp(out, track_buf, a.gains(p0, t.gain, t.pan), a.gains(p1, t.gain, t.pan))
                }
                None => mn_core::mix::mix_ramp(out, track_buf, (t.gain_l, t.gain_r), (t.gain_l, t.gain_r)),
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
            // Lo que se lee ahora suena `output_latency` frames después (efectos con
            // latencia): la grabación alinea las tomas con ese instante.
            self.shared.play_start.store(self.output_clock + self.output_latency() as u64, self.position);
        }
        let delay = self.stretch_delay_out();
        if !self.stretching {
            self.live_inject = true;
            self.mix_source(n, false);
            self.live_inject = false;
            for k in 0..self.block_beats_len {
                let (f, accent) = self.block_beats[k];
                if self.metronome_on {
                    self.push_click(self.output_clock + f as u64 + self.output_latency() as u64, ClickKind::beat(accent));
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
                    self.push_click(self.output_clock + offset + delay + self.output_latency() as u64, ClickKind::beat(accent));
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
        let block_start = self.position;
        // Instrumento en vivo: notas que llegaron y su sonido de este bloque.
        self.collect_midi();
        self.live_mixed = false;
        if let Some(l) = self.live.as_deref_mut() {
            l.render(&self.live_notes, &mut self.live_buf[..n * 2]);
        }
        // El transporte puede pararse a mitad de bloque (final de la canción): el
        // máster procesa igualmente el bloque entero para no cortar su último tramo.
        let playing = self.playing;
        self.render_music(n);
        self.mix_live_fallback(n, playing);
        // Con un instrumento armado el máster sigue procesando aunque esté parado.
        let master_on = playing || self.live.is_some();
        // Bus máster: efectos, ganancia y medición (el metrónomo va después y no se mide).
        let auto = self.master_automation.as_deref().filter(|_| playing);
        if let Some(chain) = self.master_chain.as_deref_mut() {
            if master_on {
                if let Some(a) = auto {
                    a.apply_effects(chain, block_start);
                }
                chain.process(&mut self.scratch_out[..n * 2]);
            }
        }
        match auto {
            Some(a) => {
                // Igual que la mezcla offline: del inicio al final del bloque leído.
                let (g0, g1) = (a.master_gain(block_start, self.master), a.master_gain(self.position, self.master));
                mn_core::mix::ramp_in_place(&mut self.scratch_out[..n * 2], g0, g1);
            }
            None => {
                for s in &mut self.scratch_out[..n * 2] {
                    *s *= self.master;
                }
            }
        }
        if playing {
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
            self.anchor = Some((self.output_clock, at));
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

/// Rueda de tono MIDI (0–16383, 8192 = centro) → -1…1, con los extremos exactos.
pub(crate) fn pitch_bend_amount(v: u16) -> f32 {
    let d = v.min(16383) as f32 - 8192.0;
    if d >= 0.0 { d / 8191.0 } else { d / 8192.0 }
}
