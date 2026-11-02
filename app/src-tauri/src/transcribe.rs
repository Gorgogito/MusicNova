//! Fase 6: audio a MIDI, piano roll, afinación vocal e instrumentos.
//!
//! Las pistas se renderizan con sus clips (sin efectos) alineadas con la línea
//! de tiempo, así que las notas y la curva de tono quedan en tiempo de proyecto.

use std::path::{Path, PathBuf};

use mn_core::analysis::Analysis;
use mn_core::audio::{self, AudioBuffer};
use mn_core::export::{export_audio, ExportFormat};
use mn_core::mix::render_track;
use mn_core::project::{clip_frames, TrackInfo, TrackKind};
use mn_core::transcription::{
    synthesize, write_midi, InstrumentReport, MidiPart, Note, NotesResult, PitchCurve, TempoMap, Timbre, TrackNotes,
    Transcription,
};
use mn_core::tuning::{tune, TuneSettings};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::{err, load_into_engine, session_snapshot, write_mix_input, AppState, ProjectView};

/// Carpetas de las pistas que genera esta fase (se pueden borrar desde la interfaz).
pub const MIDI_DIR: &str = "audio/midi/";
pub const TUNED_DIR: &str = "audio/tuned/";

const NOTES_RATE: u32 = 22_050;
const PITCH_RATE: u32 = 16_000;
const INSTRUMENTS_RATE: u32 = 32_000;

/// Audio de una pista con sus clips, a la frecuencia del motor, desde el instante 0.
fn render_track_audio(state: &AppState, track_id: Uuid) -> Result<(AudioBuffer, TrackInfo), String> {
    let rate = state.engine()?.sample_rate();
    let (info, data) = {
        let s = state.session.lock().unwrap();
        let p = s.project.as_ref().ok_or("no hay ningún proyecto abierto")?;
        let info = p.tracks.iter().find(|t| t.id == track_id).ok_or("pista desconocida")?.clone();
        let data = s.buffers.get(&track_id).ok_or("la pista no tiene audio cargado")?.clone();
        (info, data)
    };
    let frames = clip_frames(&info.clips, rate, (data.len() / 2) as u64);
    Ok((AudioBuffer { sample_rate: rate, data: render_track(&data, &frames) }, info))
}

/// Escribe la pista renderizada a `rate` en `cache/<name>` para el proceso de IA.
async fn write_track_input(app: &AppHandle, track_id: Uuid, name: String, rate: u32) -> Result<PathBuf, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let (dir, _) = session_snapshot(&state)?;
        let (buf, _) = render_track_audio(&state, track_id)?;
        if buf.frames() == 0 {
            return Err("la pista está vacía".to_string());
        }
        let buf = audio::resample(&buf, rate).map_err(err)?;
        let path = dir.join("cache").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
        audio::write_wav_f32(&path, &buf).map_err(err)?;
        Ok(path)
    })
    .await
    .map_err(err)?
}

fn notes_result_path(dir: &Path, track_id: Uuid) -> PathBuf {
    dir.join("cache").join(format!("notes_{track_id}.json"))
}

// ---- Audio a MIDI -------------------------------------------------------------------

/// Lanza la transcripción de una pista a notas.
#[tauri::command]
pub async fn notes_start(
    app: AppHandle,
    track_id: Uuid,
    mode: String,
    sensitivity: Option<f64>,
    device: Option<String>,
) -> Result<Value, String> {
    let input = write_track_input(&app, track_id, format!("notes_input_{track_id}.wav"), NOTES_RATE).await?;
    let state = app.state::<AppState>();
    let (dir, _) = session_snapshot(&state)?;
    // Sensibilidad 0–1 (0,5 = valores por defecto de Basic Pitch): más alta, más notas.
    let s = sensitivity.unwrap_or(0.5).clamp(0.0, 1.0);
    let params = json!({
        "input": input,
        "output": notes_result_path(&dir, track_id),
        "mode": if mode == "mono" { "mono" } else { "poly" },
        "onset_threshold": 0.7 - 0.4 * s,
        "frame_threshold": 0.45 - 0.3 * s,
        "device": device.unwrap_or_else(|| "auto".into()),
    });
    state.worker.clone().call("tasks.run", json!({ "task": "transcribe_notes", "params": params })).await
}

/// Guarda en el proyecto las notas recién transcritas de una pista.
#[tauri::command]
pub fn notes_apply(state: State<'_, AppState>, track_id: Uuid) -> Result<Transcription, String> {
    let (dir, _) = session_snapshot(&state)?;
    let path = notes_result_path(&dir, track_id);
    let result = NotesResult::load(&path).map_err(err)?;
    let mut t = Transcription::load(&dir).map_err(err)?.unwrap_or_default();
    t.set_track(TrackNotes { track_id, mode: result.mode, notes: result.notes });
    t.save(&dir).map_err(err)?;
    let _ = std::fs::remove_file(path);
    Ok(t)
}

