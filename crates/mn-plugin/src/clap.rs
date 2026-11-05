//! Alojamiento CLAP (API C estable, enlaces de `clap-sys`).

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    clap_event_header, clap_event_midi, clap_event_note, clap_event_param_value, clap_input_events, clap_output_events,
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_VALUE,
};
use clap_sys::ext::audio_ports::{clap_audio_port_info, clap_plugin_audio_ports, CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS};
use clap_sys::ext::gui::{clap_host_gui, clap_plugin_gui, clap_window, clap_window_handle, CLAP_EXT_GUI, CLAP_WINDOW_API_WIN32};
use clap_sys::ext::note_ports::{clap_plugin_note_ports, CLAP_EXT_NOTE_PORTS};
use clap_sys::ext::latency::{clap_host_latency, clap_plugin_latency, CLAP_EXT_LATENCY};
use clap_sys::ext::log::{clap_host_log, clap_log_severity, CLAP_EXT_LOG};
use clap_sys::ext::params::{
    clap_host_params, clap_param_clear_flags, clap_param_info, clap_param_rescan_flags, clap_plugin_params, CLAP_EXT_PARAMS,
    CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_READONLY, CLAP_PARAM_IS_STEPPED,
};
use clap_sys::ext::state::{clap_host_state, clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::ext::thread_check::{clap_host_thread_check, CLAP_EXT_THREAD_CHECK};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::clap_process;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::{clap_version_is_compatible, CLAP_VERSION};

use crate::{EventKind, NoteEvent, is_audio_thread, is_main_thread, on_main_async, Backend, ParamInfo, PluginDescriptor, PluginFormat, MAX_BLOCK};

/// Biblioteca CLAP cargada; se comparte entre instancias del mismo archivo y
/// llama a `deinit` al soltarse.
struct ClapLib {
    entry: *const clap_plugin_entry,
    _lib: libloading::Library,
}

unsafe impl Send for ClapLib {}
unsafe impl Sync for ClapLib {}

impl Drop for ClapLib {
    fn drop(&mut self) {
        unsafe {
            if let Some(deinit) = (*self.entry).deinit {
                deinit();
            }
        }
    }
}

static LIBS: OnceLock<Mutex<HashMap<PathBuf, Weak<ClapLib>>>> = OnceLock::new();

fn open_lib(path: &Path) -> Result<Arc<ClapLib>, String> {
    let mut libs = LIBS.get_or_init(Default::default).lock().unwrap();
    if let Some(l) = libs.get(path).and_then(Weak::upgrade) {
        return Ok(l);
    }
    let lib = crate::load_library(path)?;
    let entry: *const clap_plugin_entry = unsafe {
        *lib.get::<*const clap_plugin_entry>(b"clap_entry\0").map_err(|_| "no es un plugin CLAP (falta clap_entry)".to_string())?
    };
    unsafe {
        if !clap_version_is_compatible((*entry).clap_version) {
            return Err("versión de CLAP incompatible".into());
        }
        let cpath = CString::new(path.to_string_lossy().as_bytes()).unwrap_or_default();
        let init = (*entry).init.ok_or("plugin CLAP sin init")?;
        if !init(cpath.as_ptr()) {
            return Err("el plugin no se pudo inicializar".into());
        }
    }
    let l = Arc::new(ClapLib { entry, _lib: lib });
    libs.insert(path.to_path_buf(), Arc::downgrade(&l));
    Ok(l)
}

unsafe fn factory(lib: &ClapLib) -> Result<&clap_plugin_factory, String> {
    let get = (*lib.entry).get_factory.ok_or("plugin CLAP sin fábrica")?;
    let f = get(CLAP_PLUGIN_FACTORY_ID.as_ptr()) as *const clap_plugin_factory;
    f.as_ref().ok_or_else(|| "el archivo no contiene plugins".into())
}

unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

/// Lista los efectos de un archivo `.clap`.
pub(crate) fn scan(path: &Path) -> Result<Vec<PluginDescriptor>, String> {
    let lib = open_lib(path)?;
    let mut out = Vec::new();
    unsafe {
        let f = factory(&lib)?;
        let count = f.get_plugin_count.map_or(0, |c| c(f));
        for i in 0..count {
            let Some(d) = f.get_plugin_descriptor.and_then(|g| g(f, i).as_ref()) else { continue };
            let mut features = Vec::new();
            if !d.features.is_null() {
                let mut p = d.features;
                while !(*p).is_null() {
                    features.push(cstr(*p));
                    p = p.add(1);
                }
            }
            // Efectos de audio e instrumentos (para pistas de instrumento).
            let instrument = features.iter().any(|f| f == "instrument");
            if !instrument && !features.iter().any(|f| f == "audio-effect") {
                continue;
            }
            out.push(PluginDescriptor {
                format: PluginFormat::Clap,
                path: path.display().to_string(),
                id: cstr(d.id),
                name: cstr(d.name),
                vendor: cstr(d.vendor),
                version: cstr(d.version),
                category: features.join(", "),
                instrument,
            });
        }
    }
    Ok(out)
}

// ---- Anfitrión ---------------------------------------------------------------

/// Datos del anfitrión que ve el plugin (`clap_host.host_data`).
struct HostData {
    host: clap_host,
    plugin: AtomicPtr<clap_plugin>,
    alive: AtomicBool,
    /// Ventana del editor abierta (para atender `request_resize`).
    editor_hwnd: AtomicIsize,
}

unsafe fn host_data<'a>(host: *const clap_host) -> &'a HostData {
    &*((*host).host_data as *const HostData)
}

unsafe extern "C" fn host_get_extension(_host: *const clap_host, id: *const c_char) -> *const c_void {
    let id = CStr::from_ptr(id);
    if id == CLAP_EXT_GUI {
        &HOST_GUI as *const _ as *const c_void
    } else if id == CLAP_EXT_PARAMS {
        &HOST_PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &HOST_STATE as *const _ as *const c_void
    } else if id == CLAP_EXT_LOG {
        &HOST_LOG as *const _ as *const c_void
    } else if id == CLAP_EXT_THREAD_CHECK {
        &HOST_THREAD_CHECK as *const _ as *const c_void
    } else if id == CLAP_EXT_LATENCY {
        &HOST_LATENCY as *const _ as *const c_void
    } else {
        ptr::null()
    }
}

unsafe extern "C" fn host_request_restart(_host: *const clap_host) {}
unsafe extern "C" fn host_request_process(_host: *const clap_host) {}

struct SendPtr<T>(*const T);
unsafe impl<T> Send for SendPtr<T> {}

impl<T> SendPtr<T> {
    /// Acceder por método obliga al closure a capturar la envoltura entera (que es `Send`).
    fn get(&self) -> *const T {
        self.0
    }
}

unsafe extern "C" fn host_request_callback(host: *const clap_host) {
    // El plugin pide que se le llame en el hilo principal.
    let data = SendPtr(host_data(host));
    on_main_async(move || {
        let data = &*data.get();
        if data.alive.load(Ordering::Acquire) {
            let p = data.plugin.load(Ordering::Acquire);
            if let Some(cb) = p.as_ref().and_then(|p| p.on_main_thread) {
                cb(p);
            }
        }
    });
}

unsafe extern "C" fn gui_resize_hints_changed(_host: *const clap_host) {}
unsafe extern "C" fn gui_request_resize(host: *const clap_host, w: u32, h: u32) -> bool {
    #[cfg(windows)]
    {
        let hwnd = host_data(host).editor_hwnd.load(Ordering::Acquire);
        if hwnd != 0 {
            crate::window::resize_client(hwnd, w, h);
            return true;
        }
    }
    let _ = (host, w, h);
    false
}
unsafe extern "C" fn gui_request_show(_host: *const clap_host) -> bool {
    false
}
unsafe extern "C" fn gui_request_hide(_host: *const clap_host) -> bool {
    false
}
unsafe extern "C" fn gui_closed(_host: *const clap_host, _was_destroyed: bool) {}
static HOST_GUI: clap_host_gui = clap_host_gui {
    resize_hints_changed: Some(gui_resize_hints_changed),
    request_resize: Some(gui_request_resize),
    request_show: Some(gui_request_show),
    request_hide: Some(gui_request_hide),
    closed: Some(gui_closed),
};

unsafe extern "C" fn params_rescan(_host: *const clap_host, _flags: clap_param_rescan_flags) {}
unsafe extern "C" fn params_clear(_host: *const clap_host, _id: clap_id, _flags: clap_param_clear_flags) {}
unsafe extern "C" fn params_request_flush(_host: *const clap_host) {}
static HOST_PARAMS: clap_host_params = clap_host_params {
    rescan: Some(params_rescan),
    clear: Some(params_clear),
    request_flush: Some(params_request_flush),
};

unsafe extern "C" fn state_mark_dirty(_host: *const clap_host) {}
static HOST_STATE: clap_host_state = clap_host_state { mark_dirty: Some(state_mark_dirty) };

unsafe extern "C" fn log_log(_host: *const clap_host, severity: clap_log_severity, msg: *const c_char) {
    eprintln!("[clap {severity}] {}", cstr(msg));
}
static HOST_LOG: clap_host_log = clap_host_log { log: Some(log_log) };

unsafe extern "C" fn tc_is_main(_host: *const clap_host) -> bool {
    is_main_thread() && !is_audio_thread()
}
unsafe extern "C" fn tc_is_audio(_host: *const clap_host) -> bool {
    is_audio_thread()
}
static HOST_THREAD_CHECK: clap_host_thread_check =
    clap_host_thread_check { is_main_thread: Some(tc_is_main), is_audio_thread: Some(tc_is_audio) };

unsafe extern "C" fn latency_changed(_host: *const clap_host) {}
static HOST_LATENCY: clap_host_latency = clap_host_latency { changed: Some(latency_changed) };

// ---- Listas de eventos para `process` -----------------------------------------

/// Eventos de entrada del bloque, ordenados por tiempo (parámetros y notas).
struct InEvents<'a>(&'a [*const clap_event_header]);

