//! Alojamiento de plugins de audio CLAP y VST3 para MusicNova.
//!
//! Cada formato implementa [`Backend`]; [`Instance`] añade lo común: cola de
//! cambios de parámetros sin bloqueos, ventana del editor y apagado ordenado.
//!
//! Aislamiento: con [`set_host_command`] cada plugin corre en su propio proceso
//! anfitrión ([`host_main`], la aplicación relanzada con `--plugin-host`); la
//! aplicación habla con él mediante `remote` (órdenes por tuberías y audio por
//! memoria compartida). Si un plugin se cae o se cuelga, su efecto pasa el audio
//! sin procesar, se avisa con [`on_plugin_failed`] y la aplicación sigue. Sin
//! anfitrión configurado (pruebas, el propio anfitrión) se cargan en el proceso.
//! El escaneo también se hace en un proceso aparte (`--scan-plugin`).
//!
//! Hilos: las funciones que los estándares reservan al hilo principal (crear,
//! activar, estado, parámetros, interfaz, destruir) se ejecutan en el hilo que
//! registra [`set_main_thread_executor`] (el de la ventana de la aplicación).
//! El proceso de audio corre en el hilo del motor o en el de exportación.

mod clap;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod ipc;
#[cfg(windows)]
mod remote;
mod scan;
mod vst3;
#[cfg(windows)]
mod window;

#[cfg(windows)]
pub use host::{host_main, HOST_FLAG};

/// Sin anfitrión fuera de Windows: nunca es el modo anfitrión.
#[cfg(not(windows))]
pub fn host_main() -> bool {
    false
}

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, ThreadId};

use base64::Engine as _;
use mn_fx::{EffectParams, EffectSlot, Instrument, LiveControl, LiveNote, PluginRef, Processor};
use serde::{Deserialize, Serialize};

pub use scan::{default_folders, find_candidates, scan_file};

/// Frames máximos por llamada a `process` (los bloques mayores se trocean).
pub const MAX_BLOCK: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginFormat {
    Clap,
    Vst3,
}

impl PluginFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clap => "clap",
            Self::Vst3 => "vst3",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "clap" => Some(Self::Clap),
            "vst3" => Some(Self::Vst3),
            _ => None,
        }
    }
}

/// Un plugin encontrado al escanear.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginDescriptor {
    pub format: PluginFormat,
    /// Archivo `.clap` o carpeta/archivo `.vst3`.
    pub path: String,
    /// Identificador dentro del archivo (id CLAP o CID VST3 en hexadecimal).
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// Categorías que declara el plugin ("audio-effect", "Fx|Reverb"...).
    pub category: String,
    /// Instrumento (recibe notas) en vez de efecto.
    #[serde(default)]
    pub instrument: bool,
}

impl PluginDescriptor {
    pub fn to_ref(&self) -> PluginRef {
        PluginRef {
            format: self.format.as_str().into(),
            path: self.path.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            vendor: self.vendor.clone(),
            state: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub value: f64,
    pub stepped: bool,
    /// Número de pasos entre `min` y `max` si es escalonado (0: continuo).
    #[serde(default)]
    pub steps: u32,
    /// Valor actual como lo muestra el plugin ("-6,0 dB").
    pub text: String,
}

/// Qué es un [`NoteEvent`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EventKind {
    /// Nota: `key`, `velocity` (0–1) y `on`.
    #[default]
    Note,
    /// Rueda de tono: `velocity` de -1 a 1.
    PitchBend,
    /// Controlador MIDI: `key` es su número y `velocity` su valor (0–1).
    Control,
    /// Aftertouch de canal: `velocity` de 0 a 1.
    Pressure,
}

impl EventKind {
    pub(crate) fn to_u32(self) -> u32 {
        self as u32
    }
    pub(crate) fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::PitchBend,
            2 => Self::Control,
            3 => Self::Pressure,
            _ => Self::Note,
        }
    }
}

/// Nota (o gesto del teclado) para un instrumento dentro de un bloque.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteEvent {
    /// Frame dentro del bloque.
    pub time: u32,
    pub key: u8,
    /// 0–1.
    pub velocity: f32,
    pub on: bool,
    pub kind: EventKind,
}

