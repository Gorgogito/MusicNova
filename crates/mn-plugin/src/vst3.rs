//! Alojamiento VST3 (interfaces COM, enlaces de `vst3`; SDK 3.8 con licencia MIT).
//!
//! El anfitrión aporta: `IHostApplication` (y los `IMessage`/`IAttributeList`
//! que los plugins piden para hablar entre componente y controlador),
//! `IComponentHandler` (las ediciones hechas en la interfaz del plugin se
//! reenvían al procesador), `IBStream` en memoria para el estado,
//! `IPlugFrame` para redimensionar y colas de parámetros preasignadas.

#![allow(non_snake_case)]

use std::cell::{Cell, RefCell, UnsafeCell};
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComRef, ComWrapper, Interface};

use crate::scan::vst3_binary;
use crate::{Backend, NoteEvent, ParamInfo, PluginDescriptor, PluginFormat, MAX_BLOCK};

const AUDIO_MODULE_CLASS: &str = "Audio Module Class";

// ---- Biblioteca y fábrica -------------------------------------------------------

struct Vst3Lib {
    factory: Option<ComPtr<IPluginFactory>>,
    lib: libloading::Library,
}

unsafe impl Send for Vst3Lib {}
unsafe impl Sync for Vst3Lib {}

impl Drop for Vst3Lib {
    fn drop(&mut self) {
        self.factory.take();
        unsafe {
            if let Ok(exit) = self.lib.get::<unsafe extern "system" fn() -> bool>(b"ExitDll\0") {
                exit();
            }
        }
    }
}

static LIBS: OnceLock<Mutex<HashMap<PathBuf, Weak<Vst3Lib>>>> = OnceLock::new();

fn open_lib(path: &Path) -> Result<Arc<Vst3Lib>, String> {
    let binary = vst3_binary(path)?;
    let mut libs = LIBS.get_or_init(Default::default).lock().unwrap();
    if let Some(l) = libs.get(&binary).and_then(Weak::upgrade) {
        return Ok(l);
    }
    let lib = crate::load_library(&binary)?;
    unsafe {
        if let Ok(init) = lib.get::<unsafe extern "system" fn() -> bool>(b"InitDll\0") {
            if !init() {
                return Err("el plugin no se pudo inicializar (InitDll)".into());
            }
        }
        let get = lib
            .get::<unsafe extern "system" fn() -> *mut IPluginFactory>(b"GetPluginFactory\0")
            .map_err(|_| "no es un plugin VST3 (falta GetPluginFactory)".to_string())?;
        let factory = ComPtr::from_raw(get()).ok_or("el plugin no devolvió su fábrica")?;
        let l = Arc::new(Vst3Lib { factory: Some(factory), lib });
        libs.insert(binary, Arc::downgrade(&l));
        Ok(l)
    }
}

fn fixed_str(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn wide_str(buf: &[TChar]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n].iter().map(|&c| c).collect::<Vec<_>>())
}

fn tuid_hex(t: &TUID) -> String {
    t.iter().map(|&b| format!("{:02X}", b as u8)).collect()
}

fn parse_tuid(s: &str) -> Option<TUID> {
    if s.len() != 32 {
        return None;
    }
    let mut out: TUID = [0; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()? as c_char;
    }
    Some(out)
}

fn iid_ptr<I: Interface>() -> FIDString {
    I::IID.as_ptr() as FIDString
}

/// Lista los efectos de un archivo o paquete `.vst3`.
pub(crate) fn scan(path: &Path) -> Result<Vec<PluginDescriptor>, String> {
    let lib = open_lib(path)?;
    let factory = lib.factory.as_ref().unwrap();
    let mut out = Vec::new();
    unsafe {
        let mut finfo: PFactoryInfo = std::mem::zeroed();
        factory.getFactoryInfo(&mut finfo);
        let factory_vendor = fixed_str(&finfo.vendor);
        let f2 = factory.cast::<IPluginFactory2>();
        for i in 0..factory.countClasses() {
            let (cid, category, name, mut vendor, version, sub) = match &f2 {
                Some(f2) => {
                    let mut ci: PClassInfo2 = std::mem::zeroed();
                    if f2.getClassInfo2(i, &mut ci) != kResultOk {
                        continue;
                    }
                    (ci.cid, fixed_str(&ci.category), fixed_str(&ci.name), fixed_str(&ci.vendor), fixed_str(&ci.version), fixed_str(&ci.subCategories))
                }
                None => {
                    let mut ci: PClassInfo = std::mem::zeroed();
                    if factory.getClassInfo(i, &mut ci) != kResultOk {
                        continue;
                    }
                    (ci.cid, fixed_str(&ci.category), fixed_str(&ci.name), String::new(), String::new(), "Fx".to_string())
                }
            };
            if category != AUDIO_MODULE_CLASS {
                continue;
            }
            // Efectos e instrumentos (los instrumentos van en pistas de instrumento).
            let parts: Vec<&str> = sub.split('|').collect();
            let instrument = parts.contains(&"Instrument") && !parts.contains(&"Fx");
            if vendor.is_empty() {
                vendor = factory_vendor.clone();
            }
            out.push(PluginDescriptor {
                format: PluginFormat::Vst3,
                path: path.display().to_string(),
                id: tuid_hex(&cid),
                name,
                vendor,
                version,
                category: sub,
                instrument,
            });
        }
    }
    Ok(out)
}

