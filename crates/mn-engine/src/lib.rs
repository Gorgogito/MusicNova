//! Motor de audio en tiempo real de MusicNova.
//!
//! El hilo de audio (callback de cpal) nunca bloquea ni reserva memoria: recibe
//! órdenes por un ring buffer sin bloqueos (`rtrb`) y publica su estado mediante
//! atómicos. Los objetos retirados (pistas, rejillas de pulsos) se devuelven por
//! otro ring buffer para liberarlos fuera del hilo de audio.
//!
//! Funciones: mezcla multipista, tempo y tono en tiempo real (Signalsmith
//! Stretch), bucle A-B, metrónomo sobre una rejilla de pulsos y cuenta previa.

mod click;
mod rt;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use rt::RtState;

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
    frames: u64,
    gain: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    gain_l: f32,
    gain_r: f32,
}

impl EngineTrack {
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
        if sample_rate != self.sample_rate {
            return Err(EngineError::SampleRate { expected: self.sample_rate, got: sample_rate });
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let frames = (data.len() / 2) as u64;
        let mut t = Box::new(EngineTrack {
            id,
            data,
            frames,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            gain_l: 1.0,
            gain_r: 1.0,
        });
        t.update_gains();
        self.send(Command::AddTrack(t))?;
        Ok(id)
    }

    pub fn remove_track(&self, id: TrackId) -> Result<(), EngineError> {
        self.send(Command::RemoveTrack(id))
    }

    pub fn clear(&self) -> Result<(), EngineError> {
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
        self.send(Command::SetPitch(semitones))
    }

    pub fn set_loop(&self, enabled: bool, start: u64, end: u64) -> Result<(), EngineError> {
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
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
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
            move |out: &mut [T], _| rt.render(out, channels),
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
            frames: frames as u64,
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            gain_l: 1.0,
            gain_r: 1.0,
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
