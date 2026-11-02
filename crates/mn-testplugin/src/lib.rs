//! Plugin de prueba para el alojamiento de plugins de MusicNova.
//!
//! Una sola DLL exporta los dos formatos: copiada como `.clap` es un plugin
//! CLAP (ganancia en dB + inversión de fase, con estado e interfaz Win32
//! mínima) y copiada como `.vst3` es un plugin VST3 (ganancia lineal con
//! componente y controlador separados). Solo se usa en las pruebas.

#![allow(clippy::missing_safety_doc)]

mod vst3;

use std::ffi::{c_char, c_void, CStr};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{clap_event_header, clap_event_param_value, clap_input_events, clap_output_events, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE};
use clap_sys::ext::audio_ports::{clap_audio_port_info, clap_plugin_audio_ports, CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS, CLAP_PORT_STEREO};
use clap_sys::ext::gui::{clap_plugin_gui, clap_window, CLAP_EXT_GUI, CLAP_WINDOW_API_WIN32};
use clap_sys::ext::params::{clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS, CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_STEPPED};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::{clap_process, clap_process_status, CLAP_PROCESS_CONTINUE};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;

pub const CLAP_ID: &CStr = c"org.musicnova.test-gain";

struct SyncDesc(clap_plugin_descriptor);
unsafe impl Sync for SyncDesc {}

struct Features([*const c_char; 3]);
unsafe impl Sync for Features {}
static FEATURES: Features = Features([c"audio-effect".as_ptr(), c"utility".as_ptr(), ptr::null()]);

static DESC: SyncDesc = SyncDesc(clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: CLAP_ID.as_ptr(),
    name: c"MusicNova Test Gain".as_ptr(),
    vendor: c"MusicNova".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0.0".as_ptr(),
    description: c"Ganancia de prueba".as_ptr(),
    features: FEATURES.0.as_ptr(),
});

const PARAM_GAIN: clap_id = 0;
const PARAM_INVERT: clap_id = 1;

struct Plugin {
    plugin: clap_plugin,
    gain_db: AtomicU64,
    invert: AtomicBool,
    gui: Mutex<Option<isize>>,
}

impl Plugin {
    unsafe fn from<'a>(p: *const clap_plugin) -> &'a Plugin {
        &*((*p).plugin_data as *const Plugin)
    }

    fn set_param(&self, id: clap_id, value: f64) {
        match id {
            PARAM_GAIN => self.gain_db.store(value.clamp(-60.0, 12.0).to_bits(), Ordering::Relaxed),
            PARAM_INVERT => self.invert.store(value >= 0.5, Ordering::Relaxed),
            _ => {}
        }
    }

    fn gain_db(&self) -> f64 {
        f64::from_bits(self.gain_db.load(Ordering::Relaxed))
    }

    unsafe fn read_events(&self, events: *const clap_input_events) {
        if events.is_null() {
            return;
        }
        let ev = &*events;
        let (Some(size), Some(get)) = (ev.size, ev.get) else { return };
        for i in 0..size(events) {
            let h = get(events, i);
            if !h.is_null() && (*h).space_id == CLAP_CORE_EVENT_SPACE_ID && (*h).type_ == CLAP_EVENT_PARAM_VALUE {
                let e = &*(h as *const clap_event_param_value);
                self.set_param(e.param_id, e.value);
            }
        }
    }
}

unsafe extern "C" fn init(_p: *const clap_plugin) -> bool {
    true
}
unsafe extern "C" fn destroy(p: *const clap_plugin) {
    drop(Box::from_raw((*p).plugin_data as *mut Plugin));
}
unsafe extern "C" fn activate(_p: *const clap_plugin, _sr: f64, _min: u32, _max: u32) -> bool {
    true
}
unsafe extern "C" fn deactivate(_p: *const clap_plugin) {}
unsafe extern "C" fn start_processing(_p: *const clap_plugin) -> bool {
    true
}
unsafe extern "C" fn stop_processing(_p: *const clap_plugin) {}
unsafe extern "C" fn reset(_p: *const clap_plugin) {}

