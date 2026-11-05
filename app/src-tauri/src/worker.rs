//! Supervisor del proceso de IA en Python (`python -m musicnova_ai`).
//!
//! Habla JSON por líneas: las respuestas se emparejan por `id` y los eventos
//! (`job.progress`, `job.done`...) se reenvían a la interfaz como `worker-event`.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;

type Reply = Result<Value, String>;

#[derive(Clone, Debug, Serialize, Default)]
pub struct WorkerStatus {
    /// "stopped" | "starting" | "ready" | "crashed"
    pub state: String,
    pub python: String,
    pub worker_dir: String,
    pub error: Option<String>,
    pub stderr_tail: Vec<String>,
}

struct Proc {
    child: Child,
    stdin: ChildStdin,
}

pub struct Worker {
    app: AppHandle,
    proc: Mutex<Option<Proc>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>,
    status: Arc<Mutex<WorkerStatus>>,
    next_id: AtomicU64,
}

const STDERR_LINES: usize = 40;

/// Código del proceso de IA en el repositorio (desarrollo).
const SOURCE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../ai-worker");

/// Dónde está el código del proceso de IA y con qué Python se ejecuta.
struct Runtime {
    dir: PathBuf,
    python: PathBuf,
    /// Entorno propio de la app instalada: las dependencias se instalan bajo demanda.
    managed: bool,
}

fn venv_python(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts").join("python.exe")
    } else {
        venv.join("bin").join("python")
    }
}

/// Sin el prefijo `\\?\` de las rutas de Tauri (más legible en «Sistema»).
fn plain_path(p: PathBuf) -> PathBuf {
    let s = p.display().to_string();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p,
    }
}

/// Carpeta de datos por usuario (la misma que usa el proceso de IA).
pub fn data_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("MUSICNOVA_DATA") {
        return Some(PathBuf::from(d));
    }
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("MusicNova"))
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/musicnova"))
    }
}