/// Guarda las notas editadas a mano (piano roll).
#[tauri::command]
pub fn notes_update(state: State<'_, AppState>, track_id: Uuid, notes: Vec<Note>) -> Result<Transcription, String> {
    let (dir, project) = session_snapshot(&state)?;
    if !project.tracks.iter().any(|t| t.id == track_id) {
        return Err("pista desconocida".into());
    }
    let mut t = Transcription::load(&dir).map_err(err)?.unwrap_or_default();
    let mode = t.track(track_id).map_or_else(|| "poly".to_string(), |x| x.mode.clone());
    t.set_track(TrackNotes { track_id, mode, notes });
    t.save(&dir).map_err(err)?;
    Ok(t)
}

#[tauri::command]
pub fn notes_remove(state: State<'_, AppState>, track_id: Uuid) -> Result<Transcription, String> {
    let (dir, _) = session_snapshot(&state)?;
    let mut t = Transcription::load(&dir).map_err(err)?.unwrap_or_default();
    t.retain_tracks(|id| id != track_id);
    t.save(&dir).map_err(err)?;
    Ok(t)
}

/// Programa General MIDI según el tipo de pista.
fn gm_program(kind: TrackKind) -> u8 {
    match kind {
        TrackKind::Bass => 33,                             // bajo eléctrico (dedo)
        TrackKind::Guitar => 25,                           // guitarra acústica (acero)
        TrackKind::Vocals | TrackKind::BackingVocals => 53, // voz «oohs»
        TrackKind::Other => 48,                            // cuerdas
        _ => 0,                                            // piano
    }
}

/// Exporta las notas (de todas las pistas transcritas o de una) a MIDI
/// estándar, con el mapa de tempo de los pulsos detectados si los hay.
#[tauri::command]
pub fn notes_export_midi(state: State<'_, AppState>, path: String, track_id: Option<Uuid>) -> Result<String, String> {
    let (dir, project) = session_snapshot(&state)?;
    let t = Transcription::load(&dir).map_err(err)?.ok_or("no hay notas transcritas")?;
    let analysis = Analysis::load(&dir).ok().flatten();
    let (tempo, bpb) = match &analysis {
        Some(a) if a.beats.len() >= 2 => {
            let first = a.downbeats.first().and_then(|d| a.beats.iter().position(|b| (b - d).abs() < 0.03));
            (TempoMap::from_beats(&a.beats, first, a.beats_per_bar), a.beats_per_bar)
        }
        _ => (TempoMap::constant(project.practice.bpm), project.practice.beats_per_bar),
    };
    let parts: Vec<(String, u8, &[Note])> = t
        .tracks
        .iter()
        .filter(|tn| track_id.is_none_or(|id| id == tn.track_id) && !tn.notes.is_empty())
        .filter_map(|tn| {
            let info = project.tracks.iter().find(|x| x.id == tn.track_id)?;
            Some((info.name.clone(), gm_program(info.kind), tn.notes.as_slice()))
        })
        .collect();
    if parts.is_empty() {
        return Err("no hay notas que exportar".into());
    }
    let parts: Vec<MidiPart> = parts.iter().map(|(n, p, notes)| MidiPart { name: n, program: *p, notes }).collect();
    let bytes = write_midi(&parts, &tempo, bpb, &project.name);
    let path = Path::new(&path).with_extension("mid");
    std::fs::write(&path, bytes).map_err(err)?;
    Ok(path.display().to_string())
}

/// Añade una pista generada al proyecto y lo recarga en el motor.
fn add_generated_track(state: &AppState, rel: String, name: String, mute_source: Option<Uuid>) -> Result<ProjectView, String> {
    let engine = state.engine()?;
    let (dir, mut project) = session_snapshot(state)?;
    let before = engine.state();
    engine.pause().map_err(err)?;
    project.tracks.retain(|t| t.file != rel);
    if let Some(src) = mute_source {
        if let Some(t) = project.tracks.iter_mut().find(|t| t.id == src) {
            t.mute = true;
        }
    }
    project.tracks.push(TrackInfo::new(name, TrackKind::Generated, rel));
    project.save(&dir).map_err(err)?;
    let view = load_into_engine(state, dir, project)?;
    engine.seek(before.position_frames).map_err(err)?;
    Ok(view)
}

/// Sintetiza las notas de una pista y las añade como pista nueva para escucharlas.
#[tauri::command]
pub async fn notes_render(app: AppHandle, track_id: Uuid) -> Result<ProjectView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let rate = state.engine()?.sample_rate();
        let (dir, project) = session_snapshot(&state)?;
        let t = Transcription::load(&dir).map_err(err)?.ok_or("no hay notas transcritas")?;
        let tn = t.track(track_id).ok_or("esta pista no tiene notas")?;
        let info = project.tracks.iter().find(|x| x.id == track_id).ok_or("pista desconocida")?;
        let timbre = match info.kind {
            TrackKind::Bass => Timbre::Bass,
            TrackKind::Vocals | TrackKind::BackingVocals => Timbre::Lead,
            _ => Timbre::Keys,
        };
        let buf = synthesize(&tn.notes, rate, None, timbre);
        let rel = format!("{MIDI_DIR}{track_id}.flac");
        let path = dir.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
        export_audio(&path, &buf, ExportFormat::Flac24).map_err(err)?;
        add_generated_track(&state, rel, format!("{} · MIDI", info.name), None)
    })
    .await
    .map_err(err)?
}

