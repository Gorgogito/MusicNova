//! Proceso anfitrión de un plugin (`--plugin-host`).
//!
//! Aloja un solo plugin en este proceso con el código de siempre ([`Instance`]
//! local): el hilo principal ejecuta un bucle de mensajes de Windows (órdenes de
//! la aplicación y ventana del plugin) y un hilo de audio procesa cada bloque
//! que la aplicación pide por memoria compartida. Si el plugin se cae, solo cae
//! este proceso.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, Write};
use std::os::windows::io::FromRawHandle;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use base64::Engine as _;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL};
use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, PostMessageW, PostQuitMessage, RegisterClassExW,
    TranslateMessage, HWND_MESSAGE, MSG, WM_APP, WNDCLASSEXW,
};

use crate::ipc::{Envelope, Event, Message, Reply, Request, Shm, MARK, MAX_EVENTS};
use crate::{Instance, NoteEvent, IN_AUDIO, MAX_BLOCK};

/// Argumento con el que se lanza el anfitrión.
pub const HOST_FLAG: &str = "--plugin-host";

type Task = Box<dyn FnOnce() + Send>;

static TASKS: Mutex<VecDeque<Task>> = Mutex::new(VecDeque::new());
static MSG_WINDOW: AtomicIsize = AtomicIsize::new(0);
static OUT: OnceLock<Mutex<File>> = OnceLock::new();

/// Plugin cargado y su hilo de audio (solo el hilo principal).
struct Hosted {
    inst: Arc<Instance>,
    stop: Arc<AtomicBool>,
    audio: Option<JoinHandle<()>>,
}

thread_local! {
    static HOSTED: RefCell<Option<Hosted>> = const { RefCell::new(None) };
}

fn send(msg: &Message) {
    let Some(out) = OUT.get() else { return };
    let Ok(json) = serde_json::to_string(msg) else { return };
    let mut f = out.lock().unwrap();
    let _ = writeln!(f, "{MARK}{json}").and_then(|_| f.flush());
}

fn reply(id: u64, result: Result<Reply, String>) {
    if id != 0 {
        send(&Message::Reply { id, result });
    }
}

/// Ejecuta `f` en el hilo principal (por la ventana de mensajes: así también
/// llega durante los bucles modales, como al arrastrar la ventana del plugin).
fn post(f: Task) {
    TASKS.lock().unwrap().push_back(f);
    unsafe { PostMessageW(MSG_WINDOW.load(Ordering::Acquire) as HWND, WM_APP, 0, 0) };
}

unsafe extern "system" fn msg_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_APP {
        loop {
            let task = TASKS.lock().unwrap().pop_front();
            match task {
                Some(t) => t(),
                None => break,
            }
        }
        return 0;
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn create_message_window() -> Result<(), String> {
    let class: Vec<u16> = "MusicNovaPluginHost\0".encode_utf16().collect();
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(msg_wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(ptr::null()),
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class.as_ptr(),
            hIconSm: ptr::null_mut(),
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            ptr::null_mut(),
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        );
        if hwnd.is_null() {
            return Err("no se pudo crear la ventana de mensajes".into());
        }
        MSG_WINDOW.store(hwnd as isize, Ordering::Release);
    }
    Ok(())
}

/// Hilo de audio: espera cada pedido, procesa el bloque y lo entrega.
fn audio_loop(inst: Arc<Instance>, shm: Shm, ev_req: Event, ev_done: Event, stop: Arc<AtomicBool>) {
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
    let backend = inst.backend();
    let mut params: Vec<(u32, f64)> = Vec::with_capacity(MAX_EVENTS);
    let mut notes: Vec<NoteEvent> = Vec::with_capacity(MAX_EVENTS);
    IN_AUDIO.with(|f| f.set(true));
    while !stop.load(Ordering::Relaxed) {
        if !ev_req.wait(200) {
            continue;
        }
        unsafe {
            let sh = shm.get();
            if (*sh).reset != 0 {
                (*sh).reset = 0;
                backend.reset();
            }
            let n = ((*sh).frames as usize).min(MAX_BLOCK);
            params.clear();
            params.extend((&(*sh).params)[..((*sh).n_params as usize).min(MAX_EVENTS)].iter().map(|p| (p.id, p.value)));
            notes.clear();
            notes.extend((&(*sh).notes)[..((*sh).n_notes as usize).min(MAX_EVENTS)].iter().map(|e| NoteEvent {
                time: e.time,
                key: e.key.min(127) as u8,
                velocity: e.velocity,
                on: e.on != 0,
            }));
            let input = &(*sh).input;
            let output = &mut (*sh).output;
            let [ol, or] = output;
            backend.process([&input[0][..n], &input[1][..n]], [&mut ol[..n], &mut or[..n]], &params, &notes);
        }
        ev_done.set();
    }
    IN_AUDIO.with(|f| f.set(false));
}

