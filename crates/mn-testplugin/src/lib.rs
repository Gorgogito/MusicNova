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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    clap_event_header, clap_event_midi, clap_event_note, clap_event_param_value, clap_input_events, clap_output_events,
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_VALUE,
};
use clap_sys::ext::note_ports::{clap_note_port_info, clap_plugin_note_ports, CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI};
use clap_sys::ext::audio_ports::{clap_audio_port_info, clap_plugin_audio_ports, CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS, CLAP_PORT_STEREO};
use clap_sys::ext::gui::{clap_plugin_gui, clap_window, CLAP_EXT_GUI, CLAP_WINDOW_API_WIN32};
use clap_sys::ext::latency::{clap_plugin_latency, CLAP_EXT_LATENCY};
use clap_sys::ext::params::{clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS, CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_STEPPED};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::ext::thread_check::{clap_host_thread_check, CLAP_EXT_THREAD_CHECK};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::{clap_process, clap_process_status, CLAP_PROCESS_CONTINUE};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;

pub const CLAP_ID: &CStr = c"org.musicnova.test-gain";
/// Segundo efecto: retrasa la señal `DELAY_FRAMES` y lo declara como latencia
/// (para probar la compensación de latencia del anfitrión).
pub const CLAP_DELAY_ID: &CStr = c"org.musicnova.test-delay";
pub const DELAY_FRAMES: usize = 1000;
/// Tercer plugin: instrumento (puerto de notas, sin entrada de audio) que toca
/// un seno por nota, para probar las pistas de instrumento.
pub const CLAP_SYNTH_ID: &CStr = c"org.musicnova.test-synth";
struct SynthFeatures([*const c_char; 3]);
unsafe impl Sync for SynthFeatures {}
static SYNTH_FEATURES: SynthFeatures = SynthFeatures([c"instrument".as_ptr(), c"synthesizer".as_ptr(), ptr::null()]);
static DESC_SYNTH: SyncDesc = SyncDesc(clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: CLAP_SYNTH_ID.as_ptr(),
    name: c"MusicNova Test Synth".as_ptr(),
    vendor: c"MusicNova".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0.0".as_ptr(),
    description: c"Seno por nota".as_ptr(),
    features: SYNTH_FEATURES.0.as_ptr(),
});
const MAX_VOICES: usize = 16;

/// Cuarto plugin: efecto que falla a propósito, para probar el aislamiento en
/// otro proceso. Ganancia a +12 dB: se cae al procesar; a -60 dB: el audio se
/// cuelga; con «Invertir» activo, guardar el estado cuelga el hilo principal.
pub const CLAP_CRASH_ID: &CStr = c"org.musicnova.test-crash";
static DESC_CRASH: SyncDesc = SyncDesc(clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: CLAP_CRASH_ID.as_ptr(),
    name: c"MusicNova Test Crash".as_ptr(),
    vendor: c"MusicNova".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0.0".as_ptr(),
    description: c"Falla a propósito".as_ptr(),
    features: FEATURES.0.as_ptr(),
});

fn hang_forever() -> ! {
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// Voz del sintetizador de prueba: tecla, fase, nivel y si sigue pulsada.
#[derive(Clone, Copy, Default)]
struct Voice {
    key: i16,
    phase: f32,
    level: f32,
    vel: f32,
    held: bool,
}

/// Incumplimientos de los estándares que el plugin detecta en el anfitrión
/// (función llamada en un hilo indebido, falta de una interfaz obligatoria…).
/// Las pruebas lo leen con `mn_test_violations`.
static VIOLATIONS: AtomicU32 = AtomicU32::new(0);

pub(crate) fn violation(what: &str) {
    eprintln!("[mn-testplugin] el anfitrión incumple el estándar: {what}");
    VIOLATIONS.fetch_add(1, Ordering::SeqCst);
}

#[no_mangle]
pub extern "C" fn mn_test_violations() -> u32 {
    VIOLATIONS.load(Ordering::SeqCst)
}

struct SyncDesc(clap_plugin_descriptor);
unsafe impl Sync for SyncDesc {}

struct Features([*const c_char; 3]);
unsafe impl Sync for Features {}
static FEATURES: Features = Features([c"audio-effect".as_ptr(), c"utility".as_ptr(), ptr::null()]);

static DESC_DELAY: SyncDesc = SyncDesc(clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: CLAP_DELAY_ID.as_ptr(),
    name: c"MusicNova Test Delay".as_ptr(),
    vendor: c"MusicNova".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0.0".as_ptr(),
    description: c"Retardo fijo con latencia declarada".as_ptr(),
    features: FEATURES.0.as_ptr(),
});

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
    host: *const clap_host,
    gain_db: AtomicU64,
    invert: AtomicBool,
    gui: Mutex<Option<isize>>,
    /// Retardo en frames (0 en la ganancia) y su memoria, solo del hilo de audio.
    delay: usize,
    ring: std::cell::UnsafeCell<(Vec<[f32; 2]>, usize)>,
    /// Instrumento: voces, frecuencia de muestreo y rueda de tono en semitonos
    /// (±2, por MIDI) (solo el hilo de audio).
    synth: bool,
    voices: std::cell::UnsafeCell<([Voice; MAX_VOICES], f32, f32)>,
    /// Plugin que falla a propósito (ver `CLAP_CRASH_ID`).
    faulty: bool,
}