// ---- Afinación vocal ------------------------------------------------------------------

/// Lanza el seguimiento del tono de una pista de voz.
#[tauri::command]
pub async fn pitch_start(app: AppHandle, track_id: Uuid) -> Result<Value, String> {
    let input = write_track_input(&app, track_id, "pitch_input.wav".into(), PITCH_RATE).await?;
    let state = app.state::<AppState>();
    let (dir, _) = session_snapshot(&state)?;
    let params = json!({ "input": input, "output": dir.join("cache").join(format!("pitch_{track_id}.json")) });
    state.worker.clone().call("tasks.run", json!({ "task": "track_pitch", "params": params })).await
}

/// Guarda la curva de tono recién calculada (una por proyecto).
#[tauri::command]
pub fn pitch_apply(state: State<'_, AppState>, track_id: Uuid) -> Result<PitchCurve, String> {
    let (dir, _) = session_snapshot(&state)?;
    let path = dir.join("cache").join(format!("pitch_{track_id}.json"));
    let text = std::fs::read_to_string(&path).map_err(|_| "el seguimiento del tono no generó resultados".to_string())?;
    let mut curve: PitchCurve = serde_json::from_str(&text).map_err(err)?;
    curve.track_id = Some(track_id);
    curve.save(&dir).map_err(err)?;
    let _ = std::fs::remove_file(path);
    Ok(curve)
}

/// Crea una versión afinada de la voz como pista nueva (la original queda silenciada).
#[tauri::command]
pub async fn tune_render(app: AppHandle, settings: TuneSettings) -> Result<ProjectView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let (dir, _) = session_snapshot(&state)?;
        let curve = PitchCurve::load(&dir).map_err(err)?.ok_or("primero analiza la afinación de la voz")?;
        let track_id = curve.track_id.ok_or("la curva de tono no indica su pista")?;
        let (buf, info) = render_track_audio(&state, track_id)?;
        let tuned = tune(&buf, &curve, &settings);
        let rel = format!("{TUNED_DIR}{track_id}.flac");
        let path = dir.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
        export_audio(&path, &tuned, ExportFormat::Flac24).map_err(err)?;
        add_generated_track(&state, rel, format!("{} · afinada", info.name), Some(track_id))
    })
    .await
    .map_err(err)?
}

// ---- Instrumentos -----------------------------------------------------------------------

#[tauri::command]
pub async fn instruments_start(app: AppHandle, model_id: Option<String>, device: Option<String>) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let (dir, project) = session_snapshot(&state)?;
    let input = write_mix_input(&dir, &project, "instruments_input.wav", INSTRUMENTS_RATE).await?;
    let mut params = json!({
        "input": input,
        "output": InstrumentReport::path(&dir),
        "device": device.unwrap_or_else(|| "auto".into()),
    });
    if let Some(m) = model_id {
        params["model_id"] = json!(m);
    }
    state.worker.clone().call("tasks.run", json!({ "task": "detect_instruments", "params": params })).await
}

#[tauri::command]
pub fn instruments_apply(state: State<'_, AppState>) -> Result<InstrumentReport, String> {
    let (dir, _) = session_snapshot(&state)?;
    InstrumentReport::load(&dir).map_err(err)?.ok_or_else(|| "la detección no generó resultados".into())
}

/// Transcripción, curva de tono e instrumentos del proyecto (para `ProjectView`).
pub fn load_all(dir: &Path, project: &mn_core::project::Project) -> (Option<Transcription>, Option<PitchCurve>, Option<InstrumentReport>) {
    let warn = |what: &str, e: mn_core::CoreError| eprintln!("[musicnova] {what} de {}: {e}", dir.display());
    let notes = Transcription::load(dir).unwrap_or_else(|e| {
        warn("notas", e);
        None
    });
    let notes = notes.map(|mut t| {
        t.retain_tracks(|id| project.tracks.iter().any(|x| x.id == id));
        t
    });
    let pitch = PitchCurve::load(dir)
        .unwrap_or_else(|e| {
            warn("curva de tono", e);
            None
        })
        .filter(|c| c.track_id.is_some_and(|id| project.tracks.iter().any(|x| x.id == id)));
    let instruments = InstrumentReport::load(dir).unwrap_or_else(|e| {
        warn("instrumentos", e);
        None
    });
    (notes, pitch, instruments)
}