fn with_inst<R>(f: impl FnOnce(&Arc<Instance>) -> Result<R, String>) -> Result<R, String> {
    HOSTED.with(|h| match h.borrow().as_ref() {
        Some(h) => f(&h.inst),
        None => Err("no hay ningún plugin cargado".into()),
    })
}

/// Hilo principal: atiende una orden.
fn handle(req: Request) -> Result<Reply, String> {
    match req {
        Request::Load { plugin, sample_rate, shm, ev_req, ev_done } => {
            if HOSTED.with(|h| h.borrow().is_some()) {
                return Err("este proceso ya aloja un plugin".into());
            }
            let (shm, ev_req, ev_done) = (Shm::open(&shm)?, Event::open(&ev_req)?, Event::open(&ev_done)?);
            let mut inst = Instance::load_local(&plugin, sample_rate)?;
            // Con un efecto asociado, cerrar la ventana avisa a la aplicación.
            Arc::get_mut(&mut inst).expect("instancia recién creada").slot_id = Some("host".into());
            let latency = inst.latency;
            let stop = Arc::new(AtomicBool::new(false));
            let (i, s) = (inst.clone(), stop.clone());
            let audio = std::thread::Builder::new()
                .name("mn-plugin-audio".into())
                .spawn(move || audio_loop(i, shm, ev_req, ev_done, s))
                .map_err(|e| e.to_string())?;
            HOSTED.with(|h| *h.borrow_mut() = Some(Hosted { inst, stop, audio: Some(audio) }));
            Ok(Reply::Loaded { latency })
        }
        Request::Params => with_inst(|i| Ok(Reply::Params(i.params()))),
        Request::SetParam { id, value } => with_inst(|i| {
            // El valor llega al audio por la memoria compartida; aquí solo se refleja.
            i.backend().param_set_from_host(id, value);
            Ok(Reply::Done)
        }),
        Request::SaveState => with_inst(|i| Ok(Reply::State(i.state_base64()))),
        Request::LoadState { data } => with_inst(|i| {
            let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
            Ok(Reply::Bool(i.backend().load_state(&bytes)))
        }),
        Request::OpenEditor => with_inst(|i| i.open_editor().map(|_| Reply::Done)),
        Request::CloseEditor => with_inst(|i| {
            i.close_editor();
            Ok(Reply::Done)
        }),
        Request::Shutdown => {
            shutdown();
            unsafe { PostQuitMessage(0) };
            Ok(Reply::Done)
        }
    }
}

/// Hilo principal: cierra la interfaz, detiene el audio y destruye el plugin.
fn shutdown() {
    if let Some(mut h) = HOSTED.with(|h| h.borrow_mut().take()) {
        h.inst.close_editor();
        h.stop.store(true, Ordering::Relaxed);
        if let Some(a) = h.audio.take() {
            let _ = a.join();
        }
        drop(h); // última referencia: se apaga el plugin aquí mismo (hilo principal)
    }
}

/// Si la línea de órdenes pide el modo anfitrión, lo ejecuta y devuelve `true`
/// (el proceso debe terminar después); si no, devuelve `false`.
pub fn host_main() -> bool {
    if !std::env::args().any(|a| a == HOST_FLAG) {
        return false;
    }
    if let Err(e) = run() {
        eprintln!("[mn-plugin-host] {e}");
        std::process::exit(1);
    }
    true
}

fn run() -> Result<(), String> {
    unsafe {
        // El protocolo usa la salida estándar original; lo que impriman los
        // plugins (printf, std::cout) va a la salida de errores.
        let out = GetStdHandle(STD_OUTPUT_HANDLE);
        if out.is_null() || out as isize == -1 {
            return Err("sin salida estándar para hablar con la aplicación".into());
        }
        let _ = OUT.set(Mutex::new(File::from_raw_handle(out)));
        SetStdHandle(STD_OUTPUT_HANDLE, GetStdHandle(STD_ERROR_HANDLE));
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    create_message_window()?;
    crate::set_main_thread_executor(post);
    crate::on_editor_closed(|_| send(&Message::EditorClosed));

    // Lector de órdenes. Si la aplicación desaparece (fin de stdin), se cierra todo.
    std::thread::Builder::new()
        .name("mn-plugin-ordenes".into())
        .spawn(|| {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Envelope>(&line) {
                    Ok(Envelope { id, req }) => post(Box::new(move || reply(id, handle(req)))),
                    Err(e) => eprintln!("[mn-plugin-host] orden ilegible: {e}"),
                }
            }
            post(Box::new(|| {
                shutdown();
                unsafe { PostQuitMessage(0) };
            }));
        })
        .map_err(|e| e.to_string())?;

    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    shutdown();
    // Sin esperar a hilos que los plugins hayan dejado vivos.
    std::process::exit(0);
}
