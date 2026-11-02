//! Alojamiento de plugins de audio CLAP y VST3 para MusicNova.
//!
//! Los plugins se cargan en el propio proceso (como en la mayoría de DAW); el
//! escaneo, que es lo más arriesgado, se hace en un proceso aparte (la
//! aplicación se relanza con `--scan-plugin`). Cada formato implementa
//! [`Backend`]; [`Instance`] añade lo común: cola de cambios de parámetros sin
//! bloqueos, ventana del editor y apagado ordenado.
//!
//! Hilos: las funciones que los estándares reservan al hilo principal (crear,
//! activar, estado, parámetros, interfaz, destruir) se ejecutan en el hilo que
//! registra [`set_main_thread_executor`] (el de la ventana de la aplicación).
//! El proceso de audio corre en el hilo del motor o en el de exportación.

mod clap;
mod scan;
mod vst3;
#[cfg(windows)]
mod window;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, ThreadId};

use base64::Engine as _;
use mn_fx::{EffectParams, EffectSlot, PluginRef, Processor};
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

#[derive(Clone, Debug, Serialize)]
pub struct ParamInfo {
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub value: f64,
    pub stepped: bool,
    /// Valor actual como lo muestra el plugin ("-6,0 dB").
    pub text: String,
}

/// Lo que cada formato debe ofrecer. Los métodos marcados «principal» solo se
/// llaman desde el hilo principal; `process` y `reset`, desde el de audio.
pub(crate) trait Backend: Send + Sync {
    /// Principal.
    fn params(&self) -> Vec<ParamInfo>;
    /// Principal: valor que se acaba de fijar desde la interfaz genérica (VST3
    /// lo refleja en su controlador; CLAP lo recibe en el siguiente proceso).
    fn param_set_from_host(&self, _id: u32, _value: f64) {}
    /// Principal.
    fn save_state(&self) -> Option<Vec<u8>>;
    /// Principal.
    fn load_state(&self, data: &[u8]) -> bool;
    /// Principal: crea la interfaz dentro de `parent` (HWND) y devuelve su tamaño.
    fn gui_open(&self, parent: isize) -> Result<(u32, u32), String>;
    fn gui_close(&self);
    fn gui_can_resize(&self) -> bool;
    fn gui_set_size(&self, w: u32, h: u32);
    /// Audio: procesa `frames` (≤ MAX_BLOCK) en estéreo plano.
    unsafe fn process(&self, input: [&[f32]; 2], output: [&mut [f32]; 2], params: &[(u32, f64)]);
    /// Audio.
    fn reset(&self);
    /// Principal: detiene, desactiva y destruye el plugin.
    fn shutdown(&self);
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
    backend: Option<Arc<dyn Backend>>,
    queue_tx: Mutex<rtrb::Producer<(u32, f64)>>,
    queue_rx: Mutex<rtrb::Consumer<(u32, f64)>>,
    #[cfg(windows)]
    editor: Mutex<Option<window::Editor>>,
}

impl Instance {
    /// Carga el plugin, lo activa a `sample_rate` y restaura su estado guardado.
    pub fn load(plugin: &PluginRef, sample_rate: u32) -> Result<Arc<Instance>, String> {
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
        let (tx, rx) = rtrb::RingBuffer::new(1024);
        Ok(Arc::new(Instance {
            plugin: plugin.clone(),
            sample_rate,
            slot_id: None,
            backend: Some(backend),
            queue_tx: Mutex::new(tx),
            queue_rx: Mutex::new(rx),
            #[cfg(windows)]
            editor: Mutex::new(None),
        }))
    }

    fn backend(&self) -> Arc<dyn Backend> {
        self.backend.clone().expect("instancia apagada")
    }

    fn same_plugin(&self, r: &PluginRef) -> bool {
        self.plugin.format == r.format && self.plugin.path == r.path && self.plugin.id == r.id
    }

    pub fn params(&self) -> Vec<ParamInfo> {
        let b = self.backend();
        on_main(move || b.params())
    }

    /// Cambia un parámetro; llega al plugin en el siguiente bloque de audio.
    pub fn set_param(&self, id: u32, value: f64) {
        let b = self.backend();
        on_main(move || b.param_set_from_host(id, value));
        let _ = self.queue_tx.lock().unwrap().push((id, value));
    }

    /// Estado actual del plugin (base64), para guardarlo en el proyecto.
    pub fn state_base64(&self) -> Option<String> {
        let b = self.backend();
        on_main(move || b.save_state()).map(|d| base64::engine::general_purpose::STANDARD.encode(d))
    }

    /// Abre la interfaz nativa en su propia ventana (o la trae al frente).
    pub fn open_editor(self: &Arc<Self>) -> Result<(), String> {
        #[cfg(windows)]
        {
            let me = self.clone();
            on_main(move || window::open(&me))
        }
        #[cfg(not(windows))]
        Err("la interfaz de los plugins solo está disponible en Windows por ahora".into())
    }

    pub fn close_editor(&self) {
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
        }
    }
}

impl Processor for PluginProcessor {
    fn process(&mut self, buf: &mut [f32]) {
        IN_AUDIO.with(|f| f.set(true));
        self.events.clear();
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
            unsafe {
                self.backend.process(
                    [&self.inl[..n], &self.inr[..n]],
                    [&mut self.outl[..n], &mut self.outr[..n]],
                    events,
                );
            }
            for (i, f) in chunk.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                f[0] = self.outl[i];
                f[1] = self.outr[i];
            }
        }
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
        let gone: Vec<String> = map.keys().filter(|k| !keep(k)).cloned().collect();
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
            if inst.same_plugin(r) && inst.sample_rate == sample_rate {
                return Some(Box::new(PluginProcessor::new(inst)));
            }
        }
    }
    match Instance::load(r, sample_rate) {
        Ok(mut inst) => {
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
