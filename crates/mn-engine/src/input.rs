//! Entrada de audio: captura, monitorización, grabación alineada con la línea
//! de tiempo y medición de la latencia de ida y vuelta.
//!
//! El callback de entrada (tiempo real) solo copia muestras a dos ring buffers:
//! uno hacia el hilo escritor (`mn-rec`), que guarda las tomas en disco, y otro
//! hacia el callback de salida para la monitorización. La alineación usa las
//! marcas de tiempo de cpal: la salida publica en qué instante suena cada frame
//! y la entrada en qué instante se capturó; la diferencia constante que no
//! cubren (convertidores, altavoz, micrófono) se mide con [`measure_latency`]
//! y se compensa al grabar.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use mn_core::audio::WavF32Writer;

use crate::sync::{frame_instant, instant_nanos, Pair};
use crate::{EngineError, Shared};

/// Segundos de audio que caben en el ring buffer hacia el escritor.
const CAPTURE_SECONDS: usize = 4;
/// Segundos de audio que se conservan mientras se espera el arranque del transporte.
const BACKLOG_SECONDS: usize = 2;

#[derive(Clone, Debug, serde::Serialize)]
pub struct InputDeviceInfo {
    pub name: String,
    pub channels: u16,
    pub sample_rate: u32,
    pub is_default: bool,
}

/// Dispositivos de entrada del sistema.
pub fn input_devices() -> Vec<InputDeviceInfo> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| d.name().ok());
    let Ok(devices) = host.input_devices() else { return Vec::new() };
    devices
        .filter_map(|d| {
            let name = d.name().ok()?;
            let cfg = d.default_input_config().ok()?;
            Some(InputDeviceInfo {
                is_default: default.as_deref() == Some(name.as_str()),
                name,
                channels: cfg.channels(),
                sample_rate: cfg.sample_rate().0,
            })
        })
        .collect()
}

/// Canales del dispositivo que se graban (índices desde 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "mode", content = "first", rename_all = "snake_case")]
pub enum InputChannels {
    /// Un canal, duplicado en ambos lados.
    Mono(u16),
    /// Par de canales consecutivos.
    Stereo(u16),
}