unsafe extern "C" fn process(p: *const clap_plugin, process: *const clap_process) -> clap_process_status {
    let me = Plugin::from(p);
    let pr = &*process;
    me.read_events(pr.in_events);
    let mut g = 10f64.powf(me.gain_db() / 20.0) as f32;
    if me.gain_db() <= -60.0 {
        g = 0.0;
    }
    if me.invert.load(Ordering::Relaxed) {
        g = -g;
    }
    if pr.audio_inputs_count < 1 || pr.audio_outputs_count < 1 {
        return CLAP_PROCESS_CONTINUE;
    }
    let inp: &clap_audio_buffer = &*pr.audio_inputs;
    let out: &clap_audio_buffer = &*pr.audio_outputs;
    let n = pr.frames_count as usize;
    for ch in 0..out.channel_count.min(inp.channel_count) as usize {
        let i = std::slice::from_raw_parts(*inp.data32.add(ch), n);
        let o = std::slice::from_raw_parts_mut(*out.data32.add(ch), n);
        for k in 0..n {
            o[k] = i[k] * g;
        }
    }
    CLAP_PROCESS_CONTINUE
}

// ---- Parámetros -------------------------------------------------------------

fn copy_name(dst: &mut [c_char], s: &str) {
    for (d, b) in dst.iter_mut().zip(s.bytes().chain(std::iter::once(0))) {
        *d = b as c_char;
    }
}

unsafe extern "C" fn params_count(_p: *const clap_plugin) -> u32 {
    2
}
unsafe extern "C" fn params_get_info(_p: *const clap_plugin, index: u32, info: *mut clap_param_info) -> bool {
    let info = &mut *info;
    info.cookie = ptr::null_mut();
    info.module[0] = 0;
    match index {
        0 => {
            info.id = PARAM_GAIN;
            info.flags = CLAP_PARAM_IS_AUTOMATABLE;
            copy_name(&mut info.name, "Ganancia");
            info.min_value = -60.0;
            info.max_value = 12.0;
            info.default_value = 0.0;
            true
        }
        1 => {
            info.id = PARAM_INVERT;
            info.flags = CLAP_PARAM_IS_AUTOMATABLE | CLAP_PARAM_IS_STEPPED;
            copy_name(&mut info.name, "Invertir");
            info.min_value = 0.0;
            info.max_value = 1.0;
            info.default_value = 0.0;
            true
        }
        _ => false,
    }
}
unsafe extern "C" fn params_get_value(p: *const clap_plugin, id: clap_id, out: *mut f64) -> bool {
    let me = Plugin::from(p);
    match id {
        PARAM_GAIN => *out = me.gain_db(),
        PARAM_INVERT => *out = if me.invert.load(Ordering::Relaxed) { 1.0 } else { 0.0 },
        _ => return false,
    }
    true
}
unsafe extern "C" fn params_value_to_text(_p: *const clap_plugin, id: clap_id, value: f64, buf: *mut c_char, cap: u32) -> bool {
    let text = match id {
        PARAM_GAIN => format!("{value:.1} dB"),
        PARAM_INVERT => (if value >= 0.5 { "Sí" } else { "No" }).to_string(),
        _ => return false,
    };
    copy_name(std::slice::from_raw_parts_mut(buf, cap as usize), &text);
    true
}
unsafe extern "C" fn params_text_to_value(_p: *const clap_plugin, _id: clap_id, _t: *const c_char, _v: *mut f64) -> bool {
    false
}
unsafe extern "C" fn params_flush(p: *const clap_plugin, input: *const clap_input_events, _out: *const clap_output_events) {
    Plugin::from(p).read_events(input);
}

static PARAMS: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_get_info),
    get_value: Some(params_get_value),
    value_to_text: Some(params_value_to_text),
    text_to_value: Some(params_text_to_value),
    flush: Some(params_flush),
};

// ---- Estado -------------------------------------------------------------------