// ---- Objetos del anfitrión ---------------------------------------------------------

struct HostApp;

impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}

fn same_iid<I: Interface>(t: *const TUID) -> bool {
    !t.is_null() && unsafe { std::slice::from_raw_parts(t as *const u8, 16) } == I::IID
}

impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        let n = &mut *name;
        for (d, s) in n.iter_mut().zip("MusicNova".encode_utf16().chain(std::iter::once(0))) {
            *d = s as TChar;
        }
        kResultOk
    }

    unsafe fn createInstance(&self, cid: *mut TUID, iid: *mut TUID, obj: *mut *mut c_void) -> tresult {
        if same_iid::<IMessage>(cid) || same_iid::<IMessage>(iid) {
            *obj = ComWrapper::new(Message::default()).to_com_ptr::<IMessage>().unwrap().into_raw() as *mut c_void;
            kResultOk
        } else if same_iid::<IAttributeList>(cid) || same_iid::<IAttributeList>(iid) {
            *obj = ComWrapper::new(AttrList::default()).to_com_ptr::<IAttributeList>().unwrap().into_raw() as *mut c_void;
            kResultOk
        } else {
            *obj = ptr::null_mut();
            kNoInterface
        }
    }
}

#[derive(Default)]
struct Message {
    id: RefCell<Option<CString>>,
    attrs: OnceLock<ComWrapper<AttrList>>,
}

impl Class for Message {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for Message {
    unsafe fn getMessageID(&self) -> FIDString {
        self.id.borrow().as_ref().map_or(ptr::null(), |c| c.as_ptr())
    }
    unsafe fn setMessageID(&self, id: FIDString) {
        *self.id.borrow_mut() = (!id.is_null()).then(|| CStr::from_ptr(id).to_owned());
    }
    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        let a = self.attrs.get_or_init(|| ComWrapper::new(AttrList::default()));
        a.as_com_ref::<IAttributeList>().unwrap().as_ptr()
    }
}

enum Attr {
    Int(i64),
    Float(f64),
    Str(Vec<TChar>),
    Bin(Vec<u8>),
}

#[derive(Default)]
struct AttrList {
    map: RefCell<HashMap<String, Attr>>,
}

impl Class for AttrList {
    type Interfaces = (IAttributeList,);
}

unsafe fn attr_key(id: IAttributeList_::AttrID) -> Option<String> {
    (!id.is_null()).then(|| CStr::from_ptr(id).to_string_lossy().into_owned())
}

impl IAttributeListTrait for AttrList {
    unsafe fn setInt(&self, id: IAttributeList_::AttrID, value: int64) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        self.map.borrow_mut().insert(k, Attr::Int(value));
        kResultOk
    }
    unsafe fn getInt(&self, id: IAttributeList_::AttrID, value: *mut int64) -> tresult {
        match attr_key(id).and_then(|k| match self.map.borrow().get(&k) {
            Some(Attr::Int(v)) => Some(*v),
            _ => None,
        }) {
            Some(v) => {
                *value = v;
                kResultOk
            }
            None => kResultFalse,
        }
    }
    unsafe fn setFloat(&self, id: IAttributeList_::AttrID, value: f64) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        self.map.borrow_mut().insert(k, Attr::Float(value));
        kResultOk
    }
    unsafe fn getFloat(&self, id: IAttributeList_::AttrID, value: *mut f64) -> tresult {
        match attr_key(id).and_then(|k| match self.map.borrow().get(&k) {
            Some(Attr::Float(v)) => Some(*v),
            _ => None,
        }) {
            Some(v) => {
                *value = v;
                kResultOk
            }
            None => kResultFalse,
        }
    }
    unsafe fn setString(&self, id: IAttributeList_::AttrID, string: *const TChar) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        let mut v = Vec::new();
        if !string.is_null() {
            let mut p = string;
            while *p != 0 {
                v.push(*p);
                p = p.add(1);
            }
        }
        self.map.borrow_mut().insert(k, Attr::Str(v));
        kResultOk
    }
    unsafe fn getString(&self, id: IAttributeList_::AttrID, string: *mut TChar, size_in_bytes: uint32) -> tresult {
        let map = self.map.borrow();
        let Some(Attr::Str(v)) = attr_key(id).and_then(|k| map.get(&k)) else { return kResultFalse };
        let cap = size_in_bytes as usize / std::mem::size_of::<TChar>();
        if cap == 0 {
            return kResultFalse;
        }
        let n = v.len().min(cap - 1);
        ptr::copy_nonoverlapping(v.as_ptr(), string, n);
        *string.add(n) = 0;
        kResultOk
    }
    unsafe fn setBinary(&self, id: IAttributeList_::AttrID, data: *const c_void, size_in_bytes: uint32) -> tresult {
        let Some(k) = attr_key(id) else { return kInvalidArgument };
        let v = if data.is_null() { Vec::new() } else { std::slice::from_raw_parts(data as *const u8, size_in_bytes as usize).to_vec() };
        self.map.borrow_mut().insert(k, Attr::Bin(v));
        kResultOk
    }
    unsafe fn getBinary(&self, id: IAttributeList_::AttrID, data: *mut *const c_void, size_in_bytes: *mut uint32) -> tresult {
        let map = self.map.borrow();
        let Some(Attr::Bin(v)) = attr_key(id).and_then(|k| map.get(&k)) else { return kResultFalse };
        *data = v.as_ptr() as *const c_void;
        *size_in_bytes = v.len() as uint32;
        kResultOk
    }
}