unsafe extern "C" fn in_size(list: *const clap_input_events) -> u32 {
    let ev = &*((*list).ctx as *const InEvents);
    ev.0.len() as u32
}
unsafe extern "C" fn in_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let ev = &*((*list).ctx as *const InEvents);
    ev.0.get(index as usize).copied().unwrap_or(ptr::null())
}
unsafe extern "C" fn out_try_push(_list: *const clap_output_events, _ev: *const clap_event_header) -> bool {
    true
}

// ---- Instancia ------------------------------------------------------------------

pub(crate) struct ClapPlugin {
    host: Box<HostData>,
    plugin: *const clap_plugin,
    params: Option<&'static clap_plugin_params>,
    state: Option<&'static clap_plugin_state>,
    gui: Option<&'static clap_plugin_gui>,
    in_channels: u32,
    out_channels: u32,
    /// Recibe notas (puerto de notas de entrada).
    instrument: bool,
    processing: AtomicBool,
    gui_open: AtomicBool,
    /// Solo lo toca el hilo de audio.
    audio: UnsafeCell<AudioScratch>,
    _lib: Arc<ClapLib>,
}

struct AudioScratch {
    steady: i64,
    events: Vec<clap_event_param_value>,
    notes: Vec<clap_event_note>,
    /// Rueda de tono, CC y aftertouch, como mensajes MIDI.
    midi: Vec<clap_event_midi>,
    order: Vec<*const clap_event_header>,
    mono: Vec<f32>,
}