/// Lo que cada formato debe ofrecer. Los métodos marcados «principal» solo se
/// llaman desde el hilo principal; `process` y `reset`, desde el de audio.
pub(crate) trait Backend: Send + Sync {
    /// Principal.
    fn params(&self) -> Vec<ParamInfo>;
    /// Principal: valor que se acaba de fijar desde la interfaz genérica (VST3
    /// lo refleja en su controlador; CLAP lo recibe en el siguiente proceso).
    fn param_set_from_host(&self, _id: u32, _value: f64) {}
    /// Principal: valor que corresponde a un texto escrito por el usuario
    /// («-6 dB», «1,5 kHz»), según el propio plugin. `None`: no lo entiende.
    fn param_from_text(&self, _id: u32, _text: &str) -> Option<f64> {
        None
    }
    /// Principal, periódico mientras la interfaz del plugin está abierta: refleja
    /// en ella lo que cambió en el audio (automatización).
    fn idle(&self) {}
    /// Principal.
    fn save_state(&self) -> Option<Vec<u8>>;
    /// Principal.
    fn load_state(&self, data: &[u8]) -> bool;
    /// Principal: crea la interfaz dentro de `parent` (HWND) y devuelve su tamaño.
    fn gui_open(&self, parent: isize) -> Result<(u32, u32), String>;
    fn gui_close(&self);
    fn gui_can_resize(&self) -> bool;
    fn gui_set_size(&self, w: u32, h: u32);
    /// Audio: procesa `frames` (≤ MAX_BLOCK) en estéreo plano, con los cambios
    /// de parámetros y las notas del bloque (instrumentos).
    unsafe fn process(&self, input: [&[f32]; 2], output: [&mut [f32]; 2], params: &[(u32, f64)], notes: &[NoteEvent]);
    /// Audio.
    fn reset(&self);
    /// Principal: latencia que añade el plugin (frames), consultada tras activarlo.
    fn latency(&self) -> u32 {
        0
    }
    /// Principal: detiene, desactiva y destruye el plugin.
    fn shutdown(&self);

    /// `false`: las llamadas «principal» pueden hacerse desde cualquier hilo
    /// (el plugin corre en otro proceso, con su propio hilo principal).
    fn needs_main_thread(&self) -> bool {
        true
    }
    /// Plugin en otro proceso: su interfaz se abre allí (`None`: aquí).
    fn external_editor_open(&self) -> Option<Result<(), String>> {
        None
    }
    fn external_editor_close(&self) {}
    fn external_editor_is_open(&self) -> Option<bool> {
        None
    }
    /// Por qué el plugin ya no funciona (se cayó o se colgó), si es el caso.
    fn failure(&self) -> Option<String> {
        None
    }
    /// Efecto del proyecto al que sirve (para los avisos).
    fn attach_slot(&self, _slot: &str) {}
}

// ---- Hilo principal ---------------------------------------------------------------

type Executor = Box<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

struct MainThread {
    id: ThreadId,
    exec: Executor,
}

static MAIN: OnceLock<MainThread> = OnceLock::new();

thread_local! {
    /// Marca el hilo mientras procesa audio (para `clap.thread-check`).
    static IN_AUDIO: Cell<bool> = const { Cell::new(false) };
}

/// Registra el hilo actual como principal y cómo ejecutar tareas en él.
pub fn set_main_thread_executor(exec: impl Fn(Box<dyn FnOnce() + Send>) + Send + Sync + 'static) {
    let _ = MAIN.set(MainThread { id: thread::current().id(), exec: Box::new(exec) });
}

/// Sin ejecutor registrado (pruebas, escaneo) cualquier hilo hace de principal.
pub fn is_main_thread() -> bool {
    MAIN.get().is_none_or(|m| m.id == thread::current().id())
}

pub(crate) fn is_audio_thread() -> bool {
    IN_AUDIO.with(|f| f.get())
}