unsafe extern "C" fn state_save(p: *const clap_plugin, stream: *const clap_ostream) -> bool {
    let me = Plugin::from(p);
    let mut data = me.gain_db().to_le_bytes().to_vec();
    data.push(me.invert.load(Ordering::Relaxed) as u8);
    let write = (*stream).write.unwrap();
    let mut done = 0;
    while done < data.len() {
        let n = write(stream, data[done..].as_ptr() as *const c_void, (data.len() - done) as u64);
        if n <= 0 {
            return false;
        }
        done += n as usize;
    }
    true
}
unsafe extern "C" fn state_load(p: *const clap_plugin, stream: *const clap_istream) -> bool {
    let me = Plugin::from(p);
    let read = (*stream).read.unwrap();
    let mut data = [0u8; 9];
    let mut done = 0;
    while done < 9 {
        let n = read(stream, data[done..].as_mut_ptr() as *mut c_void, (9 - done) as u64);
        if n <= 0 {
            return false;
        }
        done += n as usize;
    }
    me.set_param(PARAM_GAIN, f64::from_le_bytes(data[..8].try_into().unwrap()));
    me.set_param(PARAM_INVERT, data[8] as f64);
    true
}
static STATE: clap_plugin_state = clap_plugin_state { save: Some(state_save), load: Some(state_load) };

// ---- Puertos de audio -------------------------------------------------------------

unsafe extern "C" fn ports_count(_p: *const clap_plugin, _is_input: bool) -> u32 {
    1
}
unsafe extern "C" fn ports_get(_p: *const clap_plugin, index: u32, _is_input: bool, info: *mut clap_audio_port_info) -> bool {
    if index != 0 {
        return false;
    }
    let info = &mut *info;
    info.id = 0;
    copy_name(&mut info.name, "Principal");
    info.flags = CLAP_AUDIO_PORT_IS_MAIN;
    info.channel_count = 2;
    info.port_type = CLAP_PORT_STEREO.as_ptr();
    info.in_place_pair = 0;
    true
}
static PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports { count: Some(ports_count), get: Some(ports_get) };

// ---- Interfaz (una ventana hija con un texto) ----------------------------------------

pub const GUI_SIZE: (u32, u32) = (320, 120);

unsafe extern "C" fn gui_is_api_supported(_p: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
    !floating && CStr::from_ptr(api) == CLAP_WINDOW_API_WIN32
}
unsafe extern "C" fn gui_get_preferred_api(_p: *const clap_plugin, api: *mut *const c_char, floating: *mut bool) -> bool {
    *api = CLAP_WINDOW_API_WIN32.as_ptr();
    *floating = false;
    true
}
unsafe extern "C" fn gui_create(_p: *const clap_plugin, api: *const c_char, floating: bool) -> bool {
    gui_is_api_supported(_p, api, floating)
}
unsafe extern "C" fn gui_destroy(p: *const clap_plugin) {
    #[cfg(windows)]
    if let Some(h) = Plugin::from(p).gui.lock().unwrap().take() {
        windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(h as _);
    }
}
unsafe extern "C" fn gui_set_scale(_p: *const clap_plugin, _s: f64) -> bool {
    false
}
unsafe extern "C" fn gui_get_size(_p: *const clap_plugin, w: *mut u32, h: *mut u32) -> bool {
    (*w, *h) = GUI_SIZE;
    true
}
unsafe extern "C" fn gui_can_resize(_p: *const clap_plugin) -> bool {
    false
}
unsafe extern "C" fn gui_noop_size(_p: *const clap_plugin, _w: *mut u32, _h: *mut u32) -> bool {
    false
}
unsafe extern "C" fn gui_set_size(_p: *const clap_plugin, _w: u32, _h: u32) -> bool {
    false
}
unsafe extern "C" fn gui_set_parent(p: *const clap_plugin, window: *const clap_window) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WS_CHILD, WS_VISIBLE};
        let me = Plugin::from(p);
        let parent = (*window).specific.win32;
        let text: Vec<u16> = format!("MusicNova Test Gain · {:.1} dB\0", me.gain_db()).encode_utf16().collect();
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        let h = CreateWindowExW(
            0,
            class.as_ptr(),
            text.as_ptr(),
            WS_CHILD | WS_VISIBLE,
            0,
            0,
            GUI_SIZE.0 as i32,
            GUI_SIZE.1 as i32,
            parent as _,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
        );
        if h.is_null() {
            return false;
        }
        *me.gui.lock().unwrap() = Some(h as isize);
        true
    }
    #[cfg(not(windows))]
    false
}
unsafe extern "C" fn gui_set_transient(_p: *const clap_plugin, _w: *const clap_window) -> bool {
    false
}
unsafe extern "C" fn gui_suggest_title(_p: *const clap_plugin, _t: *const c_char) {}
unsafe extern "C" fn gui_show(_p: *const clap_plugin) -> bool {
    true
}
unsafe extern "C" fn gui_hide(_p: *const clap_plugin) -> bool {
    true
}
static GUI: clap_plugin_gui = clap_plugin_gui {
    is_api_supported: Some(gui_is_api_supported),
    get_preferred_api: Some(gui_get_preferred_api),
    create: Some(gui_create),
    destroy: Some(gui_destroy),
    set_scale: Some(gui_set_scale),
    get_size: Some(gui_get_size),
    can_resize: Some(gui_can_resize),
    get_resize_hints: None,
    adjust_size: Some(gui_noop_size),
    set_size: Some(gui_set_size),
    set_parent: Some(gui_set_parent),
    set_transient: Some(gui_set_transient),
    suggest_title: Some(gui_suggest_title),
    show: Some(gui_show),
    hide: Some(gui_hide),
};

