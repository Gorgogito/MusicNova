//! Lado de la aplicación de un plugin que corre en su propio proceso.
//!
//! [`RemoteBackend`] implementa [`Backend`] hablando con el anfitrión
//! (`--plugin-host`): las órdenes de «hilo principal» viajan por stdin/stdout y
//! cada bloque de audio por memoria compartida. Si el plugin se cae o deja de
//! responder, el efecto pasa el audio sin procesar, se avisa con
//! [`crate::on_plugin_failed`] y la aplicación sigue funcionando.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use mn_fx::PluginRef;
use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::WaitForMultipleObjects;
use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

use crate::ipc::{Envelope, Event, Message, Reply, Request, ShNote, ShParam, Shm, MARK, MAX_EVENTS};
use crate::{notify_editor_closed, notify_failed, Backend, NoteEvent, ParamInfo};

/// Tiempo para cargar el plugin (algunos cargan bibliotecas de muestras).
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// Tiempo para cualquier otra orden; si se agota, el plugin está colgado.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(8);
/// Espera de un bloque de audio en tiempo real; si se agota, ese bloque (y los
/// siguientes, hasta que el plugin responda) pasan sin procesar.
const AUDIO_TIMEOUT_RT_MS: u32 = 250;
/// Espera de un bloque al exportar o generar (sin prisa, pero no infinita).
const AUDIO_TIMEOUT_OFFLINE_MS: u32 = 30_000;
/// Un audio colgado más de esto termina el proceso del plugin.
const STALL_KILL: Duration = Duration::from_secs(5);

const REASON_NONE: u8 = 0;
const REASON_CONTROL: u8 = 1;
const REASON_AUDIO: u8 = 2;

/// Cómo lanzar el anfitrión: ejecutable y argumentos (la app se relanza a sí misma).
pub(crate) struct HostCommand {
    pub exe: std::path::PathBuf,
    pub args: Vec<String>,
}

/// Estado compartido con los hilos lector y vigilante.
struct Inner {
    name: String,
    pending: Mutex<HashMap<u64, mpsc::Sender<Result<Reply, String>>>>,
    next_id: AtomicU64,
    dead: AtomicBool,
    failure: Mutex<Option<String>>,
    reason: AtomicU8,
    kill: AtomicBool,
    shutting_down: AtomicBool,
    slot: Mutex<Option<String>>,
    editor_open: AtomicBool,
    child: Mutex<Child>,
}

impl Inner {
    fn failure_text(&self) -> String {
        self.failure.lock().unwrap().clone().unwrap_or_else(|| format!("«{}» ya no está disponible (su proceso terminó)", self.name))
    }
}