unsafe impl Send for ClapPlugin {}
unsafe impl Sync for ClapPlugin {}

unsafe fn ext<T>(plugin: *const clap_plugin, id: &CStr) -> Option<&'static T> {
    let get = (*plugin).get_extension?;
    (get(plugin, id.as_ptr()) as *const T).as_ref()
}

impl ClapPlugin {
    /// Hilo principal.
    pub(crate) fn load(path: &Path, id: &str, sample_rate: u32) -> Result<Self, String> {
        let lib = open_lib(path)?;
        let mut host = Box::new(HostData {
            host: clap_host {
                clap_version: CLAP_VERSION,
                host_data: ptr::null_mut(),
                name: c"MusicNova".as_ptr(),
                vendor: c"MusicNova".as_ptr(),
                url: c"https://github.com/".as_ptr(),
                version: c"0.1.0".as_ptr(),
                get_extension: Some(host_get_extension),
                request_restart: Some(host_request_restart),
                request_process: Some(host_request_process),
                request_callback: Some(host_request_callback),
            },
            plugin: AtomicPtr::new(ptr::null_mut()),
            alive: AtomicBool::new(false),
            editor_hwnd: AtomicIsize::new(0),
        });
        host.host.host_data = &*host as *const HostData as *mut c_void;
        unsafe {
            let f = factory(&lib)?;
            let cid = CString::new(id).map_err(|_| "id de plugin no válido")?;
            let create = f.create_plugin.ok_or("fábrica CLAP sin create_plugin")?;
            let plugin = create(f, &host.host, cid.as_ptr());
            if plugin.is_null() {
                return Err(format!("el archivo no contiene el plugin {id}"));
            }
            host.plugin.store(plugin as *mut _, Ordering::Release);
            host.alive.store(true, Ordering::Release);
            if !(*plugin).init.is_some_and(|init| init(plugin)) {
                host.alive.store(false, Ordering::Release);
                if let Some(d) = (*plugin).destroy {
                    d(plugin);
                }
                return Err("el plugin no se pudo inicializar".into());
            }
            let (in_channels, out_channels) = main_ports(plugin);
            let instrument = ext::<clap_plugin_note_ports>(plugin, CLAP_EXT_NOTE_PORTS)
                .and_then(|n| n.count)
                .is_some_and(|count| count(plugin, true) > 0);
            if out_channels == 0 || (in_channels == 0 && !instrument) {
                host.alive.store(false, Ordering::Release);
                if let Some(d) = (*plugin).destroy {
                    d(plugin);
                }
                return Err("el plugin no es un efecto (no tiene entrada y salida de audio)".into());
            }
            if !(*plugin).activate.is_some_and(|a| a(plugin, sample_rate as f64, 1, MAX_BLOCK as u32)) {
                host.alive.store(false, Ordering::Release);
                if let Some(d) = (*plugin).destroy {
                    d(plugin);
                }
                return Err("el plugin no se pudo activar".into());
            }
            Ok(Self {
                params: ext(plugin, CLAP_EXT_PARAMS),
                state: ext(plugin, CLAP_EXT_STATE),
                gui: ext(plugin, CLAP_EXT_GUI),
                host,
                plugin,
                in_channels,
                out_channels,
                instrument,
                processing: AtomicBool::new(false),
                gui_open: AtomicBool::new(false),
                audio: UnsafeCell::new(AudioScratch {
                    steady: 0,
                    events: Vec::with_capacity(1024),
                    notes: Vec::with_capacity(1024),
                    midi: Vec::with_capacity(1024),
                    order: Vec::with_capacity(3072),
                    mono: vec![0.0; MAX_BLOCK],
                }),
                _lib: lib,
            })
        }
    }
}