/// Ejecuta `f` en el hilo principal y espera su resultado.
pub(crate) fn on_main<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    match MAIN.get() {
        Some(m) if m.id != thread::current().id() => {
            let (tx, rx) = mpsc::channel();
            (m.exec)(Box::new(move || {
                let _ = tx.send(f());
            }));
            rx.recv().expect("el hilo principal no ejecutó la tarea")
        }
        _ => f(),
    }
}

/// Ejecuta `f` en el hilo principal si el plugin lo necesita; si no, aquí.
fn run_for<R: Send + 'static>(b: &Arc<dyn Backend>, f: impl FnOnce() -> R + Send + 'static) -> R {
    if b.needs_main_thread() {
        on_main(f)
    } else {
        f()
    }
}

/// Ejecuta `f` en el hilo principal sin esperar.
pub(crate) fn on_main_async(f: impl FnOnce() + Send + 'static) {
    match MAIN.get() {
        Some(m) if m.id != thread::current().id() => (m.exec)(Box::new(f)),
        _ => f(),
    }
}

// ---- Instancia ---------------------------------------------------------------------

/// Plugin cargado y activado.
pub struct Instance {
    pub plugin: PluginRef,
    pub sample_rate: u32,
    /// Efecto del proyecto al que pertenece (solo instancias del motor).
    pub slot_id: Option<String>,
    /// Latencia que declaró el plugin al activarse (frames).
    pub latency: u32,
    backend: Option<Arc<dyn Backend>>,
    queue_tx: Mutex<rtrb::Producer<(u32, f64)>>,
    queue_rx: Mutex<rtrb::Consumer<(u32, f64)>>,
    #[cfg(windows)]
    editor: Mutex<Option<window::Editor>>,
}

impl Instance {
    /// Carga el plugin, lo activa a `sample_rate` y restaura su estado guardado.
    /// Instancia para procesar sin conexión (exportar, generar).
    pub fn load(plugin: &PluginRef, sample_rate: u32) -> Result<Arc<Instance>, String> {
        Self::load_with(plugin, sample_rate, false)
    }

    /// Como [`Instance::load`]; `realtime` indica que procesará en el motor (en
    /// otro proceso, un bloque que tarda demasiado pasa sin procesar en vez de esperar).
    pub fn load_with(plugin: &PluginRef, sample_rate: u32, realtime: bool) -> Result<Arc<Instance>, String> {
        #[cfg(windows)]
        if let Some(cmd) = HOST.get() {
            let backend: Arc<dyn Backend> = Arc::new(remote::RemoteBackend::spawn(cmd, plugin, sample_rate, realtime)?);
            return Ok(Self::wrap(plugin, sample_rate, backend));
        }
        let _ = realtime;
        Self::load_local(plugin, sample_rate)
    }

    /// Carga el plugin en este proceso.
    pub(crate) fn load_local(plugin: &PluginRef, sample_rate: u32) -> Result<Arc<Instance>, String> {
        let format = PluginFormat::parse(&plugin.format).ok_or_else(|| format!("formato desconocido: {}", plugin.format))?;
        let state = match plugin.state.as_deref() {
            Some(s) => Some(
                base64::engine::general_purpose::STANDARD.decode(s).map_err(|e| format!("estado del plugin ilegible: {e}"))?,
            ),
            None => None,
        };
        let (path, id) = (PathBuf::from(&plugin.path), plugin.id.clone());
        let backend: Arc<dyn Backend> = on_main(move || -> Result<Arc<dyn Backend>, String> {
            Ok(match format {
                PluginFormat::Clap => Arc::new(clap::ClapPlugin::load(&path, &id, sample_rate)?),
                PluginFormat::Vst3 => Arc::new(vst3::Vst3Plugin::load(&path, &id, sample_rate)?),
            })
        })?;
        if let Some(data) = state {
            let b = backend.clone();
            if !on_main(move || b.load_state(&data)) {
                eprintln!("[mn-plugin] {} no aceptó su estado guardado", plugin.name);
            }
        }
        Ok(Self::wrap(plugin, sample_rate, backend))
    }

