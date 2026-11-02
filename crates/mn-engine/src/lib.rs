//! Motor de audio en tiempo real de MusicNova.
//!
//! El hilo de audio (callback de cpal) nunca bloquea ni reserva memoria: recibe
//! órdenes por un ring buffer sin bloqueos (`rtrb`) y publica su estado mediante
//! atómicos. Los objetos retirados (pistas, rejillas de pulsos) se devuelven por
//! otro ring buffer para liberarlos fuera del hilo de audio.
//!
//! Funciones: mezcla multipista, tempo y tono en tiempo real (Signalsmith
//! Stretch), bucle A-B, metrónomo sobre una rejilla de pulsos, cuenta previa,
//! efectos por pista y en el bus máster, medición de sonoridad (LUFS) y
//! grabación desde una entrada de audio con monitorización y compensación de
//! latencia.

mod click;
mod input;
mod rt;
mod sync;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use mn_core::automation::ChannelAutomation;
use mn_core::mix::{clips_end, ClipFrames};
use mn_fx::{Chain, EffectParams, EffectSlot};
use input::{InputHandle, Monitor};
use rt::RtState;
use sync::{frame_instant, instant_nanos, Pair};

pub use input::{input_devices, measure_latency, InputChannels, InputDeviceInfo, InputInfo, LatencyReport, Take};

pub type TrackId = u64;

const MAX_TRACKS: usize = 128;
const COMMAND_CAPACITY: usize = 1024;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("no hay dispositivo de salida de audio")]
    NoDevice,
    #[error("dispositivo de audio: {0}")]
    Device(String),
    #[error("la cola de órdenes del motor está llena")]
    QueueFull,
    #[error("el audio debe estar a {expected} Hz (recibido {got} Hz)")]
    SampleRate { expected: u32, got: u32 },
    #[error("{0}")]
    Invalid(String),
}

/// Pista cargada en memoria a la frecuencia del motor.
pub(crate) struct EngineTrack {
    id: TrackId,
    data: Arc<Vec<f32>>,
    /// Tramos del audio colocados en la línea de tiempo.
    clips: Vec<ClipFrames>,
    /// Final del último clip (frames).
    end: u64,
    gain: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    gain_l: f32,
    gain_r: f32,
    chain: Option<Box<Chain>>,
    automation: Option<Box<ChannelAutomation>>,
    /// Línea de retardo para la compensación de latencia (reservada al crear la pista).
    pdc: Vec<[f32; 2]>,
    pdc_pos: usize,
    /// Pista cuya señal alimenta el sidechain de esta cadena.
    sidechain: Option<TrackId>,
    /// Señal de sidechain del bloque actual (reservada al crear la pista).
    key: Vec<f32>,
    key_valid: bool,
}

/// Latencia máxima que se compensa (frames; ≈2,7 s a 48 kHz).
pub(crate) const PDC_MAX: usize = 1 << 17;

/// Retarda `buf` (estéreo entrelazado) `delay` frames con la línea circular `line`.
pub(crate) fn delay_line(line: &mut [[f32; 2]], pos: &mut usize, buf: &mut [f32], delay: usize) {
    let len = line.len();
    if delay == 0 || len == 0 {
        return;
    }
    let delay = delay.min(len - 1);
    for f in buf.as_chunks_mut::<2>().0 {
        line[*pos] = *f;
        *f = line[(*pos + len - delay) % len];
        *pos = (*pos + 1) % len;
    }
}

impl EngineTrack {
    fn end(&self) -> u64 {
        self.end
    }

    /// Latencia de los efectos de la pista (frames).
    pub(crate) fn latency(&self) -> usize {
        self.chain.as_deref().map_or(0, Chain::latency).min(PDC_MAX - 1)
    }


    /// Hace avanzar la línea de retardo con silencio (pista silenciada).
    pub(crate) fn delay_silence(&mut self, frames: usize) {
        let len = self.pdc.len();
        if len == 0 {
            return;
        }
        for _ in 0..frames {
            self.pdc[self.pdc_pos] = [0.0; 2];
            self.pdc_pos = (self.pdc_pos + 1) % len;
        }
    }

    fn update_gains(&mut self) {
        (self.gain_l, self.gain_r) = mn_core::mix::balance_gains(self.gain, self.pan);
    }
}

/// Rejilla de pulsos (en frames del motor) para el metrónomo.
pub(crate) struct BeatGrid {
    frames: Vec<u64>,
    beats_per_bar: u32,
    /// Índice del primer tiempo fuerte dentro de `frames`.
    downbeat_offset: u32,
    /// Tiempos fuertes explícitos (pulsos detectados); si está vacío se usa
    /// `beats_per_bar` y `downbeat_offset`.
    accents: Vec<bool>,
}

impl BeatGrid {
    fn is_downbeat(&self, index: usize) -> bool {
        if !self.accents.is_empty() {
            return self.accents.get(index).copied().unwrap_or(false);
        }
        let bpb = self.beats_per_bar.max(1) as i64;
        (index as i64 - self.downbeat_offset as i64).rem_euclid(bpb) == 0
    }
}

// Los objetos solo viajan para liberarse fuera del hilo de audio; nunca se leen.
#[allow(dead_code)]
pub(crate) enum Garbage {
    Track(Box<EngineTrack>),
    Beats(Box<BeatGrid>),
    Chain(Box<Chain>),
    Monitor(Box<Monitor>),
    Clips(Vec<ClipFrames>),
    Automation(Box<ChannelAutomation>),
    Data(Arc<Vec<f32>>),
}

pub(crate) enum Command {
    AddTrack(Box<EngineTrack>),
    RemoveTrack(TrackId),
    Clear,
    Play,
    /// Reproduce tras `beats` golpes separados `interval` frames de salida.
    PlayWithCountIn { beats: u32, interval: f64 },
    Pause,
    Seek(u64),
    SetGain(TrackId, f32),
    SetPan(TrackId, f32),
    SetMute(TrackId, bool),
    SetSolo(TrackId, bool),
    SetMasterGain(f32),
    SetTempo(f64),
    SetPitch(f32),
    SetLoop { enabled: bool, start: u64, end: u64 },
    SetBeats(Box<BeatGrid>),
    SetMetronome { enabled: bool, gain: f32 },
    /// Sustituye la cadena de efectos de una pista (`None` = bus máster).
    SetChain(Option<TrackId>, Box<Chain>),
    SetSidechain(TrackId, Option<TrackId>),
    /// Cambia los parámetros de un efecto sin reconstruirlo.
    SetEffectParams(Option<TrackId>, usize, EffectParams),
    SetEffectEnabled(Option<TrackId>, usize, bool),
    /// Sustituye los clips de una pista.
    SetClips(TrackId, Box<Vec<ClipFrames>>),
    SetData(TrackId, Arc<Vec<f32>>, Box<Vec<ClipFrames>>),
    /// Sustituye la automatización de una pista (`None` = bus máster).
    SetAutomation(Option<TrackId>, Option<Box<ChannelAutomation>>),
    /// Grabando: el transporte sigue más allá del final de las pistas.
    SetFreeRun(bool),
    AttachMonitor(Box<Monitor>),
    DetachMonitor,
    SetMonitor { enabled: bool, gain: f32 },
    /// Golpes de calibración: `count` barridos separados `interval` frames,
    /// el primero `lead` frames después del bloque actual.
    CalibrationClicks { count: u32, interval: u64, lead: u64 },
}