/// Canales del puerto principal de entrada y de salida (1 o 2; 0 si no hay).
unsafe fn main_ports(plugin: *const clap_plugin) -> (u32, u32) {
    let Some(ports) = ext::<clap_plugin_audio_ports>(plugin, CLAP_EXT_AUDIO_PORTS) else {
        return (2, 2); // Sin la extensión se asume estéreo.
    };
    let pick = |is_input: bool| -> u32 {
        let n = ports.count.map_or(0, |c| c(plugin, is_input));
        let mut first = 0;
        for i in 0..n {
            let mut info: clap_audio_port_info = std::mem::zeroed();
            if ports.get.is_some_and(|g| g(plugin, i, is_input, &mut info))
                && (info.flags & CLAP_AUDIO_PORT_IS_MAIN != 0 || i == 0) {
                    first = info.channel_count.min(2);
                    if info.flags & CLAP_AUDIO_PORT_IS_MAIN != 0 {
                        break;
                    }
                }
        }
        first
    };
    (pick(true), pick(false))
}

unsafe extern "C" fn ostream_write(stream: *const clap_ostream, buf: *const c_void, size: u64) -> i64 {
    let v = &mut *((*stream).ctx as *mut Vec<u8>);
    v.extend_from_slice(std::slice::from_raw_parts(buf as *const u8, size as usize));
    size as i64
}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