    fn wrap(plugin: &PluginRef, sample_rate: u32, backend: Arc<dyn Backend>) -> Arc<Instance> {
        let latency = {
            let b = backend.clone();
            run_for(&backend, move || b.latency())
        };
        let (tx, rx) = rtrb::RingBuffer::new(1024);
        Arc::new(Instance {
            plugin: plugin.clone(),
            sample_rate,
            slot_id: None,
            latency,
            backend: Some(backend),
            queue_tx: Mutex::new(tx),
            queue_rx: Mutex::new(rx),
            #[cfg(windows)]
            editor: Mutex::new(None),
        })
    }

    fn backend(&self) -> Arc<dyn Backend> {
        self.backend.clone().expect("instancia apagada")
    }

    /// Ejecuta `f` con el plugin en el hilo que le corresponde.
    fn call<R: Send + 'static>(&self, f: impl FnOnce(&dyn Backend) -> R + Send + 'static) -> R {
        let b = self.backend();
        let b2 = b.clone();
        run_for(&b, move || f(&*b2))
    }

    /// Por qué el plugin ya no funciona (se cayó o dejó de responder). Su efecto
    /// pasa el audio sin procesar; al reconstruir la cadena se vuelve a cargar.
    pub fn failure(&self) -> Option<String> {
        self.backend.as_ref().and_then(|b| b.failure())
    }

    /// Si corre en su propio proceso.
    pub fn isolated(&self) -> bool {
        self.backend.as_ref().is_some_and(|b| !b.needs_main_thread())
    }

    fn same_plugin(&self, r: &PluginRef) -> bool {
        self.plugin.format == r.format && self.plugin.path == r.path && self.plugin.id == r.id
    }

    pub fn params(&self) -> Vec<ParamInfo> {
        self.call(|b| b.params())
    }

    /// Convierte un texto («-6 dB») en el valor del parámetro, con las reglas del
    /// propio plugin (CLAP `text_to_value`, VST3 `getParamValueByString`).
    pub fn param_from_text(&self, id: u32, text: &str) -> Option<f64> {
        let text = text.to_string();
        self.call(move |b| b.param_from_text(id, &text))
    }

    /// Cambia un parámetro; llega al plugin en el siguiente bloque de audio.
    pub fn set_param(&self, id: u32, value: f64) {
        self.call(move |b| b.param_set_from_host(id, value));
        let _ = self.queue_tx.lock().unwrap().push((id, value));
    }

    /// Estado actual del plugin (base64), para guardarlo en el proyecto.
    pub fn state_base64(&self) -> Option<String> {
        self.call(|b| b.save_state()).map(|d| base64::engine::general_purpose::STANDARD.encode(d))
    }

    /// Abre la interfaz nativa en su propia ventana (o la trae al frente).
    pub fn open_editor(self: &Arc<Self>) -> Result<(), String> {
        if let Some(e) = self.failure() {
            return Err(e);
        }
        if let Some(r) = self.backend().external_editor_open() {
            return r;
        }
        #[cfg(windows)]
        {
            let me = self.clone();
            on_main(move || window::open(&me))
        }
        #[cfg(not(windows))]
        Err("la interfaz de los plugins solo está disponible en Windows por ahora".into())
    }

    pub fn close_editor(&self) {
        if let Some(b) = self.backend.as_ref().filter(|b| !b.needs_main_thread()) {
            b.external_editor_close();
            return;
        }
        #[cfg(windows)]
        if let Some(e) = self.editor.lock().unwrap().take() {
            let b = self.backend();
            on_main(move || {
                b.gui_close();
                drop(e);
            });
        }
    }

    pub fn editor_open(&self) -> bool {
        if let Some(open) = self.backend.as_ref().and_then(|b| b.external_editor_is_open()) {
            return open;
        }
        #[cfg(windows)]
        return self.editor.lock().unwrap().is_some();
        #[cfg(not(windows))]
        false
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        #[cfg(windows)]
        let editor = self.editor.get_mut().unwrap().take();
        if let Some(b) = self.backend.take() {
            if !b.needs_main_thread() {
                // En otro proceso: se le pide que termine sin bloquear a nadie.
                std::thread::spawn(move || b.shutdown());
                return;
            }
            // Sin esperar: quien suelta la última referencia puede ser un hilo
            // que el principal está esperando.
            on_main_async(move || {
                #[cfg(windows)]
                if let Some(e) = editor {
                    b.gui_close();
                    drop(e);
                }
                b.shutdown();
            });
        }
    }
}