/// Crea (o rehace, si cambió la versión de Python incluida) el entorno del
/// proceso de IA a partir del Python que trae el instalador. Sin red: el venv
/// sale con pip de la propia instalación y empieza sin paquetes.
fn ensure_venv(bundled: &Path, venv: &Path) -> Result<(), String> {
    let marker = bundled.join("MUSICNOVA_PYTHON_VERSION");
    let want = std::fs::read_to_string(&marker).unwrap_or_default().trim().to_string();
    let have_file = venv.join("MUSICNOVA_PYTHON_VERSION");
    let have = std::fs::read_to_string(&have_file).unwrap_or_default().trim().to_string();
    if venv_python(venv).is_file() && have == want {
        return Ok(());
    }
    if venv.exists() {
        std::fs::remove_dir_all(venv).map_err(|e| format!("no se pudo rehacer el entorno de IA: {e}"))?;
    }
    if let Some(parent) = venv.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let exe = if cfg!(windows) { bundled.join("python.exe") } else { bundled.join("bin").join("python3") };
    let mut cmd = Command::new(&exe);
    cmd.args(["-m", "venv"]).arg(venv);
    if let Some(d) = data_dir() {
        cmd.env("PYTHONPYCACHEPREFIX", d.join("cache").join("pycache"));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("no se pudo iniciar Python ({}): {e}", exe.display()))?;
    if !out.status.success() {
        return Err(format!("no se pudo crear el entorno de IA: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let _ = std::fs::write(have_file, format!("{want}\n"));
    Ok(())
}

fn runtime(app: &AppHandle) -> Result<Runtime, String> {
    // Variables para depurar o para usar otro Python.
    let env_dir = std::env::var_os("MUSICNOVA_WORKER_DIR").map(PathBuf::from);
    let env_python = std::env::var_os("MUSICNOVA_PYTHON").map(PathBuf::from);
    let resources = app.path().resource_dir().ok();
    let bundled_python = resources.as_ref().map(|r| r.join("python")).filter(|p| p.join("MUSICNOVA_PYTHON_VERSION").is_file());
    let bundled_worker = resources.as_ref().map(|r| r.join("ai-worker")).filter(|p| p.join("musicnova_ai").is_dir());

    // Desarrollo: el código del repositorio y su .venv (MUSICNOVA_FORCE_MANAGED=1
    // prueba en desarrollo el entorno gestionado de la app instalada).
    let source = PathBuf::from(SOURCE_DIR);
    let force_managed = std::env::var_os("MUSICNOVA_FORCE_MANAGED").is_some();
    if (cfg!(debug_assertions) && !force_managed) || (env_dir.is_some() && bundled_python.is_none()) {
        let dir = env_dir.unwrap_or(source);
        let venv = venv_python(&dir.join(".venv"));
        let python = env_python.unwrap_or(if venv.is_file() { venv } else { PathBuf::from("python") });
        return Ok(Runtime { dir, python, managed: false });
    }
    // App instalada: el código empaquetado y un entorno propio por usuario.
    let dir = env_dir.or(bundled_worker).ok_or("no se encontró el código del proceso de IA en la instalación")?;
    if let Some(python) = env_python {
        return Ok(Runtime { dir, python, managed: false });
    }
    let bundled = bundled_python.ok_or("no se encontró el Python incluido en la instalación")?;
    let venv = data_dir().ok_or("no se encontró la carpeta de datos")?.join("envs").join("worker");
    ensure_venv(&bundled, &venv)?;
    Ok(Runtime { dir: plain_path(dir), python: venv_python(&venv), managed: true })
}

impl Worker {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            proc: Mutex::new(None),
            pending: Arc::new(Mutex::new(HashMap::new())),
            status: Arc::new(Mutex::new(WorkerStatus { state: "stopped".into(), ..Default::default() })),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn status(&self) -> WorkerStatus {
        self.status.lock().unwrap().clone()
    }

    fn set_status(&self, f: impl FnOnce(&mut WorkerStatus)) {
        let snapshot = {
            let mut s = self.status.lock().unwrap();
            f(&mut s);
            s.clone()
        };
        let _ = self.app.emit("worker-status", snapshot);
    }

    pub fn start(&self) -> Result<(), String> {
        let mut guard = self.proc.lock().unwrap();
        if let Some(p) = guard.as_mut() {
            if matches!(p.child.try_wait(), Ok(None)) {
                return Ok(());
            }
        }
        self.set_status(|s| {
            s.state = "starting".into();
            s.error = None;
            s.stderr_tail.clear();
        });
        let rt = runtime(&self.app).map_err(|e| {
            self.set_status(|s| {
                s.state = "crashed".into();
                s.error = Some(e.clone());
            });
            e
        })?;
        let managed = rt.managed;
        let (dir, python) = (rt.dir, rt.python);
        self.set_status(|s| {
            s.python = python.display().to_string();
            s.worker_dir = dir.display().to_string();
        });

        let mut cmd = Command::new(&python);
        // App instalada: las cachés de bytecode van a la carpeta de datos, no a la del programa.
        if managed {
            if let Some(d) = data_dir() {
                cmd.env("PYTHONPYCACHEPREFIX", d.join("cache").join("pycache"));
            }
        }
        cmd.args(["-u", "-m", "musicnova_ai"])
            .current_dir(&dir)
            .env("PYTHONPATH", &dir)
            .env("PYTHONIOENCODING", "utf-8")
            // Que pip y Python no lean configuración ni paquetes del usuario.
            .env("PYTHONNOUSERSITE", "1")
            .env("PIP_DISABLE_PIP_VERSION_CHECK", "1")
            .env("MUSICNOVA_MANAGED_ENV", if managed { "1" } else { "0" })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| {
            let msg = format!("no se pudo iniciar Python ({}): {e}", python.display());
            self.set_status(|s| {
                s.state = "crashed".into();
                s.error = Some(msg.clone());
            });
            msg
        })?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        // stdout: protocolo.
        let app = self.app.clone();
        let pending = self.pending.clone();
        let status = self.status.clone();
        std::thread::Builder::new()
            .name("mn-worker-out".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                        eprintln!("[worker] línea no válida: {line}");
                        continue;
                    };
                    if let Some(ev) = msg.get("event").and_then(Value::as_str) {
                        if ev == "ready" {
                            let snap = {
                                let mut s = status.lock().unwrap();
                                s.state = "ready".into();
                                s.clone()
                            };
                            let _ = app.emit("worker-status", snap);
                        }
                        let _ = app.emit("worker-event", &msg);
                    } else if let Some(id) = msg.get("id").and_then(Value::as_u64) {
                        if let Some(tx) = pending.lock().unwrap().remove(&id) {
                            let reply = match msg.get("error") {
                                Some(err) => Err(err
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("error desconocido")
                                    .to_string()),
                                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                            };
                            let _ = tx.send(reply);
                        }
                    }
                }
                // El proceso terminó: falla todas las peticiones pendientes.
                for (_, tx) in pending.lock().unwrap().drain() {
                    let _ = tx.send(Err("el proceso de IA se detuvo".into()));
                }
                let snap = {
                    let mut s = status.lock().unwrap();
                    if s.state != "stopped" {
                        s.state = "crashed".into();
                        if s.error.is_none() {
                            s.error = Some("el proceso de IA terminó inesperadamente".into());
                        }
                    }
                    s.clone()
                };
                let _ = app.emit("worker-status", snap);
            })
            .map_err(|e| e.to_string())?;

        // stderr: registro y diagnóstico.
        let status = self.status.clone();
        std::thread::Builder::new()
            .name("mn-worker-err".into())
            .spawn(move || {
                let mut tail = VecDeque::with_capacity(STDERR_LINES);
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    eprintln!("[worker] {line}");
                    if tail.len() == STDERR_LINES {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                    status.lock().unwrap().stderr_tail = tail.iter().cloned().collect();
                }
            })
            .map_err(|e| e.to_string())?;

        *guard = Some(Proc { child, stdin });
        Ok(())
    }

    pub fn stop(&self) {
        self.set_status(|s| s.state = "stopped".into());
        let proc = self.proc.lock().unwrap().take();
        if let Some(mut p) = proc {
            let _ = writeln!(p.stdin, "{}", json!({"id": 0, "method": "shutdown"}));
            let _ = p.stdin.flush();
            for _ in 0..20 {
                if matches!(p.child.try_wait(), Ok(Some(_))) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = p.child.kill();
        }
    }

    pub async fn call(&self, method: &str, params: Value) -> Reply {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        {
            let mut guard = self.proc.lock().unwrap();
            let Some(p) = guard.as_mut() else {
                self.pending.lock().unwrap().remove(&id);
                return Err("el proceso de IA no está en ejecución".into());
            };
            let line = json!({"id": id, "method": method, "params": params}).to_string();
            if let Err(e) = writeln!(p.stdin, "{line}").and_then(|_| p.stdin.flush()) {
                self.pending.lock().unwrap().remove(&id);
                return Err(format!("no se pudo escribir al proceso de IA: {e}"));
            }
        }
        match tokio::time::timeout(Duration::from_secs(120), rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err("el proceso de IA cerró la conexión".into()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("tiempo de espera agotado en '{method}'"))
            }
        }
    }
}