/// Recibe las ediciones hechas en la interfaz del plugin.
struct Handler {
    edits: Mutex<rtrb::Producer<(u32, f64)>>,
}

impl Class for Handler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for Handler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        let _ = self.edits.lock().unwrap().push((id, value));
        kResultOk
    }
    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }
    unsafe fn restartComponent(&self, _flags: int32) -> tresult {
        kResultOk
    }
}

/// Permite a la interfaz pedir otro tamaño de ventana.
struct PlugFrame {
    hwnd: Arc<AtomicIsize>,
}

impl Class for PlugFrame {
    type Interfaces = (IPlugFrame,);
}

impl IPlugFrameTrait for PlugFrame {
    unsafe fn resizeView(&self, view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
        let (Some(view), Some(r)) = (ComRef::from_raw(view), new_size.as_mut()) else { return kInvalidArgument };
        #[cfg(windows)]
        {
            let hwnd = self.hwnd.load(Ordering::Acquire);
            if hwnd != 0 {
                crate::window::resize_client(hwnd, (r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32);
            }
        }
        view.onSize(r);
        kResultOk
    }
}

/// `IBStream` en memoria.
#[derive(Default)]
struct MemStream {
    data: RefCell<Vec<u8>>,
    pos: Cell<usize>,
}

impl MemStream {
    fn with(data: &[u8]) -> Self {
        Self { data: RefCell::new(data.to_vec()), pos: Cell::new(0) }
    }
}

impl Class for MemStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for MemStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: int32, num_read: *mut int32) -> tresult {
        let data = self.data.borrow();
        let pos = self.pos.get().min(data.len());
        let n = (num_bytes.max(0) as usize).min(data.len() - pos);
        ptr::copy_nonoverlapping(data[pos..].as_ptr(), buffer as *mut u8, n);
        self.pos.set(pos + n);
        if !num_read.is_null() {
            *num_read = n as int32;
        }
        kResultOk
    }
    unsafe fn write(&self, buffer: *mut c_void, num_bytes: int32, num_written: *mut int32) -> tresult {
        let mut data = self.data.borrow_mut();
        let n = num_bytes.max(0) as usize;
        let pos = self.pos.get();
        if data.len() < pos + n {
            data.resize(pos + n, 0);
        }
        ptr::copy_nonoverlapping(buffer as *const u8, data[pos..].as_mut_ptr(), n);
        self.pos.set(pos + n);
        if !num_written.is_null() {
            *num_written = n as int32;
        }
        kResultOk
    }
    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let len = self.data.borrow().len() as i64;
        let base = match mode {
            m if m == IBStream_::IStreamSeekMode_::kIBSeekSet as int32 => 0,
            m if m == IBStream_::IStreamSeekMode_::kIBSeekCur as int32 => self.pos.get() as i64,
            m if m == IBStream_::IStreamSeekMode_::kIBSeekEnd as int32 => len,
            _ => return kInvalidArgument,
        };
        let p = (base + pos).max(0);
        self.pos.set(p as usize);
        if !result.is_null() {
            *result = p;
        }
        kResultOk
    }
    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if !pos.is_null() {
            *pos = self.pos.get() as int64;
        }
        kResultOk
    }
}

fn stream_ptr(s: &ComWrapper<MemStream>) -> *mut IBStream {
    s.as_com_ref::<IBStream>().unwrap().as_ptr()
}