/// Adaptador al procesador de efectos de `mn-fx`: entrelazado ↔ plano, sin
/// reservar memoria al procesar.
pub struct PluginProcessor {
    inst: Arc<Instance>,
    backend: Arc<dyn Backend>,
    inl: Vec<f32>,
    inr: Vec<f32>,
    outl: Vec<f32>,
    outr: Vec<f32>,
    events: Vec<(u32, f64)>,
    /// Cambios de la automatización para el siguiente bloque.
    auto: Vec<(u32, f64)>,
    /// Instrumento en vivo: notas del bloque y teclas pulsadas.
    notes: Vec<NoteEvent>,
    held: [bool; 128],
}

impl PluginProcessor {
    pub fn new(inst: Arc<Instance>) -> Self {
        let backend = inst.backend();
        Self {
            inst,
            backend,
            inl: vec![0.0; MAX_BLOCK],
            inr: vec![0.0; MAX_BLOCK],
            outl: vec![0.0; MAX_BLOCK],
            outr: vec![0.0; MAX_BLOCK],
            events: Vec::with_capacity(1024),
            auto: Vec::with_capacity(1024),
            notes: Vec::with_capacity(1024),
            held: [false; 128],
        }
    }
}

impl Processor for PluginProcessor {
    fn process(&mut self, buf: &mut [f32]) {
        IN_AUDIO.with(|f| f.set(true));
        self.events.clear();
        // Primero la automatización; los cambios del usuario, después, mandan.
        let room = self.events.capacity();
        self.events.extend(self.auto.drain(..).take(room));
        if let Ok(mut rx) = self.inst.queue_rx.try_lock() {
            while self.events.len() < self.events.capacity() {
                match rx.pop() {
                    Ok(e) => self.events.push(e),
                    Err(_) => break,
                }
            }
        }
        for (k, chunk) in buf.chunks_mut(MAX_BLOCK * 2).enumerate() {
            let n = chunk.len() / 2;
            for (i, f) in chunk.as_chunks::<2>().0.iter().enumerate() {
                self.inl[i] = f[0];
                self.inr[i] = f[1];
            }
            let events: &[(u32, f64)] = if k == 0 { &self.events } else { &[] };
            let notes: &[NoteEvent] = if k == 0 { &self.notes } else { &[] };
            unsafe {
                self.backend.process(
                    [&self.inl[..n], &self.inr[..n]],
                    [&mut self.outl[..n], &mut self.outr[..n]],
                    events,
                    notes,
                );
            }
            for (i, f) in chunk.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                f[0] = self.outl[i];
                f[1] = self.outr[i];
            }
        }
        self.notes.clear();
        IN_AUDIO.with(|f| f.set(false));
    }

    fn reset(&mut self) {
        IN_AUDIO.with(|f| f.set(true));
        self.backend.reset();
        IN_AUDIO.with(|f| f.set(false));
    }

    fn set_params(&mut self, params: &EffectParams) -> bool {
        matches!(params, EffectParams::Plugin)
    }

    fn set_plugin_param(&mut self, id: u32, value: f64) {
        if let Some(e) = self.auto.iter_mut().find(|e| e.0 == id) {
            e.1 = value;
        } else if self.auto.len() < self.auto.capacity() {
            self.auto.push((id, value));
        }
    }

    fn latency(&self) -> usize {
        self.inst.latency as usize
    }
}

