//! Actualizaciones desde GitHub Releases (plugin oficial de Tauri).
//!
//! Cada versión publica `latest.json` y el instalador firmado. La app lo
//! comprueba, verifica la firma con la clave pública incluida en
//! `tauri.conf.json`, descarga el instalador y lo ejecuta en modo pasivo; al
//! terminar se reinicia con la versión nueva. `MUSICNOVA_UPDATE_URL` apunta a
//! otro `latest.json` (pruebas con un servidor local).

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::{err, AppState};

/// Actualización encontrada en la última comprobación, lista para instalar.
static PENDING: Mutex<Option<Update>> = Mutex::new(None);

#[derive(Serialize)]
pub struct UpdateInfo {
    version: String,
    current: String,
    notes: Option<String>,
    date: Option<String>,
}

/// Busca una versión nueva. `None`: la instalada es la última.
#[tauri::command]
pub async fn update_check(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let mut builder = app.updater_builder();
    if let Ok(url) = std::env::var("MUSICNOVA_UPDATE_URL") {
        let url = url.parse().map_err(|e| format!("dirección de actualizaciones no válida: {e}"))?;
        builder = builder.endpoints(vec![url]).map_err(err)?;
    }
    let updater = builder.build().map_err(err)?;
    let update = updater.check().await.map_err(|e| format!("no se pudo comprobar si hay actualizaciones: {e}"))?;
    let info = update.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        current: u.current_version.clone(),
        notes: u.body.clone(),
        date: u.date.map(|d| d.to_string()),
    });
    *PENDING.lock().unwrap() = update;
    Ok(info)
}

#[derive(Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

/// Descarga e instala la actualización encontrada y reinicia la app. Antes se
/// guarda el estado de los plugins del proyecto abierto (lo demás ya está guardado).
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    let update = PENDING.lock().unwrap().take().ok_or("no hay ninguna actualización pendiente: vuelve a comprobar")?;
    if let Err(e) = crate::sync_plugin_states(&app.state::<AppState>()) {
        eprintln!("[musicnova] estado de plugins: {e}");
    }
    let mut downloaded = 0u64;
    let progress = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = progress.emit("update-progress", Progress { downloaded, total });
            },
            || {},
        )
        .await
        .map_err(|e| format!("no se pudo instalar la actualización: {e}"))?;
    app.restart()
}