/// Estado publicado por el hilo de audio.
pub(crate) struct Shared {
    position: AtomicU64,
    length: AtomicU64,
    playing: AtomicBool,
    counting_in: AtomicBool,
    loop_count: AtomicU64,
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    device_error: AtomicBool,
    /// Sonoridad momentánea y a corto plazo (bits de f64).
    lufs_m: AtomicU64,
    lufs_s: AtomicU64,
    /// (reloj de salida al inicio del último callback, instante en que suena en ns).
    out_anchor: Pair,
    /// (reloj de salida, posición) del último arranque del transporte.
    play_start: Pair,
    /// (reloj de salida del primer golpe, intervalo) de la última calibración.
    calib: Pair,
}

impl Shared {
    fn new() -> Self {
        Self {
            position: AtomicU64::new(0),
            length: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            counting_in: AtomicBool::new(false),
            loop_count: AtomicU64::new(0),
            peak_l: AtomicU32::new(0),
            peak_r: AtomicU32::new(0),
            device_error: AtomicBool::new(false),
            lufs_m: AtomicU64::new(f64::NEG_INFINITY.to_bits()),
            lufs_s: AtomicU64::new(f64::NEG_INFINITY.to_bits()),
            out_anchor: Pair::new(),
            play_start: Pair::new(),
            calib: Pair::new(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TransportState {
    pub position_frames: u64,
    pub length_frames: u64,
    pub playing: bool,
    pub counting_in: bool,
    /// Veces que el bucle A-B ha vuelto al inicio.
    pub loop_count: u64,
    /// Picos de salida (lineales) desde la última lectura.
    pub peak_l: f32,
    pub peak_r: f32,
    /// Sonoridad de la salida (sin metrónomo) en LUFS; `-inf` en silencio.
    pub lufs_momentary: f64,
    pub lufs_short_term: f64,
    pub recording: bool,
}

/// Estado de la entrada de audio abierta.
#[derive(Clone, Copy, Debug)]
pub struct InputLevels {
    /// Picos de entrada (lineales) desde la última lectura.
    pub peak_l: f32,
    pub peak_r: f32,
    pub recording: bool,
    /// Segundos grabados en la toma actual.
    pub recorded_secs: f64,
    pub device_error: bool,
}

/// Manejador del motor; es `Send + Sync` y puede compartirse entre hilos.
pub struct Engine {
    commands: Mutex<rtrb::Producer<Command>>,
    garbage: Mutex<rtrb::Consumer<Garbage>>,
    shared: Arc<Shared>,
    sample_rate: u32,
    device_name: String,
    next_id: AtomicU64,
    /// Copia de la rejilla y del tempo para calcular la cuenta previa.
    beats: Mutex<Vec<u64>>,
    tempo: Mutex<f64>,
    pitch: Mutex<f32>,
    loop_on: AtomicBool,
    input: Mutex<Option<Arc<InputHandle>>>,
    /// Monitorización (activada, ganancia lineal); se conserva al reabrir la entrada.
    monitor: Mutex<(bool, f32)>,
    shutdown: Mutex<Option<mpsc::Sender<()>>>,
}

impl Engine {
    /// Abre el dispositivo de salida predeterminado y arranca el hilo de audio.
    pub fn start() -> Result<Self, EngineError> {
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
        let (garbage_tx, garbage_rx) = rtrb::RingBuffer::new(MAX_TRACKS * 2);
        let shared = Arc::new(Shared::new());

        let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, String), EngineError>>();
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
        let shared_rt = shared.clone();

        // El stream de cpal no es `Send` en todas las plataformas: vive en su propio hilo.
        thread::Builder::new()
            .name("mn-audio".into())
            .spawn(move || match open_stream(cmd_rx, garbage_tx, shared_rt) {
                Ok((stream, rate, name)) => {
                    let _ = ready_tx.send(Ok((rate, name)));
                    let _ = shutdown_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| EngineError::Device(e.to_string()))?;

        let (sample_rate, device_name) = ready_rx
            .recv()
            .map_err(|_| EngineError::Device("el hilo de audio terminó inesperadamente".into()))??;

        Ok(Self {
            commands: Mutex::new(cmd_tx),
            garbage: Mutex::new(garbage_rx),
            shared,
            sample_rate,
            device_name,
            next_id: AtomicU64::new(1),
            beats: Mutex::new(Vec::new()),
            tempo: Mutex::new(1.0),
            pitch: Mutex::new(0.0),
            loop_on: AtomicBool::new(false),
            input: Mutex::new(None),
            monitor: Mutex::new((false, 1.0)),
            shutdown: Mutex::new(Some(shutdown_tx)),
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn device_error(&self) -> bool {
        self.shared.device_error.load(Ordering::Relaxed)
    }

    fn send(&self, cmd: Command) -> Result<(), EngineError> {
        self.collect_garbage();
        self.commands.lock().unwrap().push(cmd).map_err(|_| EngineError::QueueFull)
    }

    fn collect_garbage(&self) {
        let mut g = self.garbage.lock().unwrap();
        while let Ok(item) = g.pop() {
            drop(item);
        }
    }

    /// Añade una pista (estéreo entrelazado a `sample_rate()` Hz).
    pub fn add_track(&self, data: Arc<Vec<f32>>, sample_rate: u32) -> Result<TrackId, EngineError> {
        let frames = (data.len() / 2) as u64;
        self.add_track_with_clips(data, sample_rate, vec![ClipFrames::whole(0, frames)])
    }

    /// Añade una pista con sus clips (en frames del motor).
    pub fn add_track_with_clips(&self, data: Arc<Vec<f32>>, sample_rate: u32, clips: Vec<ClipFrames>) -> Result<TrackId, EngineError> {
        if sample_rate != self.sample_rate {
            return Err(EngineError::SampleRate { expected: self.sample_rate, got: sample_rate });
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut t = Box::new(EngineTrack {
            id,
            data,
            end: clips_end(&clips),
            clips,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            gain_l: 1.0,
            gain_r: 1.0,
            chain: None,
            automation: None,
            pdc: vec![[0.0; 2]; PDC_MAX],
            pdc_pos: 0,
            sidechain: None,
            key: vec![0.0; rt::MAX_IN_FRAMES * 2],
            key_valid: false,
        });
        t.update_gains();
        self.send(Command::AddTrack(t))?;
        Ok(id)
    }

    /// Sustituye los clips de una pista sin cortar la reproducción.
    /// Sustituye la automatización de una pista o, con `None`, la del máster.
    /// Debe construirse a la frecuencia del motor y con la cadena actual.
    pub fn set_automation(&self, track: Option<TrackId>, automation: Option<ChannelAutomation>) -> Result<(), EngineError> {
        self.send(Command::SetAutomation(track, automation.map(Box::new)))
    }

    pub fn set_clips(&self, id: TrackId, clips: Vec<ClipFrames>) -> Result<(), EngineError> {
        self.send(Command::SetClips(id, Box::new(clips)))
    }

    /// Sustituye el audio y los clips de una pista sin parar la reproducción
    /// (pistas de instrumento que se vuelven a generar al editar sus notas).
    pub fn set_track_data(&self, id: TrackId, data: Arc<Vec<f32>>, sample_rate: u32, clips: Vec<ClipFrames>) -> Result<(), EngineError> {
        if sample_rate != self.sample_rate {
            return Err(EngineError::SampleRate { expected: self.sample_rate, got: sample_rate });
        }
        self.send(Command::SetData(id, data, Box::new(clips)))
    }

    pub fn remove_track(&self, id: TrackId) -> Result<(), EngineError> {
        self.send(Command::RemoveTrack(id))
    }

    pub fn clear(&self) -> Result<(), EngineError> {
        self.ensure_not_recording()?;
        self.beats.lock().unwrap().clear();
        self.send(Command::Clear)
    }

    pub fn play(&self) -> Result<(), EngineError> {
        self.rewind_if_ended()?;
        self.send(Command::Play)
    }

    fn rewind_if_ended(&self) -> Result<(), EngineError> {
        let s = self.state();
        if s.length_frames > 0 && s.position_frames >= s.length_frames {
            self.send(Command::Seek(0))?;
        }
        Ok(())
    }

    /// Reproduce tras una cuenta previa de `beats` golpes al ritmo de la rejilla
    /// (o de `fallback_bpm` si no hay rejilla), ajustada al tempo actual.
    pub fn play_with_count_in(&self, beats: u32, fallback_bpm: f64) -> Result<(), EngineError> {
        self.rewind_if_ended()?;
        self.count_in_command(beats, fallback_bpm)
    }

    fn count_in_command(&self, beats: u32, fallback_bpm: f64) -> Result<(), EngineError> {
        let pos = self.shared.position.load(Ordering::Relaxed);
        let interval_src = {
            let grid = self.beats.lock().unwrap();
            let i = grid.partition_point(|&b| b < pos).clamp(1, grid.len().max(1));
            if grid.len() >= 2 {
                let i = i.min(grid.len() - 1);
                (grid[i] - grid[i - 1]) as f64
            } else {
                60.0 / fallback_bpm.max(20.0) * self.sample_rate as f64
            }
        };
        let tempo = *self.tempo.lock().unwrap();
        self.send(Command::PlayWithCountIn { beats: beats.max(1), interval: interval_src / tempo })
    }

    pub fn pause(&self) -> Result<(), EngineError> {
        self.send(Command::Pause)
    }

    pub fn seek(&self, frame: u64) -> Result<(), EngineError> {
        self.ensure_not_recording()?;
        let len = self.shared.length.load(Ordering::Relaxed);
        self.send(Command::Seek(if len > 0 { frame.min(len) } else { frame }))
    }

    pub fn set_gain_db(&self, id: TrackId, db: f32) -> Result<(), EngineError> {
        self.send(Command::SetGain(id, mn_core::mix::db_to_gain(db)))
    }

    pub fn set_pan(&self, id: TrackId, pan: f32) -> Result<(), EngineError> {
        self.send(Command::SetPan(id, pan))
    }

    pub fn set_mute(&self, id: TrackId, mute: bool) -> Result<(), EngineError> {
        self.send(Command::SetMute(id, mute))
    }

    pub fn set_solo(&self, id: TrackId, solo: bool) -> Result<(), EngineError> {
        self.send(Command::SetSolo(id, solo))
    }

    pub fn set_master_gain_db(&self, db: f32) -> Result<(), EngineError> {
        self.send(Command::SetMasterGain(mn_core::mix::db_to_gain(db)))
    }

    /// Velocidad de reproducción (0,25×–2×) sin cambiar el tono.
    pub fn set_tempo(&self, tempo: f64) -> Result<(), EngineError> {
        let t = tempo.clamp(0.25, rt::MAX_TEMPO);
        *self.tempo.lock().unwrap() = t;
        self.send(Command::SetTempo(t))
    }

    /// Transposición en semitonos (−24..24) sin cambiar la velocidad.
    pub fn set_pitch(&self, semitones: f32) -> Result<(), EngineError> {
        *self.pitch.lock().unwrap() = semitones;
        self.send(Command::SetPitch(semitones))
    }

    pub fn set_loop(&self, enabled: bool, start: u64, end: u64) -> Result<(), EngineError> {
        self.loop_on.store(enabled && end > start, Ordering::Relaxed);
        self.send(Command::SetLoop { enabled, start, end })
    }

    /// Rejilla de pulsos en frames del motor (ordenada).
    pub fn set_beats(&self, frames: Vec<u64>, beats_per_bar: u32, downbeat_offset: u32) -> Result<(), EngineError> {
        *self.beats.lock().unwrap() = frames.clone();
        self.send(Command::SetBeats(Box::new(BeatGrid { frames, beats_per_bar, downbeat_offset, accents: Vec::new() })))
    }

    /// Rejilla de pulsos detectados con sus tiempos fuertes (`accents[i]` para `frames[i]`).
    pub fn set_beats_with_accents(&self, frames: Vec<u64>, accents: Vec<bool>) -> Result<(), EngineError> {
        if accents.len() != frames.len() {
            return Err(EngineError::Invalid("pulsos y acentos de distinta longitud".into()));
        }
        *self.beats.lock().unwrap() = frames.clone();
        self.send(Command::SetBeats(Box::new(BeatGrid { frames, beats_per_bar: 0, downbeat_offset: 0, accents })))
    }

    /// Sustituye la cadena de efectos de una pista o, con `None`, la del bus máster.
    pub fn set_effects(&self, track: Option<TrackId>, slots: &[EffectSlot]) -> Result<(), EngineError> {
        // Se construye aquí (reserva memoria); el hilo de audio solo la intercambia.
        let chain = Box::new(Chain::new(slots, self.sample_rate));
        self.send(Command::SetChain(track, chain))
    }

    /// Pista cuya señal (sin efectos ni volumen) controla los efectos con
    /// sidechain de `track` (p. ej. el bombo que hace «respirar» al bajo).
    pub fn set_sidechain(&self, track: TrackId, source: Option<TrackId>) -> Result<(), EngineError> {
        self.send(Command::SetSidechain(track, source))
    }

    /// Cambia los parámetros de un efecto de la cadena sin cortar su sonido.
    pub fn set_effect_params(&self, track: Option<TrackId>, index: usize, params: EffectParams) -> Result<(), EngineError> {
        self.send(Command::SetEffectParams(track, index, params))
    }

    pub fn set_effect_enabled(&self, track: Option<TrackId>, index: usize, enabled: bool) -> Result<(), EngineError> {
        self.send(Command::SetEffectEnabled(track, index, enabled))
    }

    pub fn set_metronome(&self, enabled: bool, gain_db: f32) -> Result<(), EngineError> {
        self.send(Command::SetMetronome { enabled, gain: mn_core::mix::db_to_gain(gain_db) })
    }

    /// Lee el estado del transporte y reinicia los medidores de pico.
    pub fn state(&self) -> TransportState {
        let sh = &self.shared;
        TransportState {
            position_frames: sh.position.load(Ordering::Relaxed),
            length_frames: sh.length.load(Ordering::Relaxed),
            playing: sh.playing.load(Ordering::Relaxed),
            counting_in: sh.counting_in.load(Ordering::Relaxed),
            loop_count: sh.loop_count.load(Ordering::Relaxed),
            peak_l: f32::from_bits(sh.peak_l.swap(0, Ordering::Relaxed)),
            peak_r: f32::from_bits(sh.peak_r.swap(0, Ordering::Relaxed)),
            lufs_momentary: f64::from_bits(sh.lufs_m.load(Ordering::Relaxed)),
            lufs_short_term: f64::from_bits(sh.lufs_s.load(Ordering::Relaxed)),
            recording: self.is_recording(),
        }
    }

    // ---- Entrada de audio y grabación -------------------------------------

    fn ensure_not_recording(&self) -> Result<(), EngineError> {
        if self.is_recording() {
            Err(EngineError::Invalid("detén la grabación primero".into()))
        } else {
            Ok(())
        }
    }

    pub fn is_recording(&self) -> bool {
        self.input.lock().unwrap().as_ref().is_some_and(|i| i.is_recording())
    }

    /// Abre (o reabre) una entrada de audio; `None` = dispositivo predeterminado.
    pub fn open_input(&self, device: Option<&str>, channels: InputChannels) -> Result<InputInfo, EngineError> {
        self.ensure_not_recording()?;
        self.close_input();
        let (handle, monitor) = InputHandle::open(device, channels, self.sample_rate, self.shared.clone())?;
        if let Some(mut m) = monitor {
            let (enabled, gain) = *self.monitor.lock().unwrap();
            m.enabled = enabled;
            m.gain = gain;
            self.send(Command::AttachMonitor(Box::new(m)))?;
        }
        let info = handle.info.clone();
        *self.input.lock().unwrap() = Some(Arc::new(handle));
        Ok(info)
    }

    /// Cierra la entrada (si se está grabando, la toma se descarta).
    pub fn close_input(&self) {
        let _ = self.send(Command::DetachMonitor);
        if let Some(h) = self.input.lock().unwrap().take() {
            if h.is_recording() {
                let _ = self.send(Command::Pause);
                let _ = self.send(Command::SetFreeRun(false));
                if let Ok(Some(take)) = h.stop() {
                    let _ = std::fs::remove_file(&take.path);
                }
            }
        }
    }

    pub fn input_info(&self) -> Option<InputInfo> {
        self.input.lock().unwrap().as_ref().map(|i| i.info.clone())
    }

    /// Lee los niveles de entrada y reinicia sus picos; `None` sin entrada abierta.
    pub fn input_levels(&self) -> Option<InputLevels> {
        self.input.lock().unwrap().as_ref().map(|i| {
            let (peak_l, peak_r) = i.take_peaks();
            InputLevels {
                peak_l,
                peak_r,
                recording: i.is_recording(),
                recorded_secs: i.recorded_secs(),
                device_error: i.device_error(),
            }
        })
    }

    /// Monitorización de la entrada por la salida (solo si las frecuencias coinciden).
    pub fn set_monitor(&self, enabled: bool, gain_db: f32) -> Result<(), EngineError> {
        let gain = mn_core::mix::db_to_gain(gain_db);
        *self.monitor.lock().unwrap() = (enabled, gain);
        self.send(Command::SetMonitor { enabled, gain })
    }

    /// Empieza a grabar desde la posición actual. La toma arranca cuando el
    /// transporte empieza a sonar (tras la cuenta previa, si se pide) y se
    /// desplaza `latency_ms` para compensar la latencia de ida y vuelta.
    /// Requiere tempo 1×, sin transposición ni bucle.
    pub fn start_recording(&self, path: PathBuf, latency_ms: f64, count_in: Option<(u32, f64)>) -> Result<(), EngineError> {
        if (*self.tempo.lock().unwrap() - 1.0).abs() > 1e-6 {
            return Err(EngineError::Invalid("para grabar, la velocidad debe estar al 100 %".into()));
        }
        if self.pitch.lock().unwrap().abs() > 1e-6 {
            return Err(EngineError::Invalid("para grabar, quita la transposición de tono".into()));
        }
        if self.loop_on.load(Ordering::Relaxed) {
            return Err(EngineError::Invalid("para grabar, desactiva el bucle A-B".into()));
        }
        let guard = self.input.lock().unwrap();
        let input = guard.as_ref().ok_or_else(|| EngineError::Invalid("activa primero una entrada de audio".into()))?;
        if input.is_recording() {
            return Err(EngineError::Invalid("ya se está grabando".into()));
        }
        let (seq, _, _) = self.shared.play_start.load();
        input.record(path, latency_ms, seq)?;
        drop(guard);
        self.send(Command::Pause)?;
        self.send(Command::SetFreeRun(true))?;
        match count_in {
            Some((beats, bpm)) if beats > 0 => self.count_in_command(beats, bpm),
            _ => self.send(Command::Play),
        }
    }

    /// Detiene el transporte y la grabación. Devuelve la toma (`None` si no
    /// llegó a empezar, p. ej. al parar durante la cuenta previa).
    pub fn stop_recording(&self) -> Result<Option<Take>, EngineError> {
        self.send(Command::Pause)?;
        self.send(Command::SetFreeRun(false))?;
        let input = self.input.lock().unwrap().clone();
        match input {
            Some(i) if i.is_recording() => i.stop(),
            _ => Ok(None),
        }
    }

    /// Mide la latencia de ida y vuelta: reproduce 8 barridos y los busca en la
    /// entrada. Necesita que la salida llegue a la entrada (micrófono cerca de
    /// los altavoces o un cable de la salida a la entrada). Tarda unos 4 s.
    pub fn calibrate_latency(&self) -> Result<LatencyReport, EngineError> {
        const COUNT: u32 = 8;
        let input = self.input.lock().unwrap().clone();
        let input = input.as_deref().ok_or_else(|| EngineError::Invalid("activa primero una entrada de audio".into()))?;
        if input.is_recording() || self.shared.playing.load(Ordering::Relaxed) {
            return Err(EngineError::Invalid("detén la reproducción para calibrar".into()));
        }
        let out_rate = self.sample_rate;
        let in_rate = input.info.sample_rate;
        let interval = (out_rate as f64 * 0.4) as u64;
        let lead = (out_rate as f64 * 0.3) as u64;
        let total = (in_rate as f64 * (0.3 + 0.4 * COUNT as f64 + 0.6)) as usize;
        let (calib_seq, _, _) = self.shared.calib.load();
        // La captura empieza con el siguiente bloque de entrada; los golpes, 0,3 s
        // después de enviarlos, así que el primero cae dentro de lo capturado.
        let (start_frame, mono) = thread::scope(|s| {
            let h = s.spawn(|| input.capture(total, Duration::from_secs(10)));
            thread::sleep(Duration::from_millis(30));
            let sent = self.send(Command::CalibrationClicks { count: COUNT, interval, lead });
            let captured = h.join().unwrap_or_else(|_| Err(EngineError::Device("fallo en la captura".into())));
            sent.and(captured)
        })?;
        let (seq, first_click, interval) = self.shared.calib.load();
        if seq == calib_seq {
            return Err(EngineError::Device("la salida de audio no reprodujo los golpes".into()));
        }
        let (_, oclock, onanos) = self.shared.out_anchor.load();
        let (iframe, inanos) = input.anchor().ok_or_else(|| EngineError::Device("la entrada no entregó audio".into()))?;
        let expected: Vec<f64> = (0..COUNT as u64)
            .map(|k| {
                let t = frame_instant(oclock, onanos, first_click + k * interval, out_rate);
                iframe as f64 + (t - inanos as i128) as f64 * in_rate as f64 / 1e9 - start_frame as f64
            })
            .collect();
        measure_latency(&mono, in_rate, &expected, &click::chirp(in_rate)).map_err(EngineError::Invalid)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.close_input();
        if let Some(tx) = self.shutdown.lock().unwrap().take() {
            let _ = tx.send(());
        }
    }
}

fn open_stream(
    commands: rtrb::Consumer<Command>,
    garbage: rtrb::Producer<Garbage>,
    shared: Arc<Shared>,
) -> Result<(cpal::Stream, u32, String), EngineError> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or(EngineError::NoDevice)?;
    let name = device.name().unwrap_or_else(|_| "Salida de audio".into());
    let supported = device.default_output_config().map_err(|e| EngineError::Device(e.to_string()))?;
    let rate = supported.sample_rate().0;
    let config: cpal::StreamConfig = supported.config();
    let rt = RtState::new(rate, commands, garbage, shared, MAX_TRACKS);

    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, rt),
        SampleFormat::F64 => build::<f64>(&device, &config, rt),
        SampleFormat::I16 => build::<i16>(&device, &config, rt),
        SampleFormat::I32 => build::<i32>(&device, &config, rt),
        SampleFormat::U16 => build::<u16>(&device, &config, rt),
        SampleFormat::U8 => build::<u8>(&device, &config, rt),
        other => Err(EngineError::Device(format!("formato de muestra no soportado: {other}"))),
    }?;
    stream.play().map_err(|e| EngineError::Device(e.to_string()))?;
    Ok((stream, rate, name))
}

fn build<T: SizedSample + FromSample<f32> + 'static>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut rt: RtState,
) -> Result<cpal::Stream, EngineError> {
    let channels = config.channels as usize;
    let shared = rt.shared.clone();
    device
        .build_output_stream(
            config,
            move |out: &mut [T], info: &cpal::OutputCallbackInfo| {
                rt.render_at(out, channels, Some(instant_nanos(&info.timestamp().playback)))
            },
            move |err| {
                eprintln!("[mn-engine] error del stream: {err}");
                shared.device_error.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|e| EngineError::Device(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn rt_with(tracks: Vec<Box<EngineTrack>>) -> (RtState, rtrb::Producer<Command>) {
        let (tx, rx) = rtrb::RingBuffer::new(64);
        let (gtx, _grx) = rtrb::RingBuffer::new(64);
        let mut rt = RtState::new(RATE, rx, gtx, Arc::new(Shared::new()), MAX_TRACKS);
        rt.tracks.extend(tracks);
        (rt, tx)
    }

    fn track(id: TrackId, value: f32, frames: usize) -> Box<EngineTrack> {
        let mut t = Box::new(EngineTrack {
            id,
            data: Arc::new(vec![value; frames * 2]),
            clips: vec![ClipFrames::whole(0, frames as u64)],
            end: frames as u64,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            gain_l: 1.0,
            gain_r: 1.0,
            chain: None,
            automation: None,
            pdc: vec![[0.0; 2]; PDC_MAX],
            pdc_pos: 0,
            sidechain: None,
            key: vec![0.0; rt::MAX_IN_FRAMES * 2],
            key_valid: false,
        });
        t.update_gains();
        t
    }

    fn sine_track(id: TrackId, hz: f32, frames: usize) -> Box<EngineTrack> {
        let mut t = track(id, 0.0, frames);
        let data = (0..frames)
            .flat_map(|i| {
                let s = (2.0 * std::f32::consts::PI * hz * i as f32 / RATE as f32).sin() * 0.5;
                [s, s]
            })
            .collect();
        t.data = Arc::new(data);
        t
    }

    #[test]
    fn mixes_mute_solo_and_stops_at_end() {
        let (mut rt, mut tx) = rt_with(vec![track(1, 0.25, 6), track(2, 0.5, 4)]);
        let mut out = vec![0.0f32; 8];

        rt.render(&mut out, 2);
        assert!(out.iter().all(|&s| s == 0.0), "en pausa debe haber silencio");

        tx.push(Command::Play).ok();
        rt.render(&mut out, 2);
        assert!(out.iter().all(|&s| (s - 0.75).abs() < 1e-6));

        tx.push(Command::SetSolo(2, true)).ok();
        tx.push(Command::Seek(0)).ok();
        rt.render(&mut out, 2);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6));

        tx.push(Command::SetSolo(2, false)).ok();
        tx.push(Command::SetMute(2, true)).ok();
        tx.push(Command::SetPan(1, 1.0)).ok();
        rt.render(&mut out, 2); // frames 4..6, luego silencio
        assert_eq!(&out[..4], &[0.0, 0.25, 0.0, 0.25]);
        assert_eq!(rt.position, 6);
        assert!(!rt.playing, "debe detenerse al llegar al final");
    }

    #[test]
    fn loop_wraps_and_counts() {
        let (mut rt, mut tx) = rt_with(vec![track(1, 0.1, 10_000)]);
        tx.push(Command::SetLoop { enabled: true, start: 1000, end: 2000 }).ok();
        tx.push(Command::Play).ok();
        let mut out = vec![0.0f32; 2 * 3500];
        rt.render(&mut out, 2);
        assert!(rt.position >= 1000 && rt.position < 2000, "posición {}", rt.position);
        assert_eq!(rt.shared.loop_count.load(Ordering::Relaxed), 3);
        assert!(rt.playing);
    }

    #[test]
    fn tempo_changes_read_speed_and_keeps_audio() {
        let (mut rt, mut tx) = rt_with(vec![sine_track(1, 440.0, RATE as usize * 4)]);
        tx.push(Command::SetTempo(0.5)).ok();
        tx.push(Command::Play).ok();
        let mut out = vec![0.0f32; 2 * RATE as usize];
        rt.render(&mut out, 2);
        // A mitad de velocidad se lee medio segundo de audio por segundo de salida.
        assert!((rt.position as i64 - RATE as i64 / 2).abs() < 600, "posición {}", rt.position);
        let tail = &out[out.len() / 2..];
        let rms = (tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(rms > 0.2, "rms {rms}");
    }

    #[test]
    fn metronome_clicks_on_beats() {
        let (mut rt, mut tx) = rt_with(vec![track(1, 0.0, RATE as usize * 2)]);
        let beats: Vec<u64> = (0..4).map(|i| i * 24_000).collect();
        tx.push(Command::SetBeats(Box::new(BeatGrid { frames: beats, beats_per_bar: 4, downbeat_offset: 0, accents: Vec::new() }))).ok();
        tx.push(Command::SetMetronome { enabled: true, gain: 1.0 }).ok();
        tx.push(Command::Play).ok();
        let mut out = vec![0.0f32; 2 * 48_000];
        rt.render(&mut out, 2);
        let energy = |from: usize| out[from * 2..(from + 800) * 2].iter().map(|s| s.abs()).sum::<f32>();
        assert!(energy(0) > 1.0 && energy(24_000) > 1.0, "debe sonar en cada pulso");
        assert!(energy(12_000) < 1e-6, "silencio entre pulsos");
    }

    #[test]
    fn explicit_accents_mark_downbeats() {
        let g = BeatGrid { frames: vec![0, 10, 20, 30, 40], beats_per_bar: 0, downbeat_offset: 0, accents: vec![false, true, false, false, true] };
        assert_eq!((0..5).map(|i| g.is_downbeat(i)).collect::<Vec<_>>(), vec![false, true, false, false, true]);
        let g = BeatGrid { frames: vec![0, 10, 20, 30], beats_per_bar: 3, downbeat_offset: 1, accents: Vec::new() };
        assert_eq!((0..4).map(|i| g.is_downbeat(i)).collect::<Vec<_>>(), vec![false, true, false, false]);
    }

    #[test]
    fn offset_tracks_and_free_run() {
        let mut late = track(2, 0.5, 4);
        late.clips = vec![ClipFrames::whole(4, 4)];
        late.end = 8;
        let (mut rt, mut tx) = rt_with(vec![track(1, 0.25, 2), late]);
        tx.push(Command::Play).ok();
        let mut out = vec![0.0f32; 2 * 10];
        rt.render(&mut out, 2);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(left, vec![0.25, 0.25, 0.0, 0.0, 0.5, 0.5, 0.5, 0.5, 0.0, 0.0]);
        assert!(!rt.playing, "sin grabar se detiene al final (frame 8)");
        assert_eq!(rt.position, 8);

        // Grabando, el transporte sigue más allá del final.
        tx.push(Command::SetFreeRun(true)).ok();
        tx.push(Command::Play).ok();
        rt.render(&mut out, 2);
        assert!(rt.playing);
        assert_eq!(rt.position, 18);
        assert_eq!(rt.shared.length.load(Ordering::Relaxed), 18, "la duración crece con la toma");
        tx.push(Command::SetFreeRun(false)).ok();
        rt.render(&mut out, 2);
        assert!(!rt.playing);
    }

    fn volume_lane(points: &[(f64, f32)]) -> Vec<mn_core::automation::AutomationLane> {
        use mn_core::automation::{AutoPoint, AutomationLane, AutomationTarget};
        vec![AutomationLane {
            id: Default::default(),
            target: AutomationTarget::Volume,
            enabled: true,
            points: points.iter().map(|&(time, value)| AutoPoint { time, value }).collect(),
        }]
    }

    #[test]
    fn track_and_master_automation() {
        let (mut rt, mut tx) = rt_with(vec![track(1, 0.5, RATE as usize * 4)]);
        // Pista: silencio (-60 dB) hasta 0,5 s y 0 dB desde 1 s.
        let a = ChannelAutomation::build(&volume_lane(&[(0.5, -60.0), (1.0, 0.0)]), &[], RATE).unwrap();
        tx.push(Command::SetAutomation(Some(1), Some(Box::new(a)))).ok();
        tx.push(Command::Play).ok();
        let mut out = vec![0.0f32; 2 * RATE as usize / 4];
        rt.render(&mut out, 2); // 0..0,25 s
        assert!(out.iter().all(|&s| s == 0.0), "silencio automatizado");
        for _ in 0..4 {
            rt.render(&mut out, 2); // hasta 1,25 s
        }
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6), "0 dB al final");

        // Máster a -6 dB por automatización; quitarla devuelve el fader (0 dB).
        let m = ChannelAutomation::build(&volume_lane(&[(0.0, -6.0)]), &[], RATE).unwrap();
        tx.push(Command::SetAutomation(None, Some(Box::new(m)))).ok();
        rt.render(&mut out, 2);
        rt.render(&mut out, 2);
        assert!(out.iter().all(|&s| (s - 0.5 * 0.501_187).abs() < 1e-4));
        tx.push(Command::SetAutomation(None, None)).ok();
        tx.push(Command::SetAutomation(Some(1), None)).ok();
        rt.render(&mut out, 2);
        rt.render(&mut out, 2);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6));
    }

    /// Lo que suena en el motor y lo que se exporta deben coincidir también
    /// con automatización (volumen, panorama, parámetros de efectos y máster).
    #[test]
    fn realtime_matches_offline_mixdown_with_automation() {
        use mn_core::automation::{AutoPoint, AutomationLane, AutomationTarget};
        use mn_core::mix::{mixdown, MixInput};
        use mn_core::project::MasterSettings;
        let lane = |target, pts: &[(f64, f32)]| AutomationLane {
            id: uuid_like(),
            target,
            enabled: true,
            points: pts.iter().map(|&(time, value)| AutoPoint { time, value }).collect(),
        };
        let fx = vec![EffectSlot { plugin: None, sidechain: None, id: "rv".into(), enabled: true, params: EffectParams::Reverb(mn_fx::ReverbParams::default()) }];
        let master_fx = vec![EffectSlot { plugin: None, sidechain: None, id: "eq".into(), enabled: true, params: EffectParams::Eq(mn_fx::EqParams::default()) }];
        let lanes = vec![
            lane(AutomationTarget::Volume, &[(0.1, -20.0), (0.6, 3.0)]),
            lane(AutomationTarget::Pan, &[(0.0, -1.0), (0.8, 0.7)]),
            lane(AutomationTarget::Effect { slot: "rv".into(), param: "mix".into() }, &[(0.0, 0.0), (0.5, 0.8)]),
        ];
        let master_lanes = vec![
            lane(AutomationTarget::Volume, &[(0.2, 0.0), (0.7, -9.0)]),
            lane(AutomationTarget::Effect { slot: "eq".into(), param: "mid1.gain_db".into() }, &[(0.0, -12.0), (0.9, 12.0)]),
        ];
        let frames = RATE as usize;
        let t = sine_track(1, 330.0, frames);
        let data = t.data.clone();
        let (mut rt, mut tx) = rt_with(vec![t]);
        tx.push(Command::SetChain(Some(1), Box::new(Chain::new(&fx, RATE)))).ok();
        tx.push(Command::SetChain(None, Box::new(Chain::new(&master_fx, RATE)))).ok();
        tx.push(Command::SetAutomation(Some(1), ChannelAutomation::build(&lanes, &fx, RATE).map(Box::new))).ok();
        tx.push(Command::SetAutomation(None, ChannelAutomation::build(&master_lanes, &master_fx, RATE).map(Box::new))).ok();
        tx.push(Command::Play).ok();
        let mut live = vec![0.0f32; frames * 2];
        for chunk in live.chunks_mut(512 * 2 * 3) {
            rt.render(chunk, 2);
        }

        let clips = vec![ClipFrames::whole(0, frames as u64)];
        let auto = ChannelAutomation::build(&lanes, &fx, RATE).unwrap();
        let input = MixInput { data: &data, clips: &clips, gain_db: 0.0, pan: 0.0, mute: false, solo: false, effects: &fx, automation: Some(&auto), sidechain: None };
        let master = MasterSettings { gain_db: 0.0, effects: master_fx.clone(), automation: master_lanes.clone() };
        let master_auto = ChannelAutomation::build(&master_lanes, &master_fx, RATE).unwrap();
        let offline = mixdown(&[input], &master, Some(&master_auto), RATE);
        let max_diff = live.iter().zip(&offline.data).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        let peak = offline.data.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.1, "hay señal: {peak}");
        assert!(max_diff < 1e-5, "diferencia máxima {max_diff}");
    }

    fn uuid_like() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    /// Sidechain: el compresor de la pista 1 escucha la pista 2 (silenciada) y
    /// el motor suena igual que la mezcla offline.
    #[test]
    fn sidechain_ducks_live_and_offline_alike() {
        use mn_core::mix::{mixdown, MixInput};
        use mn_core::project::MasterSettings;
        let frames = RATE as usize;
        let pad = sine_track(1, 330.0, frames);
        let pad_data = pad.data.clone();
        // Clave: fuerte la primera mitad, silencio después.
        let key_data: Arc<Vec<f32>> = Arc::new((0..frames * 2).map(|i| if i < frames { 0.9 } else { 0.0 }).collect());
        let mut key = track(2, 0.0, frames);
        key.data = key_data.clone();
        key.mute = true;
        let comp = mn_fx::CompressorParams { threshold_db: -30.0, ratio: 10.0, knee_db: 0.0, attack_ms: 1.0, release_ms: 10.0, makeup_db: 0.0 };
        let fx = vec![EffectSlot { plugin: None, sidechain: Some("clave".into()), id: "c".into(), enabled: true, params: EffectParams::Compressor(comp) }];
        let (mut rt, mut tx) = rt_with(vec![pad, key]);
        tx.push(Command::SetChain(Some(1), Box::new(Chain::new(&fx, RATE)))).ok();
        tx.push(Command::SetSidechain(1, Some(2))).ok();
        tx.push(Command::Play).ok();
        let mut live = vec![0.0f32; frames * 2];
        for chunk in live.chunks_mut(512 * 2) {
            rt.render(chunk, 2);
        }
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        let (ducked, free) = (rms(&live[frames / 4..frames * 3 / 4]), rms(&live[frames * 5 / 4..frames * 7 / 4]));
        assert!(free > 0.1 && ducked < free * 0.2, "con clave {ducked}, sin clave {free}");

        let clips = vec![ClipFrames::whole(0, frames as u64)];
        let inputs = [
            MixInput { data: &pad_data, clips: &clips, gain_db: 0.0, pan: 0.0, mute: false, solo: false, effects: &fx, automation: None, sidechain: Some(1) },
            MixInput { data: &key_data, clips: &clips, gain_db: 0.0, pan: 0.0, mute: true, solo: false, effects: &[], automation: None, sidechain: None },
        ];
        let offline = mixdown(&inputs, &MasterSettings::default(), None, RATE);
        let max_diff = live.iter().zip(&offline.data).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(max_diff < 1e-5, "diferencia máxima {max_diff}");
    }

    #[test]
    fn clips_can_be_replaced_while_playing() {
        let (mut rt, mut tx) = rt_with(vec![track(1, 0.5, 100)]);
        tx.push(Command::Play).ok();
        let mut out = vec![0.0f32; 2 * 10];
        rt.render(&mut out, 2);
        assert!(out.iter().all(|&s| s == 0.5));
        // Solo dos tramos con media ganancia: 12..14 y 18..20.
        let clip = |start| ClipFrames { start, src_start: 0, len: 2, fade_in: 0, fade_out: 0, gain: 0.5 };
        tx.push(Command::SetClips(1, Box::new(vec![clip(12), clip(18)]))).ok();
        rt.render(&mut out, 2);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(left, vec![0.0, 0.0, 0.25, 0.25, 0.0, 0.0, 0.0, 0.0, 0.25, 0.25]);
        assert_eq!(rt.shared.length.load(Ordering::Relaxed), 20, "la duración sigue a los clips");
        rt.render(&mut out, 2);
        assert!(!rt.playing, "termina al acabar el último clip");
    }

    #[test]
    fn transport_start_is_announced_once() {
        let (mut rt, mut tx) = rt_with(vec![track(1, 0.1, RATE as usize)]);
        let mut out = vec![0.0f32; 2 * 512];
        rt.render(&mut out, 2);
        assert_eq!(rt.shared.play_start.load().0, 0);
        tx.push(Command::Seek(1000)).ok();
        tx.push(Command::Play).ok();
        rt.render_at(&mut out, 2, Some(5_000));
        assert_eq!(rt.shared.play_start.load(), (1, 512, 1000));
        assert_eq!(rt.shared.out_anchor.load(), (1, 512, 5_000));
        rt.render(&mut out, 2);
        assert_eq!(rt.shared.play_start.load().0, 1, "solo se anuncia al arrancar");

        // Con cuenta previa se anuncia cuando la música empieza de verdad.
        tx.push(Command::Pause).ok();
        tx.push(Command::PlayWithCountIn { beats: 1, interval: 700.0 }).ok();
        rt.render(&mut out, 2); // reloj 1536..2048: aún en cuenta previa
        assert_eq!(rt.shared.play_start.load().0, 1);
        rt.render(&mut out, 2); // la cuenta termina en 2236 (dentro de este bloque)
        rt.render(&mut out, 2); // la música arranca en el bloque siguiente
        let (seq, clock, pos) = rt.shared.play_start.load();
        assert_eq!((seq, clock), (2, 2560));
        assert_eq!(pos, 1000 + 512 * 2);
    }

    #[test]
    fn calibration_chirps_play_at_requested_frames() {
        let (mut rt, mut tx) = rt_with(vec![]);
        tx.push(Command::CalibrationClicks { count: 2, interval: 4000, lead: 300 }).ok();
        let mut out = vec![0.0f32; 2 * 8000];
        rt.render(&mut out, 2);
        assert_eq!(rt.shared.calib.load(), (1, 300, 4000));
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let first_sound = left.iter().position(|s| s.abs() > 1e-6).unwrap();
        assert!((300..304).contains(&first_sound), "{first_sound}");
        assert!(left[..300].iter().all(|&s| s == 0.0));
        assert!(left[4300..4600].iter().any(|s| s.abs() > 0.1), "segundo barrido");
    }

    #[test]
    fn monitor_input_reaches_output_unmetered() {
        let (mut rt, mut tx) = rt_with(vec![]);
        let (mut mtx, mrx) = rtrb::RingBuffer::new(48_000);
        let mut m = Monitor::new(mrx, RATE);
        m.enabled = true;
        m.gain = 0.5;
        tx.push(Command::AttachMonitor(Box::new(m))).ok();
        for _ in 0..2048 {
            mtx.push(0.4).unwrap();
        }
        let mut out = vec![0.0f32; 2 * 512];
        rt.render(&mut out, 2);
        assert!(out.iter().all(|&s| (s - 0.2).abs() < 1e-6));
        tx.push(Command::SetMonitor { enabled: false, gain: 1.0 }).ok();
        rt.render(&mut out, 2);
        assert!(out.iter().all(|&s| s == 0.0));
        assert!(rt.shared.lufs_m.load(Ordering::Relaxed) == f64::NEG_INFINITY.to_bits(), "no se mide");
        tx.push(Command::DetachMonitor).ok();
        rt.render(&mut out, 2);
        assert!(rt.monitor.is_none());
    }

    fn limiter(ceiling_db: f32) -> Vec<EffectSlot> {
        vec![EffectSlot { plugin: None, sidechain: None,
            id: "lim".into(),
            enabled: true,
            params: EffectParams::Limiter(mn_fx::LimiterParams { input_db: 0.0, ceiling_db, release_ms: 50.0, true_peak: false }),
        }]
    }

    fn peak(buf: &[f32]) -> f32 {
        buf.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn track_and_master_effects_are_applied_and_tweakable() {
        let (mut rt, mut tx) = rt_with(vec![sine_track(1, 440.0, RATE as usize), sine_track(2, 220.0, RATE as usize)]);
        // Limitador a -9 dBFS solo en la pista 1 (pico 0,5 → ≈0,355).
        tx.push(Command::SetChain(Some(1), Box::new(Chain::new(&limiter(-9.0), RATE)))).ok();
        tx.push(Command::SetMute(2, true)).ok();
        tx.push(Command::Play).ok();
        let mut out = vec![0.0f32; 2 * 9600];
        rt.render(&mut out, 2);
        let p = peak(&out[4800..]);
        assert!(p <= 0.3549 && p > 0.34, "pista limitada: {p}");

        // Cambio de parámetro sin reconstruir: techo a -12 dBFS.
        tx.push(Command::SetEffectParams(
            Some(1),
            0,
            EffectParams::Limiter(mn_fx::LimiterParams { input_db: 0.0, ceiling_db: -12.0, release_ms: 50.0, true_peak: false }),
        ))
        .ok();
        rt.render(&mut out, 2);
        let p = peak(&out[4800..]);
        assert!(p <= 0.2512 && p > 0.24, "techo nuevo: {p}");

        // Efecto desactivado → señal original; limitador en el máster a -6 dBFS.
        tx.push(Command::SetEffectEnabled(Some(1), 0, false)).ok();
        tx.push(Command::SetChain(None, Box::new(Chain::new(&limiter(-6.0), RATE)))).ok();
        rt.render(&mut out, 2);
        let p = peak(&out[4800..]);
        assert!(p <= 0.5012 && p > 0.48, "solo el máster limita: {p}");
    }

    #[test]
    fn loudness_meter_tracks_output() {
        let (mut rt, mut tx) = rt_with(vec![sine_track(1, 1000.0, RATE as usize * 2)]);
        tx.push(Command::Play).ok();
        let mut out = vec![0.0f32; 2 * 4800];
        for _ in 0..10 {
            rt.render(&mut out, 2);
        }
        // Seno estéreo de amplitud 0,5 (-6,02 dBFS) a 1 kHz → ≈ -6,0 LUFS.
        let m = f64::from_bits(rt.shared.lufs_m.load(Ordering::Relaxed));
        assert!((m + 6.02).abs() < 0.2, "{m}");
    }

    #[test]
    fn count_in_waits_then_plays() {
        let (mut rt, mut tx) = rt_with(vec![track(1, 0.2, RATE as usize * 4)]);
        tx.push(Command::PlayWithCountIn { beats: 2, interval: 4800.0 }).ok();
        let mut out = vec![0.0f32; 2 * 4800];
        rt.render(&mut out, 2);
        assert_eq!(rt.position, 0, "no avanza durante la cuenta previa");
        assert!(out[..1600].iter().any(|s| s.abs() > 0.1), "suena el primer golpe");
        rt.render(&mut out, 2);
        rt.render(&mut out, 2);
        assert!(rt.playing && rt.position > 0, "arranca tras la cuenta previa");
    }
}
