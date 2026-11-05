//! Lo que comparten la aplicación y el proceso anfitrión de un plugin: el
//! protocolo de control (JSON por líneas en stdin/stdout) y el bloque de memoria
//! compartida del audio, con dos eventos de Windows para pedir y entregar cada
//! bloque.

use std::ptr;

use mn_fx::PluginRef;
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS,
    PAGE_READWRITE,
};
use windows_sys::Win32::System::Threading::{CreateEventW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_ALL_ACCESS};

use crate::{ParamInfo, MAX_BLOCK};

/// Cambios de parámetros o notas por bloque que caben en la memoria compartida.
pub(crate) const MAX_EVENTS: usize = 1024;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct ShParam {
    pub id: u32,
    pub _pad: u32,
    pub value: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct ShNote {
    pub time: u32,
    pub key: u32,
    pub velocity: f32,
    pub on: u32,
}

/// Bloque de audio compartido. Lo escribe la aplicación antes de señalar «pedido»
/// y el anfitrión antes de señalar «hecho»; los eventos ordenan los accesos.
#[repr(C)]
pub(crate) struct Shared {
    pub frames: u32,
    pub n_params: u32,
    pub n_notes: u32,
    /// 1: reiniciar el plugin (vaciar colas) antes de procesar.
    pub reset: u32,
    pub params: [ShParam; MAX_EVENTS],
    pub notes: [ShNote; MAX_EVENTS],
    pub input: [[f32; MAX_BLOCK]; 2],
    pub output: [[f32; MAX_BLOCK]; 2],
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Memoria compartida con nombre.
pub(crate) struct Shm {
    handle: isize,
    ptr: *mut Shared,
}

unsafe impl Send for Shm {}
unsafe impl Sync for Shm {}

impl Shm {
    pub fn create(name: &str) -> Result<Self, String> {
        let size = std::mem::size_of::<Shared>();
        let name_w = wide(name);
        let handle = unsafe { CreateFileMappingW(INVALID_HANDLE_VALUE, ptr::null(), PAGE_READWRITE, 0, size as u32, name_w.as_ptr()) };
        Self::map(handle, name)
    }

    pub fn open(name: &str) -> Result<Self, String> {
        let name_w = wide(name);
        let handle = unsafe { OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, name_w.as_ptr()) };
        Self::map(handle, name)
    }

    fn map(handle: HANDLE, name: &str) -> Result<Self, String> {
        if handle.is_null() {
            return Err(format!("no se pudo crear la memoria compartida {name}"));
        }
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, std::mem::size_of::<Shared>()) };
        if view.Value.is_null() {
            unsafe { CloseHandle(handle) };
            return Err(format!("no se pudo proyectar la memoria compartida {name}"));
        }
        Ok(Self { handle: handle as isize, ptr: view.Value as *mut Shared })
    }

    pub fn get(&self) -> *mut Shared {
        self.ptr
    }
}

impl Drop for Shm {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.ptr as *mut _ });
            CloseHandle(self.handle as HANDLE);
        }
    }
}

/// Evento de Windows con nombre y reinicio automático.
pub(crate) struct Event(isize);

impl Event {
    pub fn create(name: &str) -> Result<Self, String> {
        let name_w = wide(name);
        let h = unsafe { CreateEventW(ptr::null(), 0, 0, name_w.as_ptr()) };
        if h.is_null() {
            return Err(format!("no se pudo crear el evento {name}"));
        }
        Ok(Self(h as isize))
    }

    pub fn open(name: &str) -> Result<Self, String> {
        let name_w = wide(name);
        let h = unsafe { OpenEventW(EVENT_ALL_ACCESS, 0, name_w.as_ptr()) };
        if h.is_null() {
            return Err(format!("no se pudo abrir el evento {name}"));
        }
        Ok(Self(h as isize))
    }

    pub fn set(&self) {
        unsafe { SetEvent(self.0 as HANDLE) };
    }

    /// Espera hasta `ms` milisegundos; devuelve si se señaló.
    pub fn wait(&self, ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.0 as HANDLE, ms) == WAIT_OBJECT_0 }
    }

    pub fn raw(&self) -> HANDLE {
        self.0 as HANDLE
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0 as HANDLE) };
    }
}

/// Orden de la aplicación al anfitrión. `id` 0: no espera respuesta.
#[derive(Serialize, Deserialize)]
pub(crate) struct Envelope {
    pub id: u64,
    pub req: Request,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum Request {
    /// Carga el plugin (con su estado guardado) y abre la memoria y los eventos.
    Load { plugin: PluginRef, sample_rate: u32, shm: String, ev_req: String, ev_done: String },
    Params,
    SetParam { id: u32, value: f64 },
    ParamFromText { id: u32, text: String },
    SaveState,
    LoadState { data: String },
    OpenEditor,
    CloseEditor,
    Shutdown,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reply {
    Loaded { latency: u32 },
    Params(Vec<ParamInfo>),
    State(Option<String>),
    Bool(bool),
    Value(Option<f64>),
    Done,
}

/// Mensaje del anfitrión a la aplicación.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Message {
    Reply { id: u64, result: Result<Reply, String> },
    /// El usuario cerró la ventana del plugin.
    EditorClosed,
}

/// Prefijo de las líneas del protocolo (el resto de la salida se ignora).
pub(crate) const MARK: &str = "@mn ";
