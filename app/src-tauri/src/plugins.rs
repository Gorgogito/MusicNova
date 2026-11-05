//! Plugins CLAP/VST3: escaneo aislado, caché y comandos para la interfaz.
//!
//! Cada archivo se escanea relanzando este mismo ejecutable con
//! `--scan-plugin <ruta>`: si un plugin se cuelga o se cae, solo cae ese
//! proceso (se corta a los 30 s) y el archivo queda marcado como fallido.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mn_plugin::{default_folders, find_candidates, scan_file, ParamInfo, PluginDescriptor};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::AppState;

const SCAN_TIMEOUT: Duration = Duration::from_secs(30);
/// Prefijo de la línea con el resultado: los plugins pueden escribir en la salida estándar.
const SCAN_MARK: &str = "MUSICNOVA-SCAN ";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScanFailure {
    pub path: String,
    pub error: String,
}

/// Resultado del último escaneo, guardado en la carpeta de datos.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PluginCache {
    /// Carpetas añadidas por el usuario (además de las estándar).
    pub folders: Vec<String>,
    pub plugins: Vec<PluginDescriptor>,
    pub failed: Vec<ScanFailure>,
    pub scanned_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Carpetas estándar (informativo para la interfaz; no se guarda).
    #[serde(skip_deserializing, default)]
    pub default_folders: Vec<String>,
}

fn cache_path() -> Option<PathBuf> {
    mn_core::ffmpeg::data_dir().map(|d| d.join("plugins.json"))
}

pub fn load_cache() -> PluginCache {
    let mut c: PluginCache = cache_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    c.default_folders = default_folders().iter().map(|p| p.display().to_string()).collect();
    c
}

fn save_cache(c: &PluginCache) -> Result<(), String> {
    let p = cache_path().ok_or("no se encontró la carpeta de datos")?;
    std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&p, serde_json::to_string_pretty(c).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// Modo `--scan-plugin <ruta>`: escanea en este proceso e imprime JSON.
/// Devuelve `true` si se ejecutó (y la aplicación no debe arrancar).
pub fn scan_subprocess_main() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let Some(i) = args.iter().position(|a| a == "--scan-plugin") else { return false };
    let result = match args.get(i + 1) {
        Some(p) => scan_file(Path::new(p)),
        None => Err("falta la ruta del plugin".into()),
    };
    let out = match result {
        Ok(list) => serde_json::json!({ "ok": list }),
        Err(e) => serde_json::json!({ "error": e }),
    };
    println!("\n{SCAN_MARK}{out}");
    true
}

/// Resultado del escáner: la última línea marcada de su salida.
fn parse_scan_output(text: &str) -> Option<serde_json::Value> {
    text.lines().rev().find_map(|l| l.trim().strip_prefix(SCAN_MARK)).and_then(|j| serde_json::from_str(j).ok())
}

