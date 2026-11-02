//! Fase 7: limpieza de ruido y acompañamientos generados con IA.
//!
//! - Ruido constante (zumbido, siseo) en cualquier pista: `mn_core::denoise`, en Rust.
//! - Voz: DeepFilterNet3 en el proceso de IA.
//! - Acompañamientos: ACE-Step 1.5 (tarea «lego») en el proceso de IA, con la
//!   mezcla audible o la original como contexto y el tempo y la tonalidad del análisis.
//!
//! Los resultados se añaden como pistas generadas alineadas con la canción.

use mn_core::analysis::Analysis;
use mn_core::audio::{self, AudioBuffer};
use mn_core::denoise::{denoise, DenoiseSettings};
use mn_core::export::{export_audio, ExportFormat};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::transcribe::{add_generated_track, render_track_audio};
use crate::{err, render_premaster, session_snapshot, write_mix_input, AppState, ProjectView};

/// Carpetas de las pistas que genera esta fase (se pueden borrar desde la interfaz).
pub const DENOISED_DIR: &str = "audio/denoised/";
pub const GENERATED_DIR: &str = "audio/generated/";

const VOICE_RATE: u32 = 48_000;
const GENERATE_RATE: u32 = 48_000;

/// Pistas que sabe generar ACE-Step, con su nombre en la interfaz.
pub const GENERATED_TRACKS: [(&str, &str); 12] = [
    ("drums", "Batería"),
    ("bass", "Bajo"),
    ("guitar", "Guitarra"),
    ("keyboard", "Teclado"),
    ("strings", "Cuerdas"),
    ("synth", "Sintetizador"),
    ("percussion", "Percusión"),
    ("brass", "Metales"),
    ("woodwinds", "Vientos de madera"),
    ("backing_vocals", "Coros"),
    ("vocals", "Voz"),
    ("fx", "Efectos"),
];

fn track_label(track: &str) -> Option<&'static str> {
    GENERATED_TRACKS.iter().find(|(id, _)| *id == track).map(|(_, l)| *l)
}

/// Escribe `buf` como FLAC en `rel` y lo añade como pista generada.
fn add_buffer_track(
    state: &AppState,
    buf: &AudioBuffer,
    rel: String,
    name: String,
    mute_source: Option<Uuid>,
) -> Result<ProjectView, String> {
    let (dir, _) = session_snapshot(state)?;
    let path = dir.join(&rel);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
    export_audio(&path, buf, ExportFormat::Flac24).map_err(err)?;
    add_generated_track(state, rel, name, mute_source)
}

// ---- Limpieza de ruido ----------------------------------------------------------------

/// Quita el ruido constante de una pista (rápido, sin IA). La original queda silenciada.
#[tauri::command]
pub async fn denoise_steady(app: AppHandle, track_id: Uuid, settings: DenoiseSettings) -> Result<ProjectView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let (buf, info) = render_track_audio(&state, track_id)?;
        let clean = denoise(&buf, &settings);
        add_buffer_track(
            &state,
            &clean,
            format!("{DENOISED_DIR}{track_id}.flac"),
            format!("{} · sin ruido", info.name),
            Some(track_id),
        )
    })
    .await
    .map_err(err)?
}

/// Lanza la limpieza de voz con DeepFilterNet3 en el proceso de IA.
#[tauri::command]
pub async fn denoise_voice_start(app: AppHandle, track_id: Uuid, atten_lim_db: Option<f32>) -> Result<Value, String> {
    let app2 = app.clone();
    let input = tauri::async_runtime::spawn_blocking(move || {
        let state = app2.state::<AppState>();
        let (dir, _) = session_snapshot(&state)?;
        let (buf, _) = render_track_audio(&state, track_id)?;
        if buf.frames() == 0 {
            return Err("la pista está vacía".to_string());
        }
        let buf = audio::resample(&buf, VOICE_RATE).map_err(err)?;
        let path = dir.join("cache").join(format!("denoise_input_{track_id}.wav"));
        std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
        audio::write_wav_f32(&path, &buf).map_err(err)?;
        Ok(path)
    })
    .await
    .map_err(err)??;
    let state = app.state::<AppState>();
    let (dir, _) = session_snapshot(&state)?;
    let params = json!({
        "input": input,
        "output": dir.join("cache").join(format!("denoised_{track_id}.wav")),
        "atten_lim_db": atten_lim_db.filter(|v| *v > 0.0),
    });
    state.worker.clone().call("tasks.run", json!({ "task": "denoise_voice", "params": params })).await
}