impl InputChannels {
    fn indices(self) -> (usize, usize) {
        match self {
            Self::Mono(c) => (c as usize, c as usize),
            Self::Stereo(c) => (c as usize, c as usize + 1),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct InputInfo {
    pub device: String,
    pub sample_rate: u32,
    pub device_channels: u16,
    pub channels: InputChannels,
    /// La monitorización necesita la misma frecuencia en entrada y salida.
    pub monitor_available: bool,
}

/// Toma terminada.
#[derive(Clone, Debug)]
pub struct Take {
    pub path: PathBuf,
    pub sample_rate: u32,
    /// Segundo de la línea de tiempo en el que empieza la toma.
    pub offset_secs: f64,
    pub frames: u64,
    /// Frames perdidos porque el disco no siguió el ritmo (0 si todo fue bien).
    pub dropped_frames: u64,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct LatencyReport {
    /// Retraso de la grabación respecto a lo que indican las marcas de tiempo.
    pub latency_ms: f64,
    /// Dispersión entre golpes (máx. − mín. de los válidos).
    pub spread_ms: f64,
    pub detected: usize,
    pub total: usize,
    /// Pico de la señal captada (dBFS).
    pub level_db: f64,
}

/// Estado publicado por el callback de entrada.
pub(crate) struct InputShared {
    /// (índice del primer frame del último callback, instante de captura en ns).
    anchor: Pair,
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    dropped: AtomicU64,
    error: AtomicBool,
    recorded_frames: AtomicU64,
    recording: AtomicBool,
}

/// Lado de salida de la monitorización: vive en el hilo de audio.
pub(crate) struct Monitor {
    consumer: rtrb::Consumer<f32>,
    pub enabled: bool,
    pub gain: f32,
    primed: bool,
    /// Muestras acumuladas antes de empezar a sonar.
    prime: usize,
    /// Si se acumulan más, se descarta el exceso (deriva entre relojes).
    max: usize,
}

impl Monitor {
    pub(crate) fn new(consumer: rtrb::Consumer<f32>, rate: u32) -> Self {
        Self {
            consumer,
            enabled: false,
            gain: 1.0,
            primed: false,
            prime: (rate as usize / 100) * 2,
            max: (rate as usize / 25) * 2,
        }
    }

    /// Suma la entrada pendiente a `out` (estéreo entrelazado). Sin reservas.
    pub fn mix_into(&mut self, out: &mut [f32]) {
        let avail = self.consumer.slots() & !1;
        if !self.enabled {
            if let Ok(c) = self.consumer.read_chunk(avail) {
                c.commit_all();
            }
            self.primed = false;
            return;
        }
        if avail > self.max {
            if let Ok(c) = self.consumer.read_chunk((avail - self.prime) & !1) {
                c.commit_all();
            }
        }
        let avail = self.consumer.slots() & !1;
        if !self.primed {
            if avail < self.prime {
                return;
            }
            self.primed = true;
        }
        let take = out.len().min(avail);
        if let Ok(c) = self.consumer.read_chunk(take) {
            let (a, b) = c.as_slices();
            for (o, s) in out.iter_mut().zip(a.iter().chain(b)) {
                *o += s * self.gain;
            }
            c.commit_all();
        }
        if take < out.len() {
            self.primed = false;
        }
    }
}

enum WriterMsg {
    Record { path: PathBuf, latency_ns: i64, after_seq: u64 },
    Stop { reply: mpsc::Sender<Result<Option<Take>, String>> },
    Calibrate { frames: usize, reply: mpsc::Sender<(u64, Vec<f32>)> },
}

/// Entrada abierta: flujo de captura y escritor.
pub(crate) struct InputHandle {
    pub info: InputInfo,
    pub shared: Arc<InputShared>,
    shutdown: Option<mpsc::Sender<()>>,
    writer_tx: Option<mpsc::Sender<WriterMsg>>,
    writer: Option<thread::JoinHandle<()>>,
}

impl InputHandle {
    pub fn open(
        device: Option<&str>,
        channels: InputChannels,
        out_rate: u32,
        out_shared: Arc<Shared>,
    ) -> Result<(Self, Option<Monitor>), EngineError> {
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(InputInfo, rtrb::Consumer<f32>, rtrb::Consumer<f32>), EngineError>>();
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
        let shared = Arc::new(InputShared {
            anchor: Pair::new(),
            peak_l: AtomicU32::new(0),
            peak_r: AtomicU32::new(0),
            dropped: AtomicU64::new(0),
            error: AtomicBool::new(false),
            recorded_frames: AtomicU64::new(0),
            recording: AtomicBool::new(false),
        });
        let device = device.map(str::to_string);
        let shared_cb = shared.clone();
        thread::Builder::new()
            .name("mn-input".into())
            .spawn(move || match open_stream(device.as_deref(), channels, out_rate, shared_cb) {
                Ok((stream, info, capture, monitor)) => {
                    let _ = ready_tx.send(Ok((info, capture, monitor)));
                    let _ = shutdown_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .map_err(|e| EngineError::Device(e.to_string()))?;
        let (info, capture, monitor_rx) = ready_rx
            .recv()
            .map_err(|_| EngineError::Device("el hilo de entrada terminó inesperadamente".into()))??;

        let (writer_tx, writer_rx) = mpsc::channel();
        let ctx = WriterCtx {
            capture,
            input: shared.clone(),
            output: out_shared,
            in_rate: info.sample_rate,
            out_rate,
        };
        let writer = thread::Builder::new()
            .name("mn-rec".into())
            .spawn(move || writer_loop(ctx, writer_rx))
            .map_err(|e| EngineError::Device(e.to_string()))?;
        let monitor = info.monitor_available.then(|| Monitor::new(monitor_rx, out_rate));
        Ok((
            Self { info, shared, shutdown: Some(shutdown_tx), writer_tx: Some(writer_tx), writer: Some(writer) },
            monitor,
        ))
    }

    fn send(&self, msg: WriterMsg) -> Result<(), EngineError> {
        self.writer_tx
            .as_ref()
            .and_then(|tx| tx.send(msg).ok())
            .ok_or_else(|| EngineError::Device("el escritor de grabación no está activo".into()))
    }

    pub fn is_recording(&self) -> bool {
        self.shared.recording.load(Ordering::Relaxed)
    }

    pub fn device_error(&self) -> bool {
        self.shared.error.load(Ordering::Relaxed)
    }

    /// Picos de entrada desde la última lectura (y los reinicia).
    pub fn take_peaks(&self) -> (f32, f32) {
        (
            f32::from_bits(self.shared.peak_l.swap(0, Ordering::Relaxed)),
            f32::from_bits(self.shared.peak_r.swap(0, Ordering::Relaxed)),
        )
    }

    pub fn recorded_secs(&self) -> f64 {
        self.shared.recorded_frames.load(Ordering::Relaxed) as f64 / self.info.sample_rate as f64
    }

    /// Prepara una toma: empezará en el próximo arranque del transporte.
    pub fn record(&self, path: PathBuf, latency_ms: f64, play_seq: u64) -> Result<(), EngineError> {
        self.shared.recording.store(true, Ordering::Relaxed);
        self.shared.recorded_frames.store(0, Ordering::Relaxed);
        self.send(WriterMsg::Record { path, latency_ns: (latency_ms * 1e6) as i64, after_seq: play_seq })
    }

    pub fn stop(&self) -> Result<Option<Take>, EngineError> {
        let (tx, rx) = mpsc::channel();
        self.send(WriterMsg::Stop { reply: tx })?;
        let r = rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| EngineError::Device("el escritor de grabación no respondió".into()))?;
        self.shared.recording.store(false, Ordering::Relaxed);
        r.map_err(EngineError::Device)
    }

    /// Captura `frames` frames (mono) a partir del siguiente bloque recibido.
    pub fn capture(&self, frames: usize, timeout: Duration) -> Result<(u64, Vec<f32>), EngineError> {
        let (tx, rx) = mpsc::channel();
        self.send(WriterMsg::Calibrate { frames, reply: tx })?;
        rx.recv_timeout(timeout)
            .map_err(|_| EngineError::Device("la entrada no entregó audio (¿dispositivo desconectado?)".into()))
    }

    /// Ancla de la entrada: (índice de frame, instante en ns); `None` sin datos aún.
    pub fn anchor(&self) -> Option<(u64, u64)> {
        let (seq, f, n) = self.shared.anchor.load();
        (seq > 0).then_some((f, n))
    }
}

impl Drop for InputHandle {
    fn drop(&mut self) {
        // Cerrar el canal termina el escritor; después se detiene el flujo.
        self.writer_tx.take();
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

fn find_device(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device, EngineError> {
    match name {
        None => host.default_input_device().ok_or_else(|| EngineError::Device("no hay ningún dispositivo de entrada".into())),
        Some(n) => host
            .input_devices()
            .map_err(|e| EngineError::Device(e.to_string()))?
            .find(|d| d.name().ok().as_deref() == Some(n))
            .ok_or_else(|| EngineError::Device(format!("no se encontró el dispositivo de entrada «{n}»"))),
    }
}

#[allow(clippy::type_complexity)]
fn open_stream(
    device: Option<&str>,
    channels: InputChannels,
    out_rate: u32,
    shared: Arc<InputShared>,
) -> Result<(cpal::Stream, InputInfo, rtrb::Consumer<f32>, rtrb::Consumer<f32>), EngineError> {
    let host = cpal::default_host();
    let device = find_device(&host, device)?;
    let name = device.name().unwrap_or_else(|_| "Entrada de audio".into());
    let default = device.default_input_config().map_err(|e| EngineError::Device(e.to_string()))?;
    // Se prefiere la frecuencia del motor (permite monitorizar y evita remuestrear).
    let supported = device
        .supported_input_configs()
        .ok()
        .and_then(|mut it| {
            it.find(|c| {
                c.channels() == default.channels()
                    && c.sample_format() == default.sample_format()
                    && c.min_sample_rate().0 <= out_rate
                    && c.max_sample_rate().0 >= out_rate
            })
        })
        .map(|c| c.with_sample_rate(cpal::SampleRate(out_rate)))
        .unwrap_or(default);
    let rate = supported.sample_rate().0;
    let dev_channels = supported.channels();
    let (l, r) = channels.indices();
    if r >= dev_channels as usize {
        return Err(EngineError::Invalid(format!("el dispositivo solo tiene {dev_channels} canales de entrada")));
    }
    let config: cpal::StreamConfig = supported.config();
    let (cap_tx, cap_rx) = rtrb::RingBuffer::new(rate as usize * 2 * CAPTURE_SECONDS);
    let (mon_tx, mon_rx) = rtrb::RingBuffer::new(rate as usize * 2 / 5);
    let cb = InputCallback { capture: cap_tx, monitor: mon_tx, shared, l, r, channels: dev_channels as usize, frame: 0 };
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, &config, cb),
        SampleFormat::F64 => build::<f64>(&device, &config, cb),
        SampleFormat::I16 => build::<i16>(&device, &config, cb),
        SampleFormat::I32 => build::<i32>(&device, &config, cb),
        SampleFormat::U16 => build::<u16>(&device, &config, cb),
        SampleFormat::U8 => build::<u8>(&device, &config, cb),
        other => Err(EngineError::Device(format!("formato de muestra no soportado: {other}"))),
    }?;
    stream.play().map_err(|e| EngineError::Device(e.to_string()))?;
    let info = InputInfo { device: name, sample_rate: rate, device_channels: dev_channels, channels, monitor_available: rate == out_rate };
    Ok((stream, info, cap_rx, mon_rx))
}

struct InputCallback {
    capture: rtrb::Producer<f32>,
    monitor: rtrb::Producer<f32>,
    shared: Arc<InputShared>,
    l: usize,
    r: usize,
    channels: usize,
    /// Frames entregados al escritor (su índice coincide con el del escritor).
    frame: u64,
}

impl InputCallback {
    fn process<T: SizedSample>(&mut self, data: &[T], capture_nanos: u64)
    where
        f32: FromSample<T>,
    {
        let ch = self.channels;
        let frames = data.len() / ch;
        let (l, r) = (self.l, self.r);
        let stereo = |f: usize| -> [f32; 2] { [data[f * ch + l].to_sample::<f32>(), data[f * ch + r].to_sample::<f32>()] };
        let (mut pl, mut pr) = (0.0f32, 0.0f32);
        for f in 0..frames {
            let [a, b] = stereo(f);
            pl = pl.max(a.abs());
            pr = pr.max(b.abs());
        }
        self.shared.peak_l.fetch_max(pl.to_bits(), Ordering::Relaxed);
        self.shared.peak_r.fetch_max(pr.to_bits(), Ordering::Relaxed);

        let need = frames * 2;
        if self.capture.slots() >= need {
            self.shared.anchor.store(self.frame, capture_nanos);
            if let Ok(chunk) = self.capture.write_chunk_uninit(need) {
                chunk.fill_from_iter((0..frames).flat_map(stereo));
            }
            self.frame += frames as u64;
        } else {
            self.shared.dropped.fetch_add(frames as u64, Ordering::Relaxed);
        }
        if self.monitor.slots() >= need {
            if let Ok(chunk) = self.monitor.write_chunk_uninit(need) {
                chunk.fill_from_iter((0..frames).flat_map(stereo));
            }
        }
    }
}

fn build<T: SizedSample + 'static>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut cb: InputCallback,
) -> Result<cpal::Stream, EngineError>
where
    f32: FromSample<T>,
{
    let shared = cb.shared.clone();
    device
        .build_input_stream(
            config,
            move |data: &[T], info: &cpal::InputCallbackInfo| cb.process(data, instant_nanos(&info.timestamp().capture)),
            move |err| {
                eprintln!("[mn-engine] error de la entrada: {err}");
                shared.error.store(true, Ordering::Relaxed);
            },
            None,
        )
        .map_err(|e| EngineError::Device(e.to_string()))
}

struct WriterCtx {
    capture: rtrb::Consumer<f32>,
    input: Arc<InputShared>,
    output: Arc<Shared>,
    in_rate: u32,
    out_rate: u32,
}

struct Recording {
    path: PathBuf,
    latency_ns: i64,
    after_seq: u64,
    writer: Option<WavF32Writer>,
    offset_secs: f64,
    dropped_at_start: u64,
    error: Option<String>,
}

struct Calibration {
    frames: usize,
    start: Option<u64>,
    data: Vec<f32>,
    reply: mpsc::Sender<(u64, Vec<f32>)>,
}

/// Instante de arranque del transporte publicado por la salida, si es
/// posterior a `after_seq`: (instante en ns, posición en frames de salida).
fn play_start(out: &Shared, after_seq: u64, out_rate: u32) -> Option<(i128, u64)> {
    let (seq, clock, position) = out.play_start.load();
    if seq <= after_seq {
        return None;
    }
    let (aseq, aclock, ananos) = out.out_anchor.load();
    (aseq > 0).then(|| (frame_instant(aclock, ananos, clock, out_rate), position))
}

fn writer_loop(mut ctx: WriterCtx, rx: mpsc::Receiver<WriterMsg>) {
    let mut next_frame: u64 = 0;
    let mut buf: Vec<f32> = Vec::with_capacity(ctx.in_rate as usize);
    let mut backlog: VecDeque<f32> = VecDeque::new();
    let mut backlog_start: u64 = 0;
    let backlog_cap = ctx.in_rate as usize * 2 * BACKLOG_SECONDS;
    let mut rec: Option<Recording> = None;
    let mut calib: Option<Calibration> = None;

    loop {
        match rx.recv_timeout(Duration::from_millis(5)) {
            Ok(WriterMsg::Record { path, latency_ns, after_seq }) => {
                backlog.clear();
                backlog_start = next_frame;
                rec = Some(Recording {
                    path,
                    latency_ns,
                    after_seq,
                    writer: None,
                    offset_secs: 0.0,
                    dropped_at_start: ctx.input.dropped.load(Ordering::Relaxed),
                    error: None,
                });
            }
            Ok(WriterMsg::Stop { reply }) => {
                let result = match rec.take() {
                    None => Ok(None),
                    Some(r) => finish(r, &ctx),
                };
                let _ = reply.send(result);
            }
            Ok(WriterMsg::Calibrate { frames, reply }) => {
                calib = Some(Calibration { frames, start: None, data: Vec::with_capacity(frames), reply });
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let n = ctx.capture.slots() & !1;
        buf.clear();
        if n > 0 {
            if let Ok(chunk) = ctx.capture.read_chunk(n) {
                let (a, b) = chunk.as_slices();
                buf.extend_from_slice(a);
                buf.extend_from_slice(b);
                chunk.commit_all();
            }
        }
        let first = next_frame;
        next_frame += (buf.len() / 2) as u64;

        if let Some(c) = calib.as_mut() {
            if !buf.is_empty() {
                c.start.get_or_insert(first);
                let room = c.frames - c.data.len();
                c.data.extend(buf.chunks_exact(2).take(room).map(|f| 0.5 * (f[0] + f[1])));
                if c.data.len() >= c.frames {
                    let c = calib.take().unwrap();
                    let _ = c.reply.send((c.start.unwrap_or(first), c.data));
                }
            }
        }

        let Some(r) = rec.as_mut() else { continue };
        if r.error.is_some() {
            continue;
        }
        if let Some(w) = r.writer.as_mut() {
            if let Err(e) = w.write(&buf) {
                r.error = Some(format!("no se pudo escribir la grabación: {e}"));
            }
            ctx.input.recorded_frames.store(w.frames(), Ordering::Relaxed);
            continue;
        }

        // Toma pendiente: se guarda lo reciente hasta saber cuándo arrancó el transporte.
        backlog.extend(&buf);
        if backlog.len() > backlog_cap {
            let excess = (backlog.len() - backlog_cap) & !1;
            backlog.drain(..excess);
            backlog_start += (excess / 2) as u64;
        }
        let Some((start_ns, start_pos)) = play_start(&ctx.output, r.after_seq, ctx.out_rate) else { continue };
        let Some((aidx, anano)) = ({
            let (seq, f, n) = ctx.input.anchor.load();
            (seq > 0).then_some((f, n))
        }) else {
            continue;
        };
        // Primer frame de entrada capturado (descontada la latencia) en o tras el arranque.
        let delta_ns = start_ns + r.latency_ns as i128 - anano as i128;
        let n0 = (aidx as i128 + (delta_ns * ctx.in_rate as i128 + 999_999_999).div_euclid(1_000_000_000)).max(backlog_start as i128) as u64;
        let backlog_end = backlog_start + (backlog.len() / 2) as u64;
        if n0 > backlog_end {
            continue;
        }
        let t0 = frame_instant(aidx, anano, n0, ctx.in_rate) - r.latency_ns as i128;
        r.offset_secs = start_pos as f64 / ctx.out_rate as f64 + ((t0 - start_ns) as f64 / 1e9).max(0.0);
        match WavF32Writer::create(&r.path, ctx.in_rate) {
            Ok(mut w) => {
                let skip = ((n0 - backlog_start) * 2) as usize;
                let (a, b) = backlog.as_slices();
                let res = if skip < a.len() {
                    w.write(&a[skip..]).and_then(|_| w.write(b))
                } else {
                    w.write(&b[skip - a.len()..])
                };
                if let Err(e) = res {
                    r.error = Some(format!("no se pudo escribir la grabación: {e}"));
                }
                ctx.input.recorded_frames.store(w.frames(), Ordering::Relaxed);
                r.writer = Some(w);
            }
            Err(e) => r.error = Some(format!("no se pudo crear {}: {e}", r.path.display())),
        }
        backlog.clear();
    }
    if let Some(r) = rec.take() {
        let _ = finish(r, &ctx);
    }
}

fn finish(r: Recording, ctx: &WriterCtx) -> Result<Option<Take>, String> {
    let Some(w) = r.writer else {
        return match r.error {
            Some(e) => Err(e),
            None => Ok(None),
        };
    };
    let frames = w.finish().map_err(|e| e.to_string())?;
    if let Some(e) = r.error {
        return Err(e);
    }
    Ok(Some(Take {
        path: r.path,
        sample_rate: ctx.in_rate,
        offset_secs: r.offset_secs,
        frames,
        dropped_frames: ctx.input.dropped.load(Ordering::Relaxed) - r.dropped_at_start,
    }))
}

/// Localiza cada golpe de calibración en la grabación por correlación cruzada.
///
/// `expected[k]` es el frame de `input` en el que llegaría el golpe `k` con
/// latencia cero (según las marcas de tiempo). Devuelve la latencia mediana de
/// los golpes detectados con claridad.
pub fn measure_latency(input: &[f32], rate: u32, expected: &[f64], template: &[f32]) -> Result<LatencyReport, String> {
    let peak = input.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let level_db = 20.0 * (peak.max(1e-9) as f64).log10();
    if peak < 0.003 {
        return Err(format!(
            "no llega señal a la entrada ({level_db:.0} dBFS): acerca el micrófono a los altavoces o conecta la salida a la entrada"
        ));
    }
    let before = (rate as f64 * 0.02) as i64;
    let after = (rate as f64 * 0.35) as i64;
    let m = template.len();
    let mut lags = Vec::new();
    let mut corr = Vec::new();
    for &e in expected {
        let e = e.round() as i64;
        let lo = (e - before).max(0) as usize;
        let hi = ((e + after) as usize).min(input.len().saturating_sub(m));
        if hi <= lo {
            continue;
        }
        corr.clear();
        corr.extend((lo..hi).map(|j| {
            let x = &input[j..j + m];
            x.iter().zip(template).map(|(a, b)| a * b).sum::<f32>().abs()
        }));
        let (best, &best_v) = corr.iter().enumerate().fold((0, &0.0f32), |acc, (i, v)| if *v > *acc.1 { (i, v) } else { acc });
        let rms = (corr.iter().map(|v| v * v).sum::<f32>() / corr.len() as f32).sqrt();
        if best_v > 6.0 * rms && best_v > 1e-6 {
            lags.push((lo + best) as f64 - e as f64);
        }
    }
    if lags.is_empty() {
        return Err("no se detectaron los golpes: sube el volumen de salida o acerca el micrófono".into());
    }
    lags.sort_by(f64::total_cmp);
    let median = lags[lags.len() / 2];
    let tol = rate as f64 * 0.002;
    let good: Vec<f64> = lags.iter().copied().filter(|l| (l - median).abs() <= tol).collect();
    let needed = expected.len().div_ceil(2).max(3);
    if good.len() < needed {
        return Err(format!(
            "medición inestable ({} de {} golpes coinciden): reduce el ruido ambiente y repite",
            good.len(),
            expected.len()
        ));
    }
    let mean = good.iter().sum::<f64>() / good.len() as f64;
    let spread = good.last().unwrap() - good.first().unwrap();
    Ok(LatencyReport {
        latency_ms: mean / rate as f64 * 1000.0,
        spread_ms: spread / rate as f64 * 1000.0,
        detected: good.len(),
        total: expected.len(),
        level_db,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::click::chirp;

    fn noise(n: usize, amp: f32) -> Vec<f32> {
        let mut seed: u32 = 12345;
        (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32 - 0.5) * 2.0 * amp
            })
            .collect()
    }

    fn input_shared() -> Arc<InputShared> {
        Arc::new(InputShared {
            anchor: Pair::new(),
            peak_l: AtomicU32::new(0),
            peak_r: AtomicU32::new(0),
            dropped: AtomicU64::new(0),
            error: AtomicBool::new(false),
            recorded_frames: AtomicU64::new(0),
            recording: AtomicBool::new(false),
        })
    }

    /// Lanza el escritor con datos sintéticos: el frame `n` de la entrada vale
    /// `n / 1e6` (izquierda) y su opuesto (derecha).
    fn run_writer(in_rate: u32, out_rate: u32, latency_ns: i64, frames: u32, stop_before_start: bool) -> Option<Take> {
        let (mut tx, rx) = rtrb::RingBuffer::new(in_rate as usize * 2 * 4);
        let input = input_shared();
        let output = Arc::new(Shared::new());
        // La salida: el frame 0 suena en t = 1 s. La entrada: el frame 0 se capturó en t = 1 s.
        output.out_anchor.store(0, 1_000_000_000);
        input.anchor.store(0, 1_000_000_000);
        let ctx = WriterCtx { capture: rx, input: input.clone(), output: output.clone(), in_rate, out_rate };
        let (wtx, wrx) = mpsc::channel();
        let h = thread::spawn(move || writer_loop(ctx, wrx));
        let path = std::env::temp_dir().join(format!("mn_take_{}_{in_rate}_{latency_ns}.wav", std::process::id()));
        wtx.send(WriterMsg::Record { path: path.clone(), latency_ns, after_seq: 0 }).unwrap();
        if !stop_before_start {
            // El transporte arrancó en el frame de salida 0,1 s, en la posición 2 s.
            output.play_start.store(out_rate as u64 / 10, 2 * out_rate as u64);
        }
        for n in 0..frames {
            let v = n as f32 / 1e6;
            tx.push(v).unwrap();
            tx.push(-v).unwrap();
        }
        thread::sleep(Duration::from_millis(80));
        let (rtx, rrx) = mpsc::channel();
        wtx.send(WriterMsg::Stop { reply: rtx }).unwrap();
        let take = rrx.recv().unwrap().unwrap();
        drop(wtx);
        h.join().unwrap();
        if take.is_none() {
            assert!(!path.exists());
        }
        take
    }

    #[test]
    fn writer_aligns_take_with_transport_start() {
        let take = run_writer(48_000, 48_000, 10_000_000, 30_000, false).unwrap();
        // Arranque en t = 1,1 s + 10 ms de latencia → frame de entrada 5280, que es la posición 2 s.
        assert!((take.offset_secs - 2.0).abs() < 1e-9, "{take:?}");
        assert_eq!(take.frames, 30_000 - 5_280);
        assert_eq!(take.dropped_frames, 0);
        let back = mn_core::audio::decode_file(&take.path).unwrap();
        std::fs::remove_file(&take.path).ok();
        assert_eq!(back.data[0], 5_280.0 / 1e6);
        assert_eq!(back.data[1], -5_280.0 / 1e6);
        assert_eq!(back.frames() as u64, take.frames);
    }

    #[test]
    fn writer_handles_other_input_rates_and_sub_frame_offsets() {
        // Entrada a 44,1 kHz: 0,11 s → 4851 frames exactos; sin latencia → 4410.
        let take = run_writer(44_100, 48_000, 0, 20_000, false).unwrap();
        std::fs::remove_file(&take.path).ok();
        assert_eq!(take.frames, 20_000 - 4_410);
        assert!((take.offset_secs - 2.0).abs() < 1e-9, "{take:?}");
        // Latencia que no cae en un frame entero: el desfase recoge la fracción.
        let take = run_writer(44_100, 48_000, 5_010_000, 20_000, false).unwrap();
        std::fs::remove_file(&take.path).ok();
        let lat_frames: f64 = 0.105_01 * 44_100.0; // 4630,94 → primer frame 4631
        assert_eq!(take.frames, 20_000 - lat_frames.ceil() as u64);
        let expected = 2.0 + (lat_frames.ceil() - lat_frames) / 44_100.0;
        assert!((take.offset_secs - expected).abs() < 1e-6, "{} vs {expected}", take.offset_secs);
    }

    #[test]
    fn writer_discards_take_that_never_started() {
        assert!(run_writer(48_000, 48_000, 0, 10_000, true).is_none());
    }

    #[test]
    fn measures_latency_of_delayed_filtered_chirps() {
        let rate = 48_000;
        let c = chirp(rate);
        let mut x = noise(rate as usize * 4, 0.01);
        let expected: Vec<f64> = (0..8).map(|k| 14_400.0 + k as f64 * 19_200.0).collect();
        let lag = 1_337usize; // 27,85 ms
        for &e in &expected {
            let at = e as usize + lag;
            // Atenuado, invertido y algo suavizado, como tras altavoz y micrófono.
            let mut prev = 0.0;
            for (i, s) in c.iter().enumerate() {
                let v = -0.2 * s;
                x[at + i] += 0.6 * v + 0.4 * prev;
                prev = v;
            }
        }
        let r = measure_latency(&x, rate, &expected, &c).unwrap();
        assert!((r.latency_ms - lag as f64 / 48.0).abs() < 0.05, "{r:?}");
        assert_eq!(r.detected, 8);
        assert!(r.spread_ms < 0.05);
    }

    #[test]
    fn rejects_silence_and_noise() {
        let rate = 48_000;
        let c = chirp(rate);
        let expected: Vec<f64> = (0..8).map(|k| 14_400.0 + k as f64 * 19_200.0).collect();
        assert!(measure_latency(&vec![0.0; rate as usize * 4], rate, &expected, &c).is_err());
        assert!(measure_latency(&noise(rate as usize * 4, 0.2), rate, &expected, &c).is_err());
    }

    #[test]
    fn monitor_primes_and_drops_excess() {
        let (mut tx, rx) = rtrb::RingBuffer::new(48_000);
        let mut m = Monitor::new(rx, 48_000);
        m.enabled = true;
        let mut out = vec![0.0f32; 512];
        for _ in 0..100 {
            tx.push(0.5).unwrap();
        }
        m.mix_into(&mut out);
        assert!(out.iter().all(|&s| s == 0.0), "no suena antes de acumular el mínimo");
        for _ in 0..(m.max + 1000) {
            tx.push(0.5).unwrap();
        }
        m.mix_into(&mut out);
        assert!(out.iter().all(|&s| s == 0.5));
        assert!(m.consumer.slots() <= m.prime, "descarta el exceso para no acumular latencia");
        m.enabled = false;
        out.fill(0.0);
        m.mix_into(&mut out);
        assert_eq!(m.consumer.slots(), 0);
        assert!(out.iter().all(|&s| s == 0.0));
    }
}