pub(crate) struct RemoteBackend {
    inner: Arc<Inner>,
    pid: u32,
    process: isize,
    stdin: Mutex<ChildStdin>,
    shm: Shm,
    ev_req: Event,
    ev_done: Event,
    latency: u32,
    realtime: bool,
    stalled: AtomicBool,
    epoch: Instant,
    stall_since_ms: AtomicU64,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl RemoteBackend {
    /// Lanza el anfitrión y carga en él el plugin con su estado guardado.
    pub(crate) fn spawn(cmd: &HostCommand, plugin: &PluginRef, sample_rate: u32, realtime: bool) -> Result<Self, String> {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = format!("Local\\MusicNova-{}-{n}", std::process::id());
        let (shm_name, req_name, done_name) = (format!("{base}-shm"), format!("{base}-req"), format!("{base}-done"));
        let shm = Shm::create(&shm_name)?;
        let ev_req = Event::create(&req_name)?;
        let ev_done = Event::create(&done_name)?;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut child = Command::new(&cmd.exe)
            .args(&cmd.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("no se pudo lanzar el proceso del plugin: {e}"))?;
        let stdin = child.stdin.take().expect("stdin del anfitrión");
        let stdout = child.stdout.take().expect("stdout del anfitrión");
        let pid = child.id();
        let process = child.as_raw_handle() as isize;
        let inner = Arc::new(Inner {
            name: plugin.name.clone(),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            dead: AtomicBool::new(false),
            failure: Mutex::new(None),
            reason: AtomicU8::new(REASON_NONE),
            kill: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            slot: Mutex::new(None),
            editor_open: AtomicBool::new(false),
            child: Mutex::new(child),
        });

        // Lector: respuestas y avisos. Las líneas sin la marca son ruido del plugin.
        let r = inner.clone();
        std::thread::Builder::new()
            .name("mn-plugin-lector".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Some(json) = line.strip_prefix(MARK) else { continue };
                    match serde_json::from_str::<Message>(json) {
                        Ok(Message::Reply { id, result }) => {
                            if let Some(tx) = r.pending.lock().unwrap().remove(&id) {
                                let _ = tx.send(result);
                            }
                        }
                        Ok(Message::EditorClosed) => {
                            r.editor_open.store(false, Ordering::Relaxed);
                            let slot = r.slot.lock().unwrap().clone();
                            notify_editor_closed(slot.as_deref());
                        }
                        Err(e) => eprintln!("[mn-plugin] mensaje ilegible del anfitrión: {e}"),
                    }
                }
                // Fin de la salida: el proceso terminó. Se despierta a quien espere.
                r.pending.lock().unwrap().clear();
            })
            .map_err(|e| e.to_string())?;

        // Vigilante: detecta el final del proceso y lo termina si se lo piden.
        let w = inner.clone();
        std::thread::Builder::new()
            .name("mn-plugin-vigilante".into())
            .spawn(move || loop {
                if w.kill.load(Ordering::Relaxed) {
                    let _ = w.child.lock().unwrap().kill();
                }
                let status = w.child.lock().unwrap().try_wait();
                match status {
                    Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                    Ok(Some(st)) => {
                        let message = match w.reason.load(Ordering::Relaxed) {
                            REASON_CONTROL => Some(format!("«{}» dejó de responder y se cerró", w.name)),
                            REASON_AUDIO => Some(format!("el audio de «{}» dejó de responder y se cerró", w.name)),
                            _ if w.shutting_down.load(Ordering::Relaxed) => None,
                            _ => Some(match st.code() {
                                Some(c) => format!("«{}» se cerró inesperadamente (código 0x{:08X})", w.name, c as u32),
                                None => format!("«{}» se cerró inesperadamente", w.name),
                            }),
                        };
                        if let Some(m) = &message {
                            *w.failure.lock().unwrap() = Some(m.clone());
                        }
                        w.dead.store(true, Ordering::Release);
                        w.pending.lock().unwrap().clear();
                        if let Some(m) = message {
                            eprintln!("[mn-plugin] {m}");
                            let slot = w.slot.lock().unwrap().clone();
                            if let Some(s) = slot {
                                notify_failed(&s, &m);
                            }
                        }
                        break;
                    }
                    Err(_) => break,
                }
            })
            .map_err(|e| e.to_string())?;