/// Cambio de un parámetro para un bloque. Las de entrada llevan un único
/// punto al principio; en las de salida el plugin añade puntos y se conserva el último.
#[derive(Default)]
struct ParamQueue {
    id: Cell<u32>,
    value: Cell<f64>,
    points: Cell<i32>,
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.get()
    }
    unsafe fn getPointCount(&self) -> int32 {
        self.points.get()
    }
    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        if index < 0 || index >= self.points.get() || offset.is_null() || value.is_null() {
            return kResultFalse;
        }
        *offset = 0;
        *value = self.value.get();
        kResultOk
    }
    unsafe fn addPoint(&self, _offset: int32, value: ParamValue, index: *mut int32) -> tresult {
        self.value.set(value);
        self.points.set(1);
        if !index.is_null() {
            *index = 0;
        }
        kResultOk
    }
}

const MAX_CHANGES: usize = 128;

/// Lista de cambios de parámetros del bloque; las colas se crean una vez.
struct ParamChanges {
    queues: Vec<ComWrapper<ParamQueue>>,
    ptrs: Vec<*mut IParamValueQueue>,
    count: Cell<usize>,
}

impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}

impl ParamChanges {
    fn new() -> Self {
        let queues: Vec<_> = (0..MAX_CHANGES).map(|_| ComWrapper::new(ParamQueue::default())).collect();
        let ptrs = queues.iter().map(|q| q.as_com_ref::<IParamValueQueue>().unwrap().as_ptr()).collect();
        Self { queues, ptrs, count: Cell::new(0) }
    }

    /// Añade o actualiza (el último valor manda). Sin reservas.
    fn set(&self, id: u32, value: f64) {
        if let Some(i) = self.slot(id) {
            self.queues[i].value.set(value);
            self.queues[i].points.set(1);
        }
    }

    /// Índice de la cola de `id`, creándola si cabe.
    fn slot(&self, id: u32) -> Option<usize> {
        let n = self.count.get();
        if let Some(i) = self.queues[..n].iter().position(|q| q.id.get() == id) {
            return Some(i);
        }
        if n == MAX_CHANGES {
            return None;
        }
        self.queues[n].id.set(id);
        self.queues[n].points.set(0);
        self.count.set(n + 1);
        Some(n)
    }

    /// Cambios con algún punto (para las de salida).
    fn each(&self, mut f: impl FnMut(u32, f64)) {
        for q in &self.queues[..self.count.get()] {
            if q.points.get() > 0 {
                f(q.id.get(), q.value.get());
            }
        }
    }
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.count.get() as int32
    }
    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        if index >= 0 && (index as usize) < self.count.get() {
            self.ptrs[index as usize]
        } else {
            ptr::null_mut()
        }
    }
    unsafe fn addParameterData(&self, id: *const ParamID, index: *mut int32) -> *mut IParamValueQueue {
        let Some(id) = id.as_ref() else { return ptr::null_mut() };
        match self.slot(*id) {
            Some(i) => {
                if !index.is_null() {
                    *index = i as int32;
                }
                self.ptrs[i]
            }
            None => ptr::null_mut(),
        }
    }
}

/// Lista de eventos de nota del bloque (reservada una vez; solo el hilo de audio).
struct EventList {
    events: std::cell::UnsafeCell<Vec<Event>>,
}

impl Class for EventList {
    type Interfaces = (IEventList,);
}

impl EventList {
    fn new() -> Self {
        Self { events: std::cell::UnsafeCell::new(Vec::with_capacity(1024)) }
    }

    /// Sustituye el contenido por las notas del bloque, sin reservar memoria.
    unsafe fn fill(&self, notes: &[NoteEvent], frames: usize) {
        let v = &mut *self.events.get();
        v.clear();
        for n in notes.iter().take(v.capacity()) {
            let mut e: Event = std::mem::zeroed();
            e.busIndex = 0;
            e.sampleOffset = n.time.min(frames.saturating_sub(1) as u32) as int32;
            if n.on {
                e.r#type = Event_::EventTypes_::kNoteOnEvent as uint16;
                e.__field0.noteOn = NoteOnEvent { channel: 0, pitch: n.key as int16, tuning: 0.0, velocity: n.velocity.clamp(0.0, 1.0), length: 0, noteId: -1 };
            } else {
                e.r#type = Event_::EventTypes_::kNoteOffEvent as uint16;
                e.__field0.noteOff = NoteOffEvent { channel: 0, pitch: n.key as int16, velocity: 0.0, noteId: -1, tuning: 0.0 };
            }
            v.push(e);
        }
    }
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> int32 {
        (&*self.events.get()).len() as int32
    }
    unsafe fn getEvent(&self, index: int32, e: *mut Event) -> tresult {
        match (&*self.events.get()).get(index.max(0) as usize) {
            Some(ev) if index >= 0 && !e.is_null() => {
                *e = *ev;
                kResultOk
            }
            _ => kResultFalse,
        }
    }
    unsafe fn addEvent(&self, _e: *mut Event) -> tresult {
        // Lista de entrada: el plugin no añade eventos.
        kResultFalse
    }
}