unsafe extern "C" fn istream_read(stream: *const clap_istream, buf: *mut c_void, size: u64) -> i64 {
    let c = &mut *((*stream).ctx as *mut Cursor);
    let n = (size as usize).min(c.data.len() - c.pos);
    ptr::copy_nonoverlapping(c.data[c.pos..].as_ptr(), buf as *mut u8, n);
    c.pos += n;
    n as i64
}

/// Mensaje MIDI (canal 1) de un gesto del teclado.
pub(crate) fn midi_bytes(e: &NoteEvent) -> [u8; 3] {
    let v7 = (e.velocity.clamp(0.0, 1.0) * 127.0).round() as u8;
    match e.kind {
        EventKind::PitchBend => {
            let v = (((e.velocity.clamp(-1.0, 1.0) + 1.0) * 0.5) * 16383.0).round() as u16;
            [0xE0, (v & 0x7F) as u8, (v >> 7) as u8]
        }
        EventKind::Control => [0xB0, e.key.min(127), v7],
        EventKind::Pressure => [0xD0, v7, 0],
        EventKind::Note => [if e.on { 0x90 } else { 0x80 }, e.key.min(127), v7],
    }
}

impl Backend for ClapPlugin {
    fn params(&self) -> Vec<ParamInfo> {
        let Some(p) = self.params else { return Vec::new() };
        let mut out = Vec::new();
        unsafe {
            let n = p.count.map_or(0, |c| c(self.plugin));
            for i in 0..n {
                let mut info: clap_param_info = std::mem::zeroed();
                if !p.get_info.is_some_and(|g| g(self.plugin, i, &mut info)) {
                    continue;
                }
                if info.flags & (CLAP_PARAM_IS_HIDDEN | CLAP_PARAM_IS_READONLY) != 0 {
                    continue;
                }
                let mut value = info.default_value;
                if let Some(g) = p.get_value {
                    g(self.plugin, info.id, &mut value);
                }
                let mut text = [0 as c_char; 64];
                let text = if p.value_to_text.is_some_and(|f| f(self.plugin, info.id, value, text.as_mut_ptr(), 64)) {
                    cstr(text.as_ptr()).trim().to_string()
                } else {
                    format!("{value:.2}")
                };
                out.push(ParamInfo {
                    id: info.id,
                    name: cstr(info.name.as_ptr()),
                    min: info.min_value,
                    max: info.max_value,
                    default: info.default_value,
                    value,
                    stepped: info.flags & CLAP_PARAM_IS_STEPPED != 0,
                    // En CLAP los escalonados toman valores enteros.
                    steps: if info.flags & CLAP_PARAM_IS_STEPPED != 0 {
                        (info.max_value - info.min_value).round().clamp(0.0, u32::MAX as f64) as u32
                    } else {
                        0
                    },
                    text,
                });
            }
        }
        out
    }

    fn param_from_text(&self, id: u32, text: &str) -> Option<f64> {
        let p = self.params?;
        let f = p.text_to_value?;
        let text = CString::new(text).ok()?;
        let mut value = 0.0;
        unsafe { f(self.plugin, id, text.as_ptr(), &mut value) }.then_some(value).filter(|v| v.is_finite())
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        let s = self.state?;
        let mut data: Vec<u8> = Vec::new();
        let stream = clap_ostream { ctx: &mut data as *mut Vec<u8> as *mut c_void, write: Some(ostream_write) };
        unsafe { s.save.is_some_and(|f| f(self.plugin, &stream)) }.then_some(data)
    }