/// Añade la voz limpia como pista nueva (la original queda silenciada).
#[tauri::command]
pub async fn denoise_voice_apply(app: AppHandle, track_id: Uuid) -> Result<ProjectView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let (dir, project) = session_snapshot(&state)?;
        let info = project.tracks.iter().find(|t| t.id == track_id).ok_or("pista desconocida")?.clone();
        let cache = dir.join("cache").join(format!("denoised_{track_id}.wav"));
        let buf = audio::decode_file(&cache).map_err(|_| "la limpieza de voz no generó resultados".to_string())?;
        let rate = state.engine()?.sample_rate();
        let buf = audio::resample(&buf, rate).map_err(err)?;
        let view = add_buffer_track(
            &state,
            &buf,
            format!("{DENOISED_DIR}{track_id}.flac"),
            format!("{} · voz limpia", info.name),
            Some(track_id),
        )?;
        let _ = std::fs::remove_file(cache);
        let _ = std::fs::remove_file(dir.join("cache").join(format!("denoise_input_{track_id}.wav")));
        Ok(view)
    })
    .await
    .map_err(err)?
}

// ---- Acompañamientos -------------------------------------------------------------------

/// Nombre de tonalidad que entiende ACE-Step («F# minor»).
fn keyscale(tonic: u8, mode: &str) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];
    format!("{} {}", NAMES[(tonic % 12) as usize], if mode == "minor" { "minor" } else { "major" })
}

/// Lanza la generación de una pista con ACE-Step. `context`: «audible» (lo que
/// suena ahora, sin las pistas silenciadas) u «original» (la mezcla importada).
#[tauri::command]
pub async fn generate_start(
    app: AppHandle,
    track: String,
    caption: String,
    context: String,
    steps: Option<u32>,
    seed: Option<i64>,
) -> Result<Value, String> {
    if track_label(&track).is_none() {
        return Err(format!("pista desconocida: {track}"));
    }
    let state = app.state::<AppState>();
    let (dir, project) = session_snapshot(&state)?;
    let input = if context == "original" {
        write_mix_input(&dir, &project, "generate_input.wav", GENERATE_RATE).await?
    } else {
        let app2 = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let state = app2.state::<AppState>();
            let (mixed, _, dir, _) = render_premaster(&state)?;
            let buf = audio::resample(&mixed, GENERATE_RATE).map_err(err)?;
            let path = dir.join("cache").join("generate_input.wav");
            std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
            audio::write_wav_f32(&path, &buf).map_err(err)?;
            Ok::<_, String>(path)
        })
        .await
        .map_err(err)??
    };
    let analysis = Analysis::load(&dir).ok().flatten();
    let mut params = json!({
        "input": input,
        "output": dir.join("cache").join(format!("generated_{track}.wav")),
        "track": track,
        "caption": caption,
        "steps": steps.unwrap_or(32).clamp(8, 100),
        "seed": seed.unwrap_or(-1),
    });
    if let Some(a) = &analysis {
        params["bpm"] = json!(a.bpm.round());
        params["keyscale"] = json!(keyscale(a.key.tonic, &a.key.mode));
        params["timesignature"] = json!(a.beats_per_bar.to_string());
    }
    state.worker.clone().call("tasks.run", json!({ "task": "generate_stem", "params": params })).await
}

/// Añade la pista generada al proyecto (cada generación es una pista nueva).
#[tauri::command]
pub async fn generate_apply(app: AppHandle, track: String) -> Result<ProjectView, String> {
    let label = track_label(&track).ok_or_else(|| format!("pista desconocida: {track}"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let (dir, _) = session_snapshot(&state)?;
        let cache = dir.join("cache").join(format!("generated_{track}.wav"));
        let buf = audio::decode_file(&cache).map_err(|_| "la generación no produjo audio".to_string())?;
        let rate = state.engine()?.sample_rate();
        let buf = audio::resample(&buf, rate).map_err(err)?;
        let id = Uuid::new_v4().simple().to_string();
        let view = add_buffer_track(
            &state,
            &buf,
            format!("{GENERATED_DIR}{track}-{}.flac", &id[..8]),
            format!("{label} · generado"),
            None,
        )?;
        let _ = std::fs::remove_file(cache);
        Ok(view)
    })
    .await
    .map_err(err)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyscale_names_match_acestep() {
        assert_eq!(keyscale(0, "major"), "C major");
        assert_eq!(keyscale(6, "minor"), "F# minor");
        assert_eq!(keyscale(10, "major"), "Bb major");
        assert!(track_label("drums").is_some() && track_label("kazoo").is_none());
    }
}