fn scan_isolated(path: &Path) -> Result<Vec<PluginDescriptor>, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.arg("--scan-plugin").arg(path).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().map_err(|e| format!("no se pudo lanzar el escáner: {e}"))?;
    // Se lee mientras corre: si el plugin escribe mucho, la tubería no se llena y bloquea.
    let reader = child.stdout.take().map(|mut out| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).into_owned()
        })
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if start.elapsed() > SCAN_TIMEOUT {
            let _ = child.kill();
            return Err("el plugin no respondió en 30 s".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let text = reader.and_then(|r| r.join().ok()).unwrap_or_default();
    let v = parse_scan_output(&text).ok_or_else(|| format!("el plugin cerró el escáner inesperadamente ({status})"))?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    serde_json::from_value(v["ok"].clone()).map_err(|e| e.to_string())
}

#[derive(Clone, Serialize)]
struct ScanProgress {
    done: usize,
    total: usize,
    current: String,
}

#[tauri::command]
pub fn plugins_list() -> PluginCache {
    load_cache()
}

/// Escanea las carpetas estándar y las del usuario, y guarda el resultado.
#[tauri::command]
pub async fn plugins_scan(app: AppHandle, folders: Vec<String>) -> Result<PluginCache, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut all: Vec<PathBuf> = default_folders();
        all.extend(folders.iter().map(PathBuf::from));
        let candidates = find_candidates(&all);
        let total = candidates.len();
        let mut cache = PluginCache { folders, ..Default::default() };
        for (i, path) in candidates.iter().enumerate() {
            let _ = app.emit("plugin-scan", ScanProgress { done: i, total, current: path.display().to_string() });
            match scan_isolated(path) {
                Ok(list) => cache.plugins.extend(list),
                Err(error) => cache.failed.push(ScanFailure { path: path.display().to_string(), error }),
            }
        }
        let _ = app.emit("plugin-scan", ScanProgress { done: total, total, current: String::new() });
        cache.plugins.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.format.as_str().cmp(b.format.as_str())));
        cache.scanned_at = Some(chrono::Utc::now());
        save_cache(&cache)?;
        cache.default_folders = default_folders().iter().map(|p| p.display().to_string()).collect();
        Ok(cache)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn live(slot_id: &str) -> Result<std::sync::Arc<mn_plugin::Instance>, String> {
    let inst = mn_plugin::instance(slot_id).ok_or_else(|| {
        mn_plugin::load_error(slot_id).unwrap_or_else(|| "el plugin no está cargado (¿falta el archivo?)".into())
    })?;
    match inst.failure() {
        Some(e) => Err(e),
        None => Ok(inst),
    }
}

/// Vuelve a cargar un plugin que se cayó o dejó de responder.
#[tauri::command]
pub async fn plugin_restart(app: AppHandle, slot_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || crate::restart_plugin(&app.state::<AppState>(), &slot_id)).await.map_err(|e| e.to_string())??;
    Ok(())
}

/// Aviso a la interfaz cuando el plugin de un efecto se cae o deja de responder
/// (su proceso termina; el efecto pasa el audio sin procesar).
pub fn on_plugin_failed(app: AppHandle) {
    mn_plugin::on_plugin_failed(move |slot_id, message| {
        let _ = app.emit("plugin-failed", serde_json::json!({ "slot_id": slot_id, "message": message }));
    });
}

#[tauri::command]
pub fn plugin_open_editor(slot_id: String) -> Result<(), String> {
    live(&slot_id)?.open_editor()
}

#[tauri::command]
pub fn plugin_params(slot_id: String) -> Result<Vec<ParamInfo>, String> {
    Ok(live(&slot_id)?.params())
}

#[tauri::command]
pub fn plugin_param_set(slot_id: String, id: u32, value: f64) -> Result<(), String> {
    live(&slot_id)?.set_param(id, value);
    Ok(())
}

/// Valor de un parámetro a partir del texto que escribió el usuario («-6 dB»),
/// según el propio plugin. `None` si no lo entiende.
#[tauri::command]
pub fn plugin_param_parse(slot_id: String, id: u32, text: String) -> Result<Option<f64>, String> {
    Ok(live(&slot_id)?.param_from_text(id, &text))
}

/// Guarda en el proyecto el estado actual de todos sus plugins.
#[tauri::command]
pub fn plugins_sync(state: State<'_, AppState>) -> Result<(), String> {
    crate::sync_plugin_states(&state)
}

/// Al cerrar la ventana de un plugin se guarda su estado (en otro hilo: el
/// aviso llega desde el bucle de mensajes del hilo principal).
pub fn on_editor_closed(app: AppHandle) {
    mn_plugin::on_editor_closed(move |slot_id| {
        let app = app.clone();
        let slot_id = slot_id.to_string();
        std::thread::spawn(move || {
            if let Err(e) = crate::sync_plugin_states(&app.state::<AppState>()) {
                eprintln!("[musicnova] estado de plugins: {e}");
            }
            let _ = app.emit("plugin-editor-closed", slot_id);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_output_ignores_plugin_noise() {
        let text = format!("hola desde el plugin\n{{\"ok\":1}}\n{SCAN_MARK}{{\"ok\":[]}}\nadiós\n");
        assert_eq!(parse_scan_output(&text).unwrap(), serde_json::json!({ "ok": [] }));
        assert!(parse_scan_output("{\"ok\":[]}").is_none());
    }
}