    fn load_state(&self, data: &[u8]) -> bool {
        let Some(s) = self.state else { return false };
        let mut cursor = Cursor { data, pos: 0 };
        let stream = clap_istream { ctx: &mut cursor as *mut Cursor as *mut c_void, read: Some(istream_read) };
        unsafe { s.load.is_some_and(|f| f(self.plugin, &stream)) }
    }

    fn gui_open(&self, parent: isize) -> Result<(u32, u32), String> {
        let g = self.gui.ok_or("este plugin no tiene interfaz propia")?;
        unsafe {
            let api = CLAP_WINDOW_API_WIN32.as_ptr();
            if !g.is_api_supported.is_some_and(|f| f(self.plugin, api, false)) {
                return Err("la interfaz del plugin no admite ventanas incrustadas de Windows".into());
            }
            if !g.create.is_some_and(|f| f(self.plugin, api, false)) {
                return Err("el plugin no pudo crear su interfaz".into());
            }
            self.gui_open.store(true, Ordering::Release);
            let (mut w, mut h) = (640u32, 480u32);
            if let Some(f) = g.get_size {
                f(self.plugin, &mut w, &mut h);
            }
            let window = clap_window { api, specific: clap_window_handle { win32: parent as *mut c_void } };
            if !g.set_parent.is_some_and(|f| f(self.plugin, &window)) {
                self.gui_close();
                return Err("el plugin no pudo colocar su interfaz en la ventana".into());
            }
            self.host.editor_hwnd.store(parent, Ordering::Release);
            if let Some(f) = g.show {
                f(self.plugin);
            }
            Ok((w.max(50), h.max(30)))
        }
    }

    fn gui_close(&self) {
        if self.gui_open.swap(false, Ordering::AcqRel) {
            self.host.editor_hwnd.store(0, Ordering::Release);
            if let Some(f) = self.gui.and_then(|g| g.destroy) {
                unsafe { f(self.plugin) };
            }
        }
    }

    fn gui_can_resize(&self) -> bool {
        self.gui.and_then(|g| g.can_resize).is_some_and(|f| unsafe { f(self.plugin) })
    }

    fn gui_set_size(&self, w: u32, h: u32) {
        let Some(g) = self.gui else { return };
        let (mut w, mut h) = (w, h);
        unsafe {
            if let Some(f) = g.adjust_size {
                f(self.plugin, &mut w, &mut h);
            }
            if let Some(f) = g.set_size {
                f(self.plugin, w, h);
            }
        }
    }