/// Plugin instrumento tocado en vivo: las notas del bloque van al plugin y su
/// salida (sin entrada de audio) es el sonido del instrumento.
impl Instrument for PluginProcessor {
    fn render(&mut self, notes: &[LiveNote], buf: &mut [f32]) {
        // `notes` puede traer ya las liberaciones de `all_notes_off`; `process` lo vacía.
        let last = (buf.len() / 2).saturating_sub(1) as u32;
        for n in notes.iter().take(self.notes.capacity()) {
            let key = n.key.min(127);
            let on = n.on && n.velocity > 0;
            self.held[key as usize] = on;
            self.notes.push(NoteEvent { time: n.time.min(last), key, velocity: n.velocity as f32 / 127.0, on, kind: EventKind::Note });
        }
        buf.fill(0.0);
        self.process(buf);
    }

    fn all_notes_off(&mut self) {
        for key in 0..128u8 {
            if std::mem::take(&mut self.held[key as usize]) && self.notes.len() < self.notes.capacity() {
                self.notes.push(NoteEvent { time: 0, key, velocity: 0.0, on: false, kind: EventKind::Note });
            }
        }
    }

    /// Rueda de tono, CC y aftertouch: al principio del bloque siguiente.
    fn control(&mut self, c: LiveControl) {
        if self.notes.len() >= self.notes.capacity() {
            return;
        }
        let (kind, key, velocity) = match c {
            LiveControl::PitchBend(b) => (EventKind::PitchBend, 0, b.clamp(-1.0, 1.0)),
            LiveControl::Control { cc, value } => (EventKind::Control, cc.min(127), value.min(127) as f32 / 127.0),
            LiveControl::Pressure(v) => (EventKind::Pressure, 0, v.min(127) as f32 / 127.0),
        };
        self.notes.push(NoteEvent { time: 0, key, velocity, on: false, kind });
    }
}

/// Prefijo de los ids de las instancias de instrumentos en vivo (`inst:<pista>`):
/// así se abre su interfaz como la de un efecto y no las suelta `retain_instances`.
pub const INSTRUMENT_PREFIX: &str = "inst:";

/// Instancia en tiempo real de un plugin instrumento para tocarlo en vivo,
/// registrada con el id `key` (reutiliza la que ya hubiera con el mismo plugin).
pub fn live_instrument(plugin: &PluginRef, sample_rate: u32, key: &str) -> Result<Box<dyn Instrument>, String> {
    if let Some(inst) = instance(key) {
        if inst.same_plugin(plugin) && inst.sample_rate == sample_rate && inst.failure().is_none() {
            return Ok(Box::new(PluginProcessor::new(inst)));
        }
    }
    let mut inst = Instance::load_with(plugin, sample_rate, true)?;
    inst.backend().attach_slot(key);
    Arc::get_mut(&mut inst).expect("instancia recién creada").slot_id = Some(key.to_string());
    if let Some(old) = live().lock().unwrap().insert(key.to_string(), inst.clone()) {
        old.close_editor();
    }
    Ok(Box::new(PluginProcessor::new(inst)))
}

/// Suelta la instancia de un instrumento en vivo (al desarmar o cambiar de instrumento).
pub fn release_instrument(key: &str) {
    if let Some(i) = live().lock().unwrap().remove(key) {
        i.close_editor();
    }
}

// ---- Instancias del motor y fábrica para mn-fx -------------------------------------

static LIVE: OnceLock<Mutex<HashMap<String, Arc<Instance>>>> = OnceLock::new();
static ERRORS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn live() -> &'static Mutex<HashMap<String, Arc<Instance>>> {
    LIVE.get_or_init(Default::default)
}

fn errors() -> &'static Mutex<HashMap<String, String>> {
    ERRORS.get_or_init(Default::default)
}

/// Instancia del motor para el efecto `slot_id`.
/// Nota para `render_instrument`: (inicio s, fin s, tecla MIDI, velocidad 1–127).
pub type RenderNote = (f64, f64, u8, u8);