// ---- Instancia --------------------------------------------------------------------

struct Scratch {
    events: ComWrapper<EventList>,
    changes: ComWrapper<ParamChanges>,
    /// Cambios que informa el propio procesador (medidores, parámetros internos).
    out_changes: ComWrapper<ParamChanges>,
    out_tx: rtrb::Producer<(u32, f64)>,
    edits: rtrb::Consumer<(u32, f64)>,
    context: ProcessContext,
    mono: Vec<f32>,
}

// El orden de los campos es el de liberación: la vista, las conexiones, el
// procesador y el controlador se sueltan antes que el componente, y la
// biblioteca al final (algunos plugins avisan o fallan si no).
pub(crate) struct Vst3Plugin {
    view: Mutex<Option<ComPtr<IPlugView>>>,
    /// Puntos de conexión enlazados (componente, controlador) si van por separado.
    connection: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    component: ComPtr<IComponent>,
    /// Cambios de salida del procesador pendientes de reflejar en el controlador.
    out_rx: Mutex<rtrb::Consumer<(u32, f64)>>,
    separate_controller: bool,
    _host: ComWrapper<HostApp>,
    _handler: ComWrapper<Handler>,
    frame: ComWrapper<PlugFrame>,
    editor_hwnd: Arc<AtomicIsize>,
    in_channels: i32,
    out_channels: i32,
    processing: AtomicBool,
    audio: UnsafeCell<Scratch>,
    _lib: Arc<Vst3Lib>,
}

unsafe impl Send for Vst3Plugin {}
unsafe impl Sync for Vst3Plugin {}

/// Canales del primer bus de audio en una dirección (0 si no hay).
unsafe fn bus_channels(component: &ComPtr<IComponent>, dir: BusDirection) -> i32 {
    let audio = MediaTypes_::kAudio as MediaType;
    if component.getBusCount(audio, dir) < 1 {
        return 0;
    }
    let mut info: BusInfo = std::mem::zeroed();
    if component.getBusInfo(audio, dir, 0, &mut info) != kResultOk {
        return 0;
    }
    info.channelCount
}