impl Plugin {
    unsafe fn from<'a>(p: *const clap_plugin) -> &'a Plugin {
        &*((*p).plugin_data as *const Plugin)
    }

    /// Comprueba con `clap.thread-check` que estamos en el hilo de audio.
    unsafe fn expect_audio_thread(&self, what: &str) {
        let Some(get) = (*self.host).get_extension else { return };
        let tc = get(self.host, CLAP_EXT_THREAD_CHECK.as_ptr()) as *const clap_host_thread_check;
        if let Some(f) = tc.as_ref().and_then(|t| t.is_audio_thread) {
            if !f(self.host) {
                violation(&format!("{what} fuera del hilo de audio"));
            }
        }
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
unsafe extern "C" fn activate(p: *const clap_plugin, sr: f64, _min: u32, _max: u32) -> bool {
    (*Plugin::from(p).voices.get()).1 = sr as f32;
    true
}

/// Instrumento: aplica las notas de cada momento y genera el bloque.
unsafe fn synth_process(me: &Plugin, pr: &clap_process) {
    let n = pr.frames_count as usize;
    let out: &clap_audio_buffer = &*pr.audio_outputs;
    let (voices, sr, bend) = &mut *me.voices.get();
    let sr = if *sr > 0.0 { *sr } else { 48_000.0 };
    let ev = &*pr.in_events;
    let count = ev.size.map_or(0, |s| s(pr.in_events));
    let mut next = 0;
    for k in 0..n {
        while next < count {
            let h = ev.get.unwrap()(pr.in_events, next);
            if (*h).time as usize > k {
                break;
            }
            next += 1;
            if (*h).space_id != CLAP_CORE_EVENT_SPACE_ID {
                continue;
            }
            let t = (*h).type_;
            if t == CLAP_EVENT_MIDI {
                let d = (*(h as *const clap_event_midi)).data;
                if d[0] & 0xF0 == 0xE0 {
                    let v = ((d[2] as i32) << 7 | d[1] as i32) - 8192;
                    *bend = 2.0 * v as f32 / 8192.0;
                }
            }
            if t == CLAP_EVENT_NOTE_ON || t == CLAP_EVENT_NOTE_OFF {
                let e = &*(h as *const clap_event_note);
                if t == CLAP_EVENT_NOTE_ON {
                    if let Some(v) = voices.iter_mut().find(|v| v.level <= 0.0 && !v.held) {
                        *v = Voice { key: e.key, phase: 0.0, level: 1.0, vel: e.velocity as f32, held: true };
                    }
                } else {
                    for v in voices.iter_mut().filter(|v| v.held && v.key == e.key) {
                        v.held = false;
                    }
                }
            }
        }
        let mut s = 0.0f32;
        for v in voices.iter_mut().filter(|v| v.level > 0.0) {
            let f = 440.0 * 2f32.powf((v.key as f32 - 69.0 + *bend) / 12.0);
            v.phase = (v.phase + f / sr).fract();
            s += 0.25 * v.vel * v.level * (std::f32::consts::TAU * v.phase).sin();
            if !v.held {
                // Liberación de unos 10 ms.
                v.level = (v.level - 1.0 / (0.01 * sr)).max(0.0);
            }
        }
        for ch in 0..out.channel_count.min(2) as usize {
            *(*out.data32.add(ch)).add(k) = s;
        }
    }
}
unsafe extern "C" fn deactivate(_p: *const clap_plugin) {}
unsafe extern "C" fn start_processing(_p: *const clap_plugin) -> bool {
    true
}
unsafe extern "C" fn stop_processing(p: *const clap_plugin) {
    Plugin::from(p).expect_audio_thread("stop_processing");
}
unsafe extern "C" fn reset(p: *const clap_plugin) {
    Plugin::from(p).expect_audio_thread("reset");
}

unsafe extern "C" fn process(p: *const clap_plugin, process: *const clap_process) -> clap_process_status {
    let me = Plugin::from(p);
    let pr = &*process;
    if me.synth {
        if pr.audio_inputs_count != 0 {
            violation("el instrumento recibió entradas de audio");
        }
        if pr.audio_outputs_count >= 1 {
            synth_process(me, pr);
        }
        return CLAP_PROCESS_CONTINUE;
    }
    me.read_events(pr.in_events);
    if me.faulty {
        if me.gain_db() >= 12.0 {
            std::process::abort();
        }
        if me.gain_db() <= -60.0 {
            hang_forever();
        }
    }
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
    let channels = out.channel_count.min(inp.channel_count) as usize;
    if me.delay > 0 {
        let (ring, pos) = &mut *me.ring.get();
        for k in 0..n {
            let mut cur = [0.0f32; 2];
            for ch in 0..channels.min(2) {
                cur[ch] = *(*inp.data32.add(ch)).add(k);
            }
            let old = ring[*pos];
            ring[*pos] = cur;
            *pos = (*pos + 1) % ring.len();
            for ch in 0..channels.min(2) {
                *(*out.data32.add(ch)).add(k) = old[ch] * g;
            }
        }
        return CLAP_PROCESS_CONTINUE;
    }
    for ch in 0..channels {
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
unsafe extern "C" fn params_text_to_value(_p: *const clap_plugin, id: clap_id, t: *const c_char, v: *mut f64) -> bool {
    let text = std::ffi::CStr::from_ptr(t).to_string_lossy().trim().to_lowercase();
    let value = match id {
        PARAM_GAIN => text.trim_end_matches("db").trim().replace(',', ".").parse::<f64>().ok().map(|x| x.clamp(-60.0, 12.0)),
        PARAM_INVERT => match text.as_str() {
            "sí" | "si" | "1" => Some(1.0),
            "no" | "0" => Some(0.0),
            _ => None,
        },
        _ => None,
    };
    match value {
        Some(x) => {
            *v = x;
            true
        }
        None => false,
    }
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
    if me.faulty && me.invert.load(Ordering::Relaxed) {
        hang_forever();
    }
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

unsafe extern "C" fn ports_count(p: *const clap_plugin, is_input: bool) -> u32 {
    // El instrumento no tiene entrada de audio.
    if is_input && Plugin::from(p).synth { 0 } else { 1 }
}

unsafe extern "C" fn note_ports_count(p: *const clap_plugin, is_input: bool) -> u32 {
    (is_input && Plugin::from(p).synth) as u32
}
unsafe extern "C" fn note_ports_get(_p: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_note_port_info) -> bool {
    if index != 0 || !is_input {
        return false;
    }
    let info = &mut *info;
    info.id = 0;
    info.supported_dialects = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
    info.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
    copy_name(&mut info.name, "Notas");
    true
}
static NOTE_PORTS: clap_plugin_note_ports = clap_plugin_note_ports { count: Some(note_ports_count), get: Some(note_ports_get) };
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

unsafe extern "C" fn latency_get(p: *const clap_plugin) -> u32 {
    Plugin::from(p).delay as u32
}
static LATENCY: clap_plugin_latency = clap_plugin_latency { get: Some(latency_get) };

unsafe extern "C" fn get_extension(p: *const clap_plugin, id: *const c_char) -> *const c_void {
    if CStr::from_ptr(id) == CLAP_EXT_LATENCY && Plugin::from(p).delay > 0 {
        return &LATENCY as *const _ as *const c_void;
    }
    if CStr::from_ptr(id) == CLAP_EXT_NOTE_PORTS && Plugin::from(p).synth {
        return &NOTE_PORTS as *const _ as *const c_void;
    }
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
    4
}
unsafe extern "C" fn factory_descriptor(_f: *const clap_plugin_factory, index: u32) -> *const clap_plugin_descriptor {
    match index {
        0 => &DESC.0,
        1 => &DESC_DELAY.0,
        2 => &DESC_SYNTH.0,
        3 => &DESC_CRASH.0,
        _ => ptr::null(),
    }
}
unsafe extern "C" fn factory_create(_f: *const clap_plugin_factory, host: *const clap_host, id: *const c_char) -> *const clap_plugin {
    let id = CStr::from_ptr(id);
    let (desc, delay, synth) = if id == CLAP_ID || id == CLAP_CRASH_ID {
        (if id == CLAP_ID { &DESC.0 } else { &DESC_CRASH.0 }, 0, false)
    } else if id == CLAP_DELAY_ID {
        (&DESC_DELAY.0, DELAY_FRAMES, false)
    } else if id == CLAP_SYNTH_ID {
        (&DESC_SYNTH.0, 0, true)
    } else {
        return ptr::null();
    };
    let me = Box::into_raw(Box::new(Plugin {
        plugin: clap_plugin {
            desc,
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
        host,
        gain_db: AtomicU64::new(0f64.to_bits()),
        invert: AtomicBool::new(false),
        gui: Mutex::new(None),
        delay,
        ring: std::cell::UnsafeCell::new((vec![[0.0; 2]; delay.max(1)], 0)),
        synth,
        voices: std::cell::UnsafeCell::new(([Voice::default(); MAX_VOICES], 48_000.0, 0.0)),
        faulty: id == CLAP_CRASH_ID,
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