    unsafe fn process(&self, input: [&[f32]; 2], output: [&mut [f32]; 2], params: &[(u32, f64)], notes: &[NoteEvent]) {
        let s = &mut *self.audio.get();
        let n = input[0].len();
        if !self.processing.load(Ordering::Relaxed) {
            let ok = (*self.plugin).start_processing.is_some_and(|f| f(self.plugin));
            self.processing.store(ok, Ordering::Relaxed);
            if !ok {
                if self.in_channels == 0 {
                    output[0].fill(0.0);
                    output[1].fill(0.0);
                } else {
                    output[0].copy_from_slice(input[0]);
                    output[1].copy_from_slice(input[1]);
                }
                return;
            }
        }
        s.events.clear();
        for &(id, value) in params.iter().take(s.events.capacity()) {
            s.events.push(clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: 0,
                },
                param_id: id,
                cookie: ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value,
            });
        }
        s.notes.clear();
        s.midi.clear();
        if self.instrument {
            for e in notes.iter().filter(|e| e.kind != EventKind::Note).take(s.midi.capacity()) {
                s.midi.push(clap_event_midi {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_midi>() as u32,
                        time: e.time.min(n.saturating_sub(1) as u32),
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: CLAP_EVENT_MIDI,
                        flags: 0,
                    },
                    port_index: 0,
                    data: midi_bytes(e),
                });
            }
            for e in notes.iter().filter(|e| e.kind == EventKind::Note).take(s.notes.capacity()) {
                s.notes.push(clap_event_note {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_note>() as u32,
                        time: e.time.min(n.saturating_sub(1) as u32),
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: if e.on { CLAP_EVENT_NOTE_ON } else { CLAP_EVENT_NOTE_OFF },
                        flags: 0,
                    },
                    note_id: -1,
                    port_index: 0,
                    channel: 0,
                    key: e.key as i16,
                    velocity: e.velocity.clamp(0.0, 1.0) as f64,
                });
            }
        }
        // Lista única ordenada por tiempo, como exige CLAP.
        s.order.clear();
        s.order.extend(s.events.iter().map(|e| &e.header as *const clap_event_header));
        s.order.extend(s.notes.iter().map(|e| &e.header as *const clap_event_header));
        s.order.extend(s.midi.iter().map(|e| &e.header as *const clap_event_header));
        s.order.sort_by_key(|&h| (*h).time);
        let in_events_data = InEvents(&s.order);
        let in_events = clap_input_events {
            ctx: &in_events_data as *const InEvents as *mut c_void,
            size: Some(in_size),
            get: Some(in_get),
        };
        let out_events = clap_output_events { ctx: ptr::null_mut(), try_push: Some(out_try_push) };

        // Entradas mono: se mezclan los dos canales.
        let mut in_ptrs: [*mut f32; 2] = [input[0].as_ptr() as *mut f32, input[1].as_ptr() as *mut f32];
        if self.in_channels == 1 {
            for i in 0..n {
                s.mono[i] = 0.5 * (input[0][i] + input[1][i]);
            }
            in_ptrs[0] = s.mono.as_mut_ptr();
        }
        let [outl, outr] = output;
        let mut out_ptrs: [*mut f32; 2] = [outl.as_mut_ptr(), outr.as_mut_ptr()];
        let audio_in = clap_audio_buffer {
            data32: in_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: self.in_channels,
            latency: 0,
            constant_mask: 0,
        };
        let mut audio_out = clap_audio_buffer {
            data32: out_ptrs.as_mut_ptr(),
            data64: ptr::null_mut(),
            channel_count: self.out_channels,
            latency: 0,
            constant_mask: 0,
        };
        let process = clap_process {
            steady_time: s.steady,
            frames_count: n as u32,
            transport: ptr::null(),
            audio_inputs: if self.in_channels == 0 { ptr::null() } else { &audio_in },
            audio_outputs: &mut audio_out,
            audio_inputs_count: if self.in_channels == 0 { 0 } else { 1 },
            audio_outputs_count: 1,
            in_events: &in_events,
            out_events: &out_events,
        };
        if let Some(f) = (*self.plugin).process {
            f(self.plugin, &process);
        }
        s.steady += n as i64;
        if self.out_channels == 1 {
            outr.copy_from_slice(outl);
        }
    }

    fn reset(&self) {
        // `reset` es del hilo de audio y solo con el procesado en marcha.
        if !self.processing.load(Ordering::Relaxed) {
            return;
        }
        if let Some(f) = unsafe { (*self.plugin).reset } {
            unsafe { f(self.plugin) };
        }
    }

    fn latency(&self) -> u32 {
        unsafe { ext::<clap_plugin_latency>(self.plugin, CLAP_EXT_LATENCY).and_then(|l| l.get).map_or(0, |get| get(self.plugin)) }
    }

    fn shutdown(&self) {
        self.gui_close();
        unsafe {
            if self.processing.swap(false, Ordering::AcqRel) {
                if let Some(f) = (*self.plugin).stop_processing {
                    // El estándar exige el hilo de audio: el motor ya soltó la
                    // instancia, así que un hilo breve marcado como de audio
                    // hace su papel (los plugins lo comprueban con thread-check).
                    let plugin = SendPtr(self.plugin);
                    let _ = std::thread::spawn(move || {
                        crate::IN_AUDIO.with(|a| a.set(true));
                        f(plugin.get());
                    })
                    .join();
                }
            }
            if let Some(f) = (*self.plugin).deactivate {
                f(self.plugin);
            }
            self.host.alive.store(false, Ordering::Release);
            if let Some(f) = (*self.plugin).destroy {
                f(self.plugin);
            }
        }
    }
}