impl Vst3Plugin {
    /// Hilo principal.
    pub(crate) fn load(path: &Path, id: &str, sample_rate: u32) -> Result<Self, String> {
        let lib = open_lib(path)?;
        let factory = lib.factory.as_ref().unwrap();
        let cid = parse_tuid(id).ok_or("identificador VST3 no válido")?;
        let host = ComWrapper::new(HostApp);
        let host_ptr = host.as_com_ref::<IHostApplication>().unwrap().as_ptr() as *mut FUnknown;
        unsafe {
            let mut obj = ptr::null_mut();
            if factory.createInstance(cid.as_ptr(), iid_ptr::<IComponent>(), &mut obj) != kResultOk {
                return Err("el plugin no pudo crear su componente".into());
            }
            let component = ComPtr::from_raw(obj as *mut IComponent).ok_or("componente nulo")?;
            if component.initialize(host_ptr) != kResultOk {
                return Err("el componente no se pudo inicializar".into());
            }
            let processor = match component.cast::<IAudioProcessor>() {
                Some(p) => p,
                None => {
                    component.terminate();
                    return Err("el plugin no procesa audio".into());
                }
            };

            // Controlador: el mismo objeto o una clase aparte.
            let (controller, separate) = match component.cast::<IEditController>() {
                Some(c) => (Some(c), false),
                None => {
                    let mut ccid: TUID = [0; 16];
                    let mut ctrl = None;
                    if component.getControllerClassId(&mut ccid) == kResultOk {
                        let mut obj = ptr::null_mut();
                        if factory.createInstance(ccid.as_ptr(), iid_ptr::<IEditController>(), &mut obj) == kResultOk {
                            if let Some(c) = ComPtr::from_raw(obj as *mut IEditController) {
                                if c.initialize(host_ptr) == kResultOk {
                                    ctrl = Some(c);
                                }
                            }
                        }
                    }
                    (ctrl, true)
                }
            };
            let connection = if separate {
                match (component.cast::<IConnectionPoint>(), controller.as_ref().and_then(|c| c.cast::<IConnectionPoint>())) {
                    (Some(a), Some(b)) => {
                        a.connect(b.as_ptr());
                        b.connect(a.as_ptr());
                        Some((a, b))
                    }
                    _ => None,
                }
            } else {
                None
            };
            let (edits_tx, edits_rx) = rtrb::RingBuffer::new(1024);
            let handler = ComWrapper::new(Handler { edits: Mutex::new(edits_tx) });
            if let Some(c) = &controller {
                c.setComponentHandler(handler.as_com_ref::<IComponentHandler>().unwrap().as_ptr());
            }

            // Buses: estéreo si lo admite; si no, lo que ofrezca (mono o estéreo).
            let in_dir = BusDirections_::kInput as BusDirection;
            let out_dir = BusDirections_::kOutput as BusDirection;
            let (ic, oc) = (bus_channels(&component, in_dir), bus_channels(&component, out_dir));
            let event_kind = MediaTypes_::kEvent as MediaType;
            let instrument = component.getBusCount(event_kind, in_dir) > 0;
            if oc == 0 || (ic == 0 && !instrument) {
                let me = Self::partial(component, processor, controller, connection, separate, host, handler, edits_rx, lib, 2, 2);
                me.shutdown_inner();
                return Err("el plugin no tiene salida de audio o no es ni efecto ni instrumento".into());
            }
            let (mut in_channels, mut out_channels) = (if ic == 0 { 0 } else { 2 }, 2);
            let n_in = (ic > 0) as int32;
            let mut ins = [SpeakerArr::kStereo];
            let mut outs = [SpeakerArr::kStereo];
            if processor.setBusArrangements(ins.as_mut_ptr(), n_in, outs.as_mut_ptr(), 1) != kResultTrue {
                in_channels = ic.clamp(0, 2);
                out_channels = oc.clamp(1, 2);
                let mut ins = [if in_channels == 1 { SpeakerArr::kMono } else { SpeakerArr::kStereo }];
                let mut outs = [if out_channels == 1 { SpeakerArr::kMono } else { SpeakerArr::kStereo }];
                processor.setBusArrangements(ins.as_mut_ptr(), n_in, outs.as_mut_ptr(), 1);
            }
            let audio = MediaTypes_::kAudio as MediaType;
            for (dir, count) in [(in_dir, component.getBusCount(audio, in_dir)), (out_dir, component.getBusCount(audio, out_dir))] {
                for b in 0..count {
                    component.activateBus(audio, dir, b, (b == 0) as TBool);
                }
            }
            if instrument {
                component.activateBus(event_kind, in_dir, 0, 1);
            }
            let mut setup = ProcessSetup {
                processMode: ProcessModes_::kRealtime as int32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
                maxSamplesPerBlock: MAX_BLOCK as int32,
                sampleRate: sample_rate as f64,
            };
            let mut me = Self::partial(component, processor, controller, connection, separate, host, handler, edits_rx, lib, in_channels, out_channels);
            if me.processor.setupProcessing(&mut setup) != kResultOk {
                me.shutdown_inner();
                return Err("el plugin no admite esta configuración de audio".into());
            }
            me.audio.get_mut().context.sampleRate = sample_rate as f64;
            if me.component.setActive(1) != kResultOk {
                me.shutdown_inner();
                return Err("el plugin no se pudo activar".into());
            }
            // El controlador refleja el estado inicial del componente.
            me.sync_controller();
            Ok(me)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn partial(
        component: ComPtr<IComponent>,
        processor: ComPtr<IAudioProcessor>,
        controller: Option<ComPtr<IEditController>>,
        connection: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
        separate_controller: bool,
        host: ComWrapper<HostApp>,
        handler: ComWrapper<Handler>,
        edits: rtrb::Consumer<(u32, f64)>,
        lib: Arc<Vst3Lib>,
        in_channels: i32,
        out_channels: i32,
    ) -> Self {
        let editor_hwnd = Arc::new(AtomicIsize::new(0));
        let mut context: ProcessContext = unsafe { std::mem::zeroed() };
        context.tempo = 120.0;
        context.timeSigNumerator = 4;
        context.timeSigDenominator = 4;
        let (out_tx, out_rx) = rtrb::RingBuffer::new(1024);
        Self {
            out_rx: Mutex::new(out_rx),
            component,
            processor,
            controller,
            connection,
            separate_controller,
            _host: host,
            _handler: handler,
            frame: ComWrapper::new(PlugFrame { hwnd: editor_hwnd.clone() }),
            view: Mutex::new(None),
            editor_hwnd,
            in_channels,
            out_channels,
            processing: AtomicBool::new(false),
            audio: UnsafeCell::new(Scratch {
                events: ComWrapper::new(EventList::new()),
                changes: ComWrapper::new(ParamChanges::new()),
                out_changes: ComWrapper::new(ParamChanges::new()),
                out_tx,
                edits,
                context,
                mono: vec![0.0; MAX_BLOCK],
            }),
            _lib: lib,
        }
    }

    /// Hilo principal: refleja en el controlador lo que informó el procesador.
    fn drain_output_changes(&self) {
        let Some(c) = &self.controller else { return };
        let Ok(mut rx) = self.out_rx.lock() else { return };
        while let Ok((id, v)) = rx.pop() {
            unsafe { c.setParamNormalized(id, v) };
        }
    }

    /// Pasa el estado del componente al controlador (si van por separado).
    fn sync_controller(&self) {
        let Some(c) = &self.controller else { return };
        if !self.separate_controller {
            return;
        }
        let s = ComWrapper::new(MemStream::default());
        unsafe {
            if self.component.getState(stream_ptr(&s)) == kResultOk {
                s.pos.set(0);
                c.setComponentState(stream_ptr(&s));
            }
        }
    }

    fn shutdown_inner(&self) {
        self.gui_close();
        unsafe {
            if self.processing.swap(false, Ordering::AcqRel) {
                self.processor.setProcessing(0);
            }
            self.component.setActive(0);
            if let Some((a, b)) = &self.connection {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
            if let Some(c) = &self.controller {
                c.setComponentHandler(ptr::null_mut());
                if self.separate_controller {
                    c.terminate();
                }
            }
            self.component.terminate();
        }
    }
}

impl Backend for Vst3Plugin {
    fn params(&self) -> Vec<ParamInfo> {
        self.drain_output_changes();
        let Some(c) = &self.controller else { return Vec::new() };
        let mut out = Vec::new();
        unsafe {
            for i in 0..c.getParameterCount() {
                let mut info: ParameterInfo = std::mem::zeroed();
                if c.getParameterInfo(i, &mut info) != kResultOk {
                    continue;
                }
                use ParameterInfo_::ParameterFlags_::{kIsHidden, kIsReadOnly};
                if info.flags & (kIsHidden | kIsReadOnly) != 0 {
                    continue;
                }
                let value = c.getParamNormalized(info.id);
                let mut text: String128 = [0; 128];
                let text = if c.getParamStringByValue(info.id, value, &mut text) == kResultOk {
                    let units = wide_str(&info.units);
                    let t = wide_str(&text);
                    let t = t.trim();
                    if units.is_empty() { t.to_string() } else { format!("{t} {units}") }
                } else {
                    format!("{value:.3}")
                };
                out.push(ParamInfo {
                    id: info.id,
                    name: wide_str(&info.title),
                    min: 0.0,
                    max: 1.0,
                    default: info.defaultNormalizedValue,
                    value,
                    stepped: info.stepCount > 0,
                    text,
                });
            }
        }
        out
    }

    fn param_set_from_host(&self, id: u32, value: f64) {
        if let Some(c) = &self.controller {
            unsafe { c.setParamNormalized(id, value.clamp(0.0, 1.0)) };
        }
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        self.drain_output_changes();
        // Formato propio: [u32 bytes del componente][componente][controlador].
        let comp = ComWrapper::new(MemStream::default());
        if unsafe { self.component.getState(stream_ptr(&comp)) } != kResultOk {
            return None;
        }
        let comp = comp.data.borrow().clone();
        let ctrl = ComWrapper::new(MemStream::default());
        if let Some(c) = self.controller.as_ref().filter(|_| self.separate_controller) {
            unsafe { c.getState(stream_ptr(&ctrl)) };
        }
        let mut out = (comp.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(&comp);
        out.extend_from_slice(&ctrl.data.borrow());
        Some(out)
    }

    fn load_state(&self, data: &[u8]) -> bool {
        if data.len() < 4 {
            return false;
        }
        let n = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
        if data.len() < 4 + n {
            return false;
        }
        let (comp, ctrl) = (&data[4..4 + n], &data[4 + n..]);
        unsafe {
            let s = ComWrapper::new(MemStream::with(comp));
            if self.component.setState(stream_ptr(&s)) != kResultOk {
                return false;
            }
            if let Some(c) = self.controller.as_ref().filter(|_| self.separate_controller) {
                let s = ComWrapper::new(MemStream::with(comp));
                c.setComponentState(stream_ptr(&s));
                if !ctrl.is_empty() {
                    let s = ComWrapper::new(MemStream::with(ctrl));
                    c.setState(stream_ptr(&s));
                }
            }
        }
        true
    }

    fn gui_open(&self, parent: isize) -> Result<(u32, u32), String> {
        let c = self.controller.as_ref().ok_or("este plugin no tiene interfaz propia")?;
        unsafe {
            let view = ComPtr::from_raw(c.createView(ViewType::kEditor)).ok_or("este plugin no tiene interfaz propia")?;
            if view.isPlatformTypeSupported(kPlatformTypeHWND) != kResultTrue {
                return Err("la interfaz del plugin no admite ventanas de Windows".into());
            }
            view.setFrame(self.frame.as_com_ref::<IPlugFrame>().unwrap().as_ptr());
            self.editor_hwnd.store(parent, Ordering::Release);
            if view.attached(parent as *mut c_void, kPlatformTypeHWND) != kResultOk {
                view.setFrame(ptr::null_mut());
                self.editor_hwnd.store(0, Ordering::Release);
                return Err("el plugin no pudo colocar su interfaz en la ventana".into());
            }
            let mut r: ViewRect = std::mem::zeroed();
            view.getSize(&mut r);
            *self.view.lock().unwrap() = Some(view);
            Ok(((r.right - r.left).max(50) as u32, (r.bottom - r.top).max(30) as u32))
        }
    }

    fn gui_close(&self) {
        if let Some(view) = self.view.lock().unwrap().take() {
            unsafe {
                view.removed();
                view.setFrame(ptr::null_mut());
            }
            self.editor_hwnd.store(0, Ordering::Release);
        }
    }

    fn gui_can_resize(&self) -> bool {
        self.view.lock().unwrap().as_ref().is_some_and(|v| unsafe { v.canResize() } == kResultTrue)
    }

    fn gui_set_size(&self, w: u32, h: u32) {
        if let Some(v) = self.view.lock().unwrap().as_ref() {
            let mut r = ViewRect { left: 0, top: 0, right: w as int32, bottom: h as int32 };
            unsafe {
                v.checkSizeConstraint(&mut r);
                v.onSize(&mut r);
            }
        }
    }

    unsafe fn process(&self, input: [&[f32]; 2], output: [&mut [f32]; 2], params: &[(u32, f64)], notes: &[NoteEvent]) {
        let s = &mut *self.audio.get();
        let n = input[0].len();
        s.events.fill(notes, n);
        if !self.processing.load(Ordering::Relaxed) {
            self.processor.setProcessing(1);
            self.processing.store(true, Ordering::Relaxed);
        }
        s.changes.count.set(0);
        s.out_changes.count.set(0);
        for &(id, v) in params {
            s.changes.set(id, v.clamp(0.0, 1.0));
        }
        // Lo que el usuario mueve en la interfaz del plugin llega al procesador por aquí.
        while let Ok((id, v)) = s.edits.pop() {
            s.changes.set(id, v);
        }
        let mut in_ptrs: [*mut f32; 2] = [input[0].as_ptr() as *mut f32, input[1].as_ptr() as *mut f32];
        if self.in_channels == 1 {
            for i in 0..n {
                s.mono[i] = 0.5 * (input[0][i] + input[1][i]);
            }
            in_ptrs[0] = s.mono.as_mut_ptr();
        }
        let [outl, outr] = output;
        let mut out_ptrs: [*mut f32; 2] = [outl.as_mut_ptr(), outr.as_mut_ptr()];
        let mut inputs = AudioBusBuffers {
            numChannels: self.in_channels,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 { channelBuffers32: in_ptrs.as_mut_ptr() },
        };
        let mut outputs = AudioBusBuffers {
            numChannels: self.out_channels,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 { channelBuffers32: out_ptrs.as_mut_ptr() },
        };
        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as int32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
            numSamples: n as int32,
            numInputs: (self.in_channels > 0) as int32,
            numOutputs: 1,
            inputs: if self.in_channels > 0 { &mut inputs } else { ptr::null_mut() },
            outputs: &mut outputs,
            inputParameterChanges: s.changes.as_com_ref::<IParameterChanges>().unwrap().as_ptr(),
            outputParameterChanges: s.out_changes.as_com_ref::<IParameterChanges>().unwrap().as_ptr(),
            inputEvents: s.events.as_com_ref::<IEventList>().unwrap().as_ptr(),
            outputEvents: ptr::null_mut(),
            processContext: &mut s.context,
        };
        if self.processor.process(&mut data) != kResultOk {
            if self.in_channels > 0 {
                outl.copy_from_slice(input[0]);
                outr.copy_from_slice(input[1]);
            } else {
                outl.fill(0.0);
                outr.fill(0.0);
            }
        } else if self.out_channels == 1 {
            outr.copy_from_slice(outl);
        }
        let tx = &mut s.out_tx;
        s.out_changes.each(|id, v| {
            let _ = tx.push((id, v));
        });
        s.context.projectTimeSamples += n as i64;
        s.context.continousTimeSamples += n as i64;
    }

    fn reset(&self) {
        // VST3 no tiene un «reset» propio: se reinicia el procesado.
        if self.processing.swap(false, Ordering::AcqRel) {
            unsafe { self.processor.setProcessing(0) };
        }
    }

    fn latency(&self) -> u32 {
        unsafe { self.processor.getLatencySamples() }
    }

    fn shutdown(&self) {
        self.shutdown_inner();
    }
}

#[allow(dead_code)]
fn _assert_cstr(_: &CStr) {}