        let mut me = Self {
            inner,
            pid,
            process,
            stdin: Mutex::new(stdin),
            shm,
            ev_req,
            ev_done,
            latency: 0,
            realtime,
            stalled: AtomicBool::new(false),
            epoch: Instant::now(),
            stall_since_ms: AtomicU64::new(0),
        };
        let req = Request::Load { plugin: plugin.clone(), sample_rate, shm: shm_name, ev_req: req_name, ev_done: done_name };
        match me.request(req, LOAD_TIMEOUT) {
            Ok(Reply::Loaded { latency }) => {
                me.latency = latency;
                Ok(me)
            }
            Ok(_) => {
                me.terminate();
                Err("respuesta inesperada del proceso del plugin".into())
            }
            Err(e) => {
                me.terminate();
                Err(e)
            }
        }
    }

    fn write(&self, env: &Envelope) -> Result<(), String> {
        let line = serde_json::to_string(env).map_err(|e| e.to_string())?;
        let mut w = self.stdin.lock().unwrap();
        writeln!(w, "{line}").and_then(|_| w.flush()).map_err(|_| self.inner.failure_text())
    }

    /// Envía una orden y espera su respuesta. Si no llega a tiempo, el plugin
    /// está colgado: se termina su proceso.
    fn request(&self, req: Request, timeout: Duration) -> Result<Reply, String> {
        if self.inner.dead.load(Ordering::Acquire) {
            return Err(self.inner.failure_text());
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.inner.pending.lock().unwrap().insert(id, tx);
        if let Err(e) = self.write(&Envelope { id, req }) {
            self.inner.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.inner.pending.lock().unwrap().remove(&id);
                self.inner.reason.store(REASON_CONTROL, Ordering::Relaxed);
                self.inner.kill.store(true, Ordering::Relaxed);
                Err(format!("«{}» dejó de responder y se cerró", self.inner.name))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // El vigilante anota el motivo justo después de que el proceso termine.
                let until = Instant::now() + Duration::from_millis(500);
                while !self.inner.dead.load(Ordering::Acquire) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(self.inner.failure_text())
            }
        }
    }

    /// Orden sin respuesta.
    fn send(&self, req: Request) {
        if !self.inner.dead.load(Ordering::Acquire) {
            let _ = self.write(&Envelope { id: 0, req });
        }
    }

    fn terminate(&self) {
        self.inner.shutting_down.store(true, Ordering::Relaxed);
        self.inner.kill.store(true, Ordering::Relaxed);
    }

    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }
}

impl Drop for RemoteBackend {
    fn drop(&mut self) {
        // Sin `shutdown` previo (error al cargar, pánico): que no quede el proceso.
        if !self.inner.dead.load(Ordering::Acquire) {
            self.inner.shutting_down.store(true, Ordering::Relaxed);
            let _ = self.inner.child.lock().unwrap().kill();
        }
    }
}

fn pass_through(input: [&[f32]; 2], output: [&mut [f32]; 2]) {
    let [ol, or] = output;
    ol.copy_from_slice(input[0]);
    or.copy_from_slice(input[1]);
}

impl Backend for RemoteBackend {
    fn needs_main_thread(&self) -> bool {
        false
    }

    fn params(&self) -> Vec<ParamInfo> {
        match self.request(Request::Params, CONTROL_TIMEOUT) {
            Ok(Reply::Params(p)) => p,
            _ => Vec::new(),
        }
    }

    fn param_set_from_host(&self, id: u32, value: f64) {
        self.send(Request::SetParam { id, value });
    }

    fn save_state(&self) -> Option<Vec<u8>> {
        match self.request(Request::SaveState, CONTROL_TIMEOUT) {
            Ok(Reply::State(Some(s))) => base64::engine::general_purpose::STANDARD.decode(s).ok(),
            _ => None,
        }
    }

    fn load_state(&self, data: &[u8]) -> bool {
        let data = base64::engine::general_purpose::STANDARD.encode(data);
        matches!(self.request(Request::LoadState { data }, CONTROL_TIMEOUT), Ok(Reply::Bool(true)))
    }

    fn gui_open(&self, _parent: isize) -> Result<(u32, u32), String> {
        Err("la interfaz de este plugin se abre en su propio proceso".into())
    }
    fn gui_close(&self) {}
    fn gui_can_resize(&self) -> bool {
        false
    }
    fn gui_set_size(&self, _w: u32, _h: u32) {}

    fn external_editor_open(&self) -> Option<Result<(), String>> {
        // Que su ventana pueda pasar al frente aunque el proceso no tenga el foco.
        unsafe { AllowSetForegroundWindow(self.pid) };
        let r = self.request(Request::OpenEditor, CONTROL_TIMEOUT).map(|_| ());
        if r.is_ok() {
            self.inner.editor_open.store(true, Ordering::Relaxed);
        }
        Some(r)
    }

