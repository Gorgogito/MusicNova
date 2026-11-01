//! Supervisor del proceso de IA en Python (`python -m musicnova_ai`).
//!
//! Habla JSON por líneas: las respuestas se emparejan por `id` y los eventos
//! (`job.progress`, `job.done`...) se reenvían a la interfaz como `worker-event`.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
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

fn worker_dir() -> PathBuf {
    if let Ok(d) = std::env::var("MUSICNOVA_WORKER_DIR") {
        return PathBuf::from(d);
    }
    // Desarrollo: el worker vive junto al código fuente.
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../ai-worker"))
}

fn python_exe(dir: &std::path::Path) -> PathBuf {
    if let Ok(p) = std::env::var("MUSICNOVA_PYTHON") {
        return PathBuf::from(p);
    }
    let venv = if cfg!(windows) { dir.join(".venv/Scripts/python.exe") } else { dir.join(".venv/bin/python") };
    if venv.is_file() {
        venv
    } else {
        PathBuf::from("python")
    }
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
        let dir = worker_dir();
        let python = python_exe(&dir);
        self.set_status(|s| {
            s.state = "starting".into();
            s.python = python.display().to_string();
            s.worker_dir = dir.display().to_string();
            s.error = None;
            s.stderr_tail.clear();
        });

        let mut cmd = Command::new(&python);
        cmd.args(["-u", "-m", "musicnova_ai"])
            .current_dir(&dir)
            .env("PYTHONPATH", &dir)
            .env("PYTHONIOENCODING", "utf-8")
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