/// Toca `notes` con un plugin instrumento y devuelve audio estéreo entrelazado
/// a `sample_rate` hasta `length` segundos más `tail` de cola (liberaciones).
/// Usa una instancia propia (como al exportar), con el estado guardado en `plugin`.
pub fn render_instrument(plugin: &PluginRef, notes: &[RenderNote], sample_rate: u32, length: f64, tail: f64) -> Result<Vec<f32>, String> {
    let inst = Instance::load(plugin, sample_rate)?;
    let backend = inst.backend();
    let frames = ((length + tail).max(0.0) * sample_rate as f64).ceil() as usize;
    // Eventos en frames absolutos, ordenados (a igual frame, los «off» antes).
    let mut events: Vec<(usize, NoteEvent)> = Vec::with_capacity(notes.len() * 2);
    for &(start, end, key, vel) in notes {
        let a = (start.max(0.0) * sample_rate as f64).round() as usize;
        let b = ((end.max(start) * sample_rate as f64).round() as usize).max(a + 1);
        events.push((a, NoteEvent { time: 0, key: key.min(127), velocity: vel.clamp(1, 127) as f32 / 127.0, on: true, kind: EventKind::Note }));
        events.push((b, NoteEvent { time: 0, key: key.min(127), velocity: 0.0, on: false, kind: EventKind::Note }));
    }
    events.sort_by_key(|(f, e)| (*f, e.on));
    let silence = vec![0.0f32; MAX_BLOCK];
    let (mut outl, mut outr) = (vec![0.0f32; MAX_BLOCK], vec![0.0f32; MAX_BLOCK]);
    let mut block_notes: Vec<NoteEvent> = Vec::with_capacity(1024);
    let mut out = vec![0.0f32; frames * 2];
    let mut next = 0;
    let mut pos = 0;
    IN_AUDIO.with(|f| f.set(true));
    while pos < frames {
        let n = MAX_BLOCK.min(frames - pos);
        block_notes.clear();
        while next < events.len() && events[next].0 < pos + n && block_notes.len() < block_notes.capacity() {
            let (f, e) = events[next];
            block_notes.push(NoteEvent { time: (f.max(pos) - pos) as u32, ..e });
            next += 1;
        }
        unsafe {
            backend.process([&silence[..n], &silence[..n]], [&mut outl[..n], &mut outr[..n]], &[], &block_notes);
        }
        for i in 0..n {
            out[(pos + i) * 2] = outl[i];
            out[(pos + i) * 2 + 1] = outr[i];
        }
        pos += n;
    }
    IN_AUDIO.with(|f| f.set(false));
    drop(backend);
    if let Some(e) = inst.failure() {
        return Err(e);
    }
    drop(inst);
    Ok(out)
}

pub fn instance(slot_id: &str) -> Option<Arc<Instance>> {
    live().lock().unwrap().get(slot_id).cloned()
}

/// Error de carga del efecto `slot_id` (el efecto suena sin procesar).
pub fn load_error(slot_id: &str) -> Option<String> {
    errors().lock().unwrap().get(slot_id).cloned()
}

/// Suelta las instancias del motor cuyo efecto ya no existe.
pub fn retain_instances(keep: impl Fn(&str) -> bool) {
    let removed: Vec<Arc<Instance>> = {
        let mut map = live().lock().unwrap();
        let gone: Vec<String> = map.keys().filter(|k| !k.starts_with(INSTRUMENT_PREFIX) && !keep(k)).cloned().collect();
        gone.iter().filter_map(|k| map.remove(k)).collect()
    };
    for i in removed {
        i.close_editor();
    }
    errors().lock().unwrap().retain(|k, _| keep(k));
}

/// Copia en los efectos el estado actual de sus plugins. Devuelve si cambió algo.
pub fn sync_states(slots: &mut [EffectSlot]) -> bool {
    let mut changed = false;
    for s in slots.iter_mut() {
        let (Some(r), Some(inst)) = (s.plugin.as_mut(), instance(&s.id)) else { continue };
        if !inst.same_plugin(r) {
            continue;
        }
        if let Some(state) = inst.state_base64() {
            if r.state.as_deref() != Some(state.as_str()) {
                r.state = Some(state);
                changed = true;
            }
        }
    }
    changed
}