    fn external_editor_close(&self) {
        self.inner.editor_open.store(false, Ordering::Relaxed);
        self.send(Request::CloseEditor);
    }

    fn external_editor_is_open(&self) -> Option<bool> {
        Some(self.inner.editor_open.load(Ordering::Relaxed) && !self.inner.dead.load(Ordering::Acquire))
    }

    fn failure(&self) -> Option<String> {
        if self.inner.dead.load(Ordering::Acquire) {
            Some(self.inner.failure_text())
        } else {
            None
        }
    }

    fn attach_slot(&self, slot: &str) {
        *self.inner.slot.lock().unwrap() = Some(slot.to_string());
    }

    unsafe fn process(&self, input: [&[f32]; 2], output: [&mut [f32]; 2], params: &[(u32, f64)], notes: &[NoteEvent]) {
        let n = input[0].len();
        if self.inner.dead.load(Ordering::Acquire) {
            return pass_through(input, output);
        }
        if self.stalled.load(Ordering::Relaxed) {
            // Solo se vuelve a escribir en la memoria cuando el anfitrión entregó el bloque pendiente.
            if self.ev_done.wait(0) {
                self.stalled.store(false, Ordering::Relaxed);
            } else {
                if self.now_ms().saturating_sub(self.stall_since_ms.load(Ordering::Relaxed)) > STALL_KILL.as_millis() as u64
                    && !self.inner.kill.load(Ordering::Relaxed)
                {
                    self.inner.reason.store(REASON_AUDIO, Ordering::Relaxed);
                    self.inner.kill.store(true, Ordering::Relaxed);
                }
                return pass_through(input, output);
            }
        }
        let sh = &mut *self.shm.get();
        sh.frames = n as u32;
        sh.input[0][..n].copy_from_slice(input[0]);
        sh.input[1][..n].copy_from_slice(input[1]);
        let np = params.len().min(MAX_EVENTS);
        for (d, &(id, value)) in sh.params.iter_mut().zip(&params[..np]) {
            *d = ShParam { id, _pad: 0, value };
        }
        sh.n_params = np as u32;
        let nn = notes.len().min(MAX_EVENTS);
        for (d, e) in sh.notes.iter_mut().zip(&notes[..nn]) {
            *d = ShNote { time: e.time, key: e.key as u32, velocity: e.velocity, on: e.on as u32 };
        }
        sh.n_notes = nn as u32;
        self.ev_req.set();
        let handles: [HANDLE; 2] = [self.ev_done.raw(), self.process as HANDLE];
        let timeout = if self.realtime { AUDIO_TIMEOUT_RT_MS } else { AUDIO_TIMEOUT_OFFLINE_MS };
        let r = WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout);
        if r == WAIT_OBJECT_0 {
            let [ol, or] = output;
            ol.copy_from_slice(&sh.output[0][..n]);
            or.copy_from_slice(&sh.output[1][..n]);
        } else if r == WAIT_OBJECT_0 + 1 {
            // El proceso terminó; el vigilante lo anota y avisa.
            pass_through(input, output);
        } else {
            self.stalled.store(true, Ordering::Relaxed);
            self.stall_since_ms.store(self.now_ms(), Ordering::Relaxed);
            if !self.realtime {
                self.inner.reason.store(REASON_AUDIO, Ordering::Relaxed);
                self.inner.kill.store(true, Ordering::Relaxed);
            }
            pass_through(input, output);
        }
    }

    fn reset(&self) {
        if !self.stalled.load(Ordering::Relaxed) && !self.inner.dead.load(Ordering::Acquire) {
            unsafe { (*self.shm.get()).reset = 1 };
        }
    }

    fn latency(&self) -> u32 {
        self.latency
    }

    fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::Relaxed);
        self.send(Request::Shutdown);
        let until = Instant::now() + Duration::from_secs(3);
        while !self.inner.dead.load(Ordering::Acquire) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !self.inner.dead.load(Ordering::Acquire) {
            self.inner.kill.store(true, Ordering::Relaxed);
        }
    }
}