unsafe extern "C" fn get_extension(_p: *const clap_plugin, id: *const c_char) -> *const c_void {
    let id = CStr::from_ptr(id);
    if id == CLAP_EXT_PARAMS {
        &PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &STATE as *const _ as *const c_void
    } else if id == CLAP_EXT_AUDIO_PORTS {
        &PORTS as *const _ as *const c_void
    } else if id == CLAP_EXT_GUI {
        &GUI as *const _ as *const c_void
    } else {
        ptr::null()
    }
}
unsafe extern "C" fn on_main_thread(_p: *const clap_plugin) {}

// ---- Fábrica y punto de entrada --------------------------------------------------

unsafe extern "C" fn factory_count(_f: *const clap_plugin_factory) -> u32 {
    1
}
unsafe extern "C" fn factory_descriptor(_f: *const clap_plugin_factory, index: u32) -> *const clap_plugin_descriptor {
    if index == 0 {
        &DESC.0
    } else {
        ptr::null()
    }
}
unsafe extern "C" fn factory_create(_f: *const clap_plugin_factory, _host: *const clap_host, id: *const c_char) -> *const clap_plugin {
    if CStr::from_ptr(id) != CLAP_ID {
        return ptr::null();
    }
    let me = Box::into_raw(Box::new(Plugin {
        plugin: clap_plugin {
            desc: &DESC.0,
            plugin_data: ptr::null_mut(),
            init: Some(init),
            destroy: Some(destroy),
            activate: Some(activate),
            deactivate: Some(deactivate),
            start_processing: Some(start_processing),
            stop_processing: Some(stop_processing),
            reset: Some(reset),
            process: Some(process),
            get_extension: Some(get_extension),
            on_main_thread: Some(on_main_thread),
        },
        gain_db: AtomicU64::new(0f64.to_bits()),
        invert: AtomicBool::new(false),
        gui: Mutex::new(None),
    }));
    (*me).plugin.plugin_data = me as *mut c_void;
    &(*me).plugin
}
static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(factory_count),
    get_plugin_descriptor: Some(factory_descriptor),
    create_plugin: Some(factory_create),
};

unsafe extern "C" fn entry_init(_path: *const c_char) -> bool {
    true
}
unsafe extern "C" fn entry_deinit() {}
unsafe extern "C" fn entry_get_factory(id: *const c_char) -> *const c_void {
    if CStr::from_ptr(id) == CLAP_PLUGIN_FACTORY_ID {
        &FACTORY as *const _ as *const c_void
    } else {
        ptr::null()
    }
}

#[no_mangle]
#[allow(non_upper_case_globals)]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_get_factory),
};

#[allow(dead_code)]
fn _unused(_: clap_event_header) {}