fn factory(slot: &EffectSlot, sample_rate: u32, realtime: bool) -> Option<Box<dyn Processor>> {
    let r = slot.plugin.as_ref()?;
    if realtime {
        if let Some(inst) = instance(&slot.id) {
            // Un plugin caído se vuelve a cargar (con el último estado guardado).
            if inst.same_plugin(r) && inst.sample_rate == sample_rate && inst.failure().is_none() {
                return Some(Box::new(PluginProcessor::new(inst)));
            }
        }
    }
    match Instance::load_with(r, sample_rate, realtime) {
        Ok(mut inst) => {
            inst.backend().attach_slot(&slot.id);
            if realtime {
                Arc::get_mut(&mut inst).expect("instancia recién creada").slot_id = Some(slot.id.clone());
                if let Some(old) = live().lock().unwrap().insert(slot.id.clone(), inst.clone()) {
                    old.close_editor();
                }
                errors().lock().unwrap().remove(&slot.id);
            }
            Some(Box::new(PluginProcessor::new(inst)))
        }
        Err(e) => {
            eprintln!("[mn-plugin] {}: {e}", r.name);
            errors().lock().unwrap().insert(slot.id.clone(), e);
            None
        }
    }
}

/// Conecta el alojamiento con las cadenas de efectos (llamar una vez al arrancar).
pub fn install() {
    mn_fx::set_plugin_factory(Box::new(factory));
}

/// Aviso de que el usuario cerró la ventana de un plugin (id del efecto).
type ClosedCallback = Box<dyn Fn(&str) + Send + Sync>;
static EDITOR_CLOSED: OnceLock<ClosedCallback> = OnceLock::new();

pub fn on_editor_closed(f: impl Fn(&str) + Send + Sync + 'static) {
    let _ = EDITOR_CLOSED.set(Box::new(f));
}

pub(crate) fn notify_editor_closed(slot_id: Option<&str>) {
    if let (Some(cb), Some(id)) = (EDITOR_CLOSED.get(), slot_id) {
        cb(id);
    }
}

/// Aviso de que el plugin de un efecto se cayó o dejó de responder (id del
/// efecto y mensaje). Llega desde un hilo propio del alojamiento.
type FailedCallback = Box<dyn Fn(&str, &str) + Send + Sync>;
static PLUGIN_FAILED: OnceLock<FailedCallback> = OnceLock::new();

pub fn on_plugin_failed(f: impl Fn(&str, &str) + Send + Sync + 'static) {
    let _ = PLUGIN_FAILED.set(Box::new(f));
}

#[cfg(windows)]
pub(crate) fn notify_failed(slot_id: &str, message: &str) {
    if let Some(cb) = PLUGIN_FAILED.get() {
        cb(slot_id, message);
    }
}

/// Anfitrión con el que aislar cada plugin en su propio proceso.
#[cfg(windows)]
static HOST: OnceLock<remote::HostCommand> = OnceLock::new();

/// Activa el aislamiento: cada plugin se cargará en un proceso lanzado como
/// `exe args…` (que debe llamar a [`host_main`]). Llamar una vez, al arrancar.
pub fn set_host_command(exe: PathBuf, args: Vec<String>) {
    #[cfg(windows)]
    let _ = HOST.set(remote::HostCommand { exe, args });
    #[cfg(not(windows))]
    let _ = (exe, args);
}

/// Si los plugins se cargan en procesos aparte.
pub fn isolation_enabled() -> bool {
    #[cfg(windows)]
    return HOST.get().is_some();
    #[cfg(not(windows))]
    false
}

/// Ventana de la aplicación que hace de dueña de las ventanas de plugins.
static OWNER: OnceLock<isize> = OnceLock::new();

pub fn set_owner_window(hwnd: isize) {
    let _ = OWNER.set(hwnd);
}

pub(crate) fn owner_window() -> isize {
    OWNER.get().copied().unwrap_or(0)
}

/// Biblioteca dinámica con la ruta para los mensajes de error.
pub(crate) fn load_library(path: &Path) -> Result<libloading::Library, String> {
    unsafe { libloading::Library::new(path) }.map_err(|e| format!("no se pudo cargar {}: {e}", path.display()))
}
