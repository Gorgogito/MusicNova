//! Aplicación de escritorio MusicNova: une la interfaz, el motor de audio,
//! los proyectos y el proceso de IA.

mod worker;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mn_core::analysis::Analysis;
use mn_core::audio::{self, AudioBuffer};
use mn_core::export::{export_audio, ExportFormat};
use mn_core::ffmpeg;
use mn_core::mix::{mixdown, MixInput};
use mn_core::peaks::{compute_peaks, Peaks};
use mn_core::project::{sanitize_dir_name, BeatSource, PracticeSettings, Project, TrackInfo, TrackKind, PROJECT_FILE};
use mn_engine::{Engine, TrackId};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;
use worker::{Worker, WorkerStatus};

#[derive(Default)]
struct Session {
    dir: Option<PathBuf>,
    project: Option<Project>,
    engine_ids: HashMap<Uuid, TrackId>,
    /// Audio de cada pista a la frecuencia del motor (compartido con el motor).
    buffers: HashMap<Uuid, Arc<Vec<f32>>>,
    /// Frecuencia original de la mezcla; se usa al exportar.
    native_rate: u32,
    analysis: Option<Analysis>,
}

struct AppState {
    engine: Option<Engine>,
    engine_error: Option<String>,
    session: Mutex<Session>,
    worker: Arc<Worker>,
    projects_root: PathBuf,
}

impl AppState {
    fn engine(&self) -> Result<&Engine, String> {
        self.engine.as_ref().ok_or_else(|| {
            format!("motor de audio no disponible: {}", self.engine_error.as_deref().unwrap_or("desconocido"))
        })
    }
}

#[derive(Serialize)]
struct AppInfo {
    version: &'static str,
    sample_rate: Option<u32>,
    device: Option<String>,
    engine_error: Option<String>,
    projects_root: String,
    supported_extensions: Vec<&'static str>,
    ffmpeg_available: bool,
}

#[derive(Serialize)]
struct TrackView {
    #[serde(flatten)]
    info: TrackInfo,
    duration: f64,
    peaks: Peaks,
}

#[derive(Serialize)]
struct ProjectView {
    dir: String,
    project: Project,
    tracks: Vec<TrackView>,
    sample_rate: u32,
    analysis: Option<Analysis>,
}

#[derive(Serialize)]
struct ProjectSummary {
    dir: String,
    name: String,
    modified: chrono::DateTime<chrono::Utc>,
    tracks: usize,
}

#[derive(Clone, Serialize)]
struct TransportEvent {
    position: f64,
    duration: f64,
    playing: bool,
    counting_in: bool,
    loop_count: u64,
    peak_l: f32,
    peak_r: f32,
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        sample_rate: state.engine.as_ref().map(|e| e.sample_rate()),
        device: state.engine.as_ref().map(|e| e.device_name().to_string()),
        engine_error: state.engine_error.clone(),
        projects_root: state.projects_root.display().to_string(),
        supported_extensions: audio::SUPPORTED_EXTENSIONS
            .iter()
            .chain(ffmpeg::FFMPEG_ONLY_EXTENSIONS)
            .copied()
            .collect(),
        ffmpeg_available: ffmpeg::locate().is_some(),
    }
}

/// Decodifica y prepara (remuestreo + picos) todas las pistas en paralelo.
fn prepare_tracks(dir: &Path, project: &Project, rate: u32) -> Result<Vec<(AudioBuffer, Peaks, u32)>, String> {
    let results: Vec<Result<(AudioBuffer, Peaks, u32), String>> = std::thread::scope(|s| {
        let handles: Vec<_> = project
            .tracks
            .iter()
            .map(|t| {
                let path = project.track_path(dir, t);
                s.spawn(move || {
                    let decoded = audio::decode_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    let buf = audio::resample(&decoded, rate).map_err(err)?;
                    let peaks = compute_peaks(&buf, (rate / 100) as usize);
                    Ok((buf, peaks, decoded.sample_rate))
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("fallo al decodificar".into()))).collect()
    });
    results.into_iter().collect()
}

/// Carga un proyecto en el motor y devuelve su vista para la interfaz.
fn load_into_engine(state: &AppState, dir: PathBuf, project: Project) -> Result<ProjectView, String> {
    let engine = state.engine()?;
    let rate = engine.sample_rate();
    let prepared = prepare_tracks(&dir, &project, rate)?;

    engine.clear().map_err(err)?;
    let mut ids = HashMap::new();
    let mut buffers = HashMap::new();
    let mut native_rate = rate;
    let mut tracks = Vec::new();
    for (info, (buf, peaks, src_rate)) in project.tracks.iter().zip(prepared) {
        let duration = buf.duration_secs();
        if info.kind == TrackKind::Mix || tracks.is_empty() {
            native_rate = src_rate;
        }
        let data = Arc::new(buf.data);
        buffers.insert(info.id, data.clone());
        let id = engine.add_track(data, rate).map_err(err)?;
        engine.set_gain_db(id, info.gain_db).map_err(err)?;
        engine.set_pan(id, info.pan).map_err(err)?;
        engine.set_mute(id, info.mute).map_err(err)?;
        engine.set_solo(id, info.solo).map_err(err)?;
        ids.insert(info.id, id);
        tracks.push(TrackView { info: info.clone(), duration, peaks });
    }

    // Un análisis ilegible no impide abrir el proyecto: se puede repetir.
    let analysis = Analysis::load(&dir).unwrap_or_else(|e| {
        eprintln!("[musicnova] análisis de {}: {e}", dir.display());
        None
    });
    let length = buffers.values().map(|b| (b.len() / 2) as u64).max().unwrap_or(0);
    apply_practice(engine, &project.practice, None, analysis.as_ref(), length)?;

    let view = ProjectView {
        dir: dir.display().to_string(),
        project: project.clone(),
        tracks,
        sample_rate: rate,
        analysis: analysis.clone(),
    };
    let mut s = state.session.lock().unwrap();
    s.dir = Some(dir);
    s.project = Some(project);
    s.engine_ids = ids;
    s.buffers = buffers;
    s.native_rate = native_rate;
    s.analysis = analysis;
    Ok(view)
}

fn unique_dir(root: &Path, name: &str) -> PathBuf {
    let base = sanitize_dir_name(name);
    let mut dir = root.join(&base);
    let mut n = 2;
    while dir.exists() {
        dir = root.join(format!("{base} ({n})"));
        n += 1;
    }
    dir
}

#[tauri::command]
async fn import_audio(app: AppHandle, path: String) -> Result<ProjectView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let src = PathBuf::from(&path);
        let ext = src.extension().and_then(|e| e.to_str()).unwrap_or("wav").to_lowercase();
        // Valida el archivo antes de crear nada en disco. Vídeo y formatos que
        // symphonia no conoce pasan por FFmpeg.
        let native_error = if ffmpeg::FFMPEG_ONLY_EXTENSIONS.contains(&ext.as_str()) {
            Some("formato no soportado de forma nativa".to_string())
        } else {
            audio::decode_file(&src).err().map(|e| e.to_string())
        };
        let ffmpeg_exe = match &native_error {
            None => None,
            Some(e) => Some(ffmpeg::locate().ok_or_else(|| {
                format!(
                    "No se pudo leer el archivo ({e}). Para vídeo y otros formatos instala FFmpeg \
                     (gratuito) desde «Modelos IA» → Herramientas."
                )
            })?),
        };

        let name = src.file_stem().and_then(|s| s.to_str()).unwrap_or("Proyecto").to_string();
        let dir = unique_dir(&state.projects_root, &name);
        Project::create_dirs(&dir).map_err(err)?;
        let rel = match &ffmpeg_exe {
            Some(exe) => {
                let rel = "audio/original.wav".to_string();
                if let Err(e) = ffmpeg::convert_to_wav(exe, &src, &dir.join(&rel)) {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(e.to_string());
                }
                rel
            }
            None => {
                let rel = format!("audio/original.{ext}");
                std::fs::copy(&src, dir.join(&rel)).map_err(err)?;
                rel
            }
        };

        let mut project = Project::new(name);
        project.source_path = Some(path);
        project.tracks.push(TrackInfo::new("Mezcla original", TrackKind::Mix, rel));
        project.save(&dir).map_err(err)?;
        load_into_engine(&state, dir, project)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
async fn open_project(app: AppHandle, dir: String) -> Result<ProjectView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let dir = PathBuf::from(dir);
        let project = Project::load(&dir).map_err(err)?;
        load_into_engine(&state, dir, project)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn list_projects(state: State<'_, AppState>) -> Vec<ProjectSummary> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&state.projects_root) {
        for e in entries.flatten() {
            let dir = e.path();
            if !dir.join(PROJECT_FILE).is_file() {
                continue;
            }
            if let Ok(p) = Project::load(&dir) {
                out.push(ProjectSummary { dir: dir.display().to_string(), name: p.name, modified: p.modified, tracks: p.tracks.len() });
            }
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

/// Aplica los ajustes de práctica al motor. Con `prev`, la rejilla de pulsos
/// solo se regenera si cambió su origen, el tempo, el desplazamiento o el compás.
fn apply_practice(
    engine: &Engine,
    p: &PracticeSettings,
    prev: Option<&PracticeSettings>,
    analysis: Option<&Analysis>,
    length: u64,
) -> Result<(), String> {
    let rate = engine.sample_rate() as f64;
    engine.set_tempo(p.tempo).map_err(err)?;
    engine.set_pitch(p.pitch).map_err(err)?;
    let (start, end) = ((p.loop_start.max(0.0) * rate) as u64, (p.loop_end.max(0.0) * rate) as u64);
    engine.set_loop(p.loop_enabled, start, end).map_err(err)?;
    engine.set_metronome(p.metronome, p.metronome_gain_db).map_err(err)?;
    let grid_changed = prev.map_or(true, |q| {
        q.bpm != p.bpm || q.beat_offset != p.beat_offset || q.beats_per_bar != p.beats_per_bar || q.beat_source != p.beat_source
    });
    if grid_changed {
        match (p.beat_source, analysis) {
            (BeatSource::Detected, Some(a)) if !a.beats.is_empty() => {
                let (frames, accents) = a.beat_grid(engine.sample_rate());
                engine.set_beats_with_accents(frames, accents).map_err(err)?;
            }
            _ => {
                let beats = p.manual_beats(engine.sample_rate(), length);
                engine.set_beats(beats, p.beats_per_bar, p.manual_downbeat_offset()).map_err(err)?;
            }
        }
    }
    Ok(())
}

fn sanitize_practice(mut p: PracticeSettings) -> PracticeSettings {
    p.tempo = p.tempo.clamp(0.25, 2.0);
    p.pitch = p.pitch.clamp(-12.0, 12.0);
    p.bpm = p.bpm.clamp(20.0, 400.0);
    p.beats_per_bar = p.beats_per_bar.clamp(1, 16);
    p.count_in_bars = p.count_in_bars.min(4);
    p.metronome_gain_db = p.metronome_gain_db.clamp(-40.0, 6.0);
    if p.loop_end <= p.loop_start {
        p.loop_enabled = false;
    }
    p.trainer_start = p.trainer_start.clamp(0.25, 2.0);
    p.trainer_target = p.trainer_target.clamp(0.25, 2.0);
    p.trainer_step = p.trainer_step.clamp(0.01, 0.5);
    p.trainer_every = p.trainer_every.clamp(1, 32);
    p.capo = p.capo.min(12);
    p.markers.sort_by(|a, b| a.time.total_cmp(&b.time));
    p
}

/// Actualiza los ajustes de práctica (tempo, tono, bucle, metrónomo, marcadores...).
#[tauri::command]
fn practice_set(state: State<'_, AppState>, settings: PracticeSettings) -> Result<PracticeSettings, String> {
    let engine = state.engine()?;
    let settings = sanitize_practice(settings);
    let mut guard = state.session.lock().unwrap();
    let s = &mut *guard;
    let length = s.buffers.values().map(|b| (b.len() / 2) as u64).max().unwrap_or(0);
    let project = s.project.as_mut().ok_or("no hay ningún proyecto abierto")?;
    apply_practice(engine, &settings, Some(&project.practice), s.analysis.as_ref(), length)?;
    project.practice = settings.clone();
    if let Some(dir) = s.dir.clone() {
        project.save(&dir).map_err(err)?;
    }
    Ok(settings)
}

#[tauri::command]
fn transport_play(state: State<'_, AppState>, count_in: Option<bool>) -> Result<(), String> {
    let engine = state.engine()?;
    let practice = state.session.lock().unwrap().project.as_ref().map(|p| p.practice.clone());
    match practice {
        Some(p) if count_in.unwrap_or(true) && p.count_in_bars > 0 => {
            engine.play_with_count_in(p.count_in_bars * p.beats_per_bar, p.bpm).map_err(err)
        }
        _ => engine.play().map_err(err),
    }
}

#[tauri::command]
fn transport_pause(state: State<'_, AppState>) -> Result<(), String> {
    state.engine()?.pause().map_err(err)
}

#[tauri::command]
fn transport_seek(state: State<'_, AppState>, seconds: f64) -> Result<(), String> {
    let engine = state.engine()?;
    engine.seek((seconds.max(0.0) * engine.sample_rate() as f64) as u64).map_err(err)
}

#[tauri::command]
fn track_update(
    state: State<'_, AppState>,
    track_id: Uuid,
    gain_db: Option<f32>,
    pan: Option<f32>,
    mute: Option<bool>,
    solo: Option<bool>,
) -> Result<TrackInfo, String> {
    let engine = state.engine()?;
    let mut guard = state.session.lock().unwrap();
    let s = &mut *guard;
    let id = *s.engine_ids.get(&track_id).ok_or("pista desconocida")?;
    let project = s.project.as_mut().ok_or("no hay proyecto abierto")?;
    let t = project.tracks.iter_mut().find(|t| t.id == track_id).ok_or("pista desconocida")?;
    if let Some(v) = gain_db {
        t.gain_db = v.clamp(-60.0, 12.0);
        engine.set_gain_db(id, t.gain_db).map_err(err)?;
    }
    if let Some(v) = pan {
        t.pan = v.clamp(-1.0, 1.0);
        engine.set_pan(id, t.pan).map_err(err)?;
    }
    if let Some(v) = mute {
        t.mute = v;
        engine.set_mute(id, v).map_err(err)?;
    }
    if let Some(v) = solo {
        t.solo = v;
        engine.set_solo(id, v).map_err(err)?;
    }
    let info = t.clone();
    if let Some(dir) = s.dir.clone() {
        project.save(&dir).map_err(err)?;
    }
    Ok(info)
}

#[tauri::command]
async fn worker_call(state: State<'_, AppState>, method: String, params: Option<Value>) -> Result<Value, String> {
    let worker = state.worker.clone();
    worker.call(&method, params.unwrap_or(Value::Object(Default::default()))).await
}

#[tauri::command]
fn worker_status(state: State<'_, AppState>) -> WorkerStatus {
    state.worker.status()
}

#[tauri::command]
fn worker_restart(state: State<'_, AppState>) -> Result<(), String> {
    state.worker.stop();
    state.worker.start()
}

/// Frecuencia a la que trabajan los modelos Demucs.
const SEPARATION_RATE: u32 = 44_100;

fn session_snapshot(state: &AppState) -> Result<(PathBuf, Project), String> {
    let s = state.session.lock().unwrap();
    match (&s.dir, &s.project) {
        (Some(d), Some(p)) => Ok((d.clone(), p.clone())),
        _ => Err("no hay ningún proyecto abierto".into()),
    }
}

/// Decodifica la mezcla original y la escribe como WAV flotante en `cache/<name>`
/// (el proceso de IA solo necesita leer WAV).
async fn write_mix_input(dir: &Path, project: &Project, name: &str, rate: u32) -> Result<PathBuf, String> {
    let mix = project
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Mix)
        .or_else(|| project.tracks.first())
        .ok_or("el proyecto no tiene pistas")?;
    let src = project.track_path(dir, mix);
    let input = dir.join("cache").join(name);
    let input_clone = input.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let decoded = audio::decode_file(&src).map_err(err)?;
        let buf = audio::resample(&decoded, rate).map_err(err)?;
        std::fs::create_dir_all(input_clone.parent().unwrap()).map_err(err)?;
        audio::write_wav_f32(&input_clone, &buf).map_err(err)
    })
    .await
    .map_err(err)??;
    Ok(input)
}

/// Prepara la mezcla original y lanza la separación en el proceso de IA.
#[tauri::command]
async fn separate_start(app: AppHandle, model_id: String, device: Option<String>) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let (dir, project) = session_snapshot(&state)?;
    let input = write_mix_input(&dir, &project, "separation_input.wav", SEPARATION_RATE).await?;

    let out_dir = dir.join("audio").join("stems").join(&model_id);
    let worker = state.worker.clone();
    worker
        .call(
            "tasks.run",
            json!({
                "task": "separate",
                "params": {
                    "model_id": model_id,
                    "input": input,
                    "output_dir": out_dir,
                    "device": device.unwrap_or_else(|| "auto".into()),
                }
            }),
        )
        .await
}

#[derive(Deserialize)]
struct StemOutput {
    name: String,
    label: String,
    file: String,
}

fn stem_kind(name: &str) -> (TrackKind, u8) {
    match name {
        "vocals" => (TrackKind::Vocals, 0),
        "drums" => (TrackKind::Drums, 1),
        "bass" => (TrackKind::Bass, 2),
        "guitar" => (TrackKind::Guitar, 3),
        "piano" => (TrackKind::Piano, 4),
        "instrumental" => (TrackKind::Other, 1),
        _ => (TrackKind::Other, 5),
    }
}

/// Sustituye los stems anteriores por los recién separados y recarga el proyecto.
#[tauri::command]
async fn separate_apply(app: AppHandle, stems: Vec<StemOutput>) -> Result<ProjectView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let engine = state.engine()?;
        let (dir, mut project) = session_snapshot(&state)?;
        let before = engine.state();
        engine.pause().map_err(err)?;

        project.tracks.retain(|t| !t.file.starts_with("audio/stems/"));
        for t in project.tracks.iter_mut().filter(|t| t.kind == TrackKind::Mix) {
            t.mute = true;
        }
        let mut stems = stems;
        stems.sort_by_key(|s| stem_kind(&s.name).1);
        for s in stems {
            let rel = Path::new(&s.file)
                .strip_prefix(&dir)
                .map_err(|_| format!("el stem {} está fuera del proyecto", s.file))?
                .to_string_lossy()
                .replace('\\', "/");
            project.tracks.push(TrackInfo::new(s.label, stem_kind(&s.name).0, rel));
        }
        project.save(&dir).map_err(err)?;
        let view = load_into_engine(&state, dir, project)?;
        engine.seek(before.position_frames).map_err(err)?;
        Ok(view)
    })
    .await
    .map_err(err)?
}

/// Frecuencia de trabajo del análisis musical.
const ANALYSIS_RATE: u32 = 22_050;

/// Lanza el análisis musical. Si hay stems, se usan para los acordes.
#[tauri::command]
async fn analyze_start(app: AppHandle, device: Option<String>, beat_model_id: Option<String>) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let (dir, project) = session_snapshot(&state)?;
    let input = write_mix_input(&dir, &project, "analysis_input.wav", ANALYSIS_RATE).await?;
    // Nombre del stem = nombre del archivo que escribió la separación (bass.flac -> "bass").
    let stems: serde_json::Map<String, Value> = project
        .tracks
        .iter()
        .filter(|t| t.file.starts_with("audio/stems/"))
        .filter_map(|t| {
            let path = project.track_path(&dir, t);
            let name = path.file_stem()?.to_str()?.to_string();
            Some((name, json!(path)))
        })
        .collect();
    let mut params = json!({
        "input": input,
        "stems": stems,
        "output": Analysis::path(&dir),
        "device": device.unwrap_or_else(|| "auto".into()),
    });
    if let Some(m) = beat_model_id {
        params["beat_model_id"] = json!(m);
    }
    state.worker.clone().call("tasks.run", json!({ "task": "analyze", "params": params })).await
}

#[derive(Serialize)]
struct AnalysisApplied {
    analysis: Analysis,
    practice: PracticeSettings,
}

/// Carga el análisis recién terminado y alinea el metrónomo con él.
#[tauri::command]
fn analysis_apply(state: State<'_, AppState>) -> Result<AnalysisApplied, String> {
    let engine = state.engine()?;
    let mut guard = state.session.lock().unwrap();
    let s = &mut *guard;
    let dir = s.dir.clone().ok_or("no hay ningún proyecto abierto")?;
    let analysis = Analysis::load(&dir).map_err(err)?.ok_or("el análisis no generó resultados")?;
    let length = s.buffers.values().map(|b| (b.len() / 2) as u64).max().unwrap_or(0);
    let project = s.project.as_mut().ok_or("no hay ningún proyecto abierto")?;
    let mut p = project.practice.clone();
    p.beat_source = BeatSource::Detected;
    if analysis.bpm > 0.0 {
        p.bpm = (analysis.bpm * 10.0).round() / 10.0;
    }
    p.beats_per_bar = analysis.beats_per_bar.max(1);
    p.beat_offset = analysis.first_downbeat();
    let p = sanitize_practice(p);
    // `None` como anterior: fuerza a regenerar la rejilla con los pulsos nuevos.
    apply_practice(engine, &p, None, Some(&analysis), length)?;
    project.practice = p.clone();
    project.save(&dir).map_err(err)?;
    s.analysis = Some(analysis.clone());
    Ok(AnalysisApplied { analysis, practice: p })
}

/// Guarda correcciones manuales del análisis (acordes, secciones, tonalidad).
#[tauri::command]
fn analysis_update(state: State<'_, AppState>, analysis: Analysis) -> Result<Analysis, String> {
    let engine = state.engine()?;
    let mut guard = state.session.lock().unwrap();
    let s = &mut *guard;
    let dir = s.dir.clone().ok_or("no hay ningún proyecto abierto")?;
    let mut analysis = analysis;
    analysis.chords.sort_by(|a, b| a.start.total_cmp(&b.start));
    analysis.sections.sort_by(|a, b| a.start.total_cmp(&b.start));
    analysis.save(&dir).map_err(err)?;
    let beats_changed =
        s.analysis.as_ref().map_or(true, |a| a.beats != analysis.beats || a.downbeats != analysis.downbeats);
    if beats_changed {
        let length = s.buffers.values().map(|b| (b.len() / 2) as u64).max().unwrap_or(0);
        if let Some(p) = s.project.as_ref() {
            apply_practice(engine, &p.practice, None, Some(&analysis), length)?;
        }
    }
    s.analysis = Some(analysis.clone());
    Ok(analysis)
}

#[derive(Serialize)]
struct MixExport {
    path: String,
    peak_db: f32,
    sample_rate: u32,
}

/// Exporta la mezcla tal como suena (volumen, panorama, silencio y solo).
#[tauri::command]
async fn export_mix(app: AppHandle, path: String, format: ExportFormat) -> Result<MixExport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let rate = state.engine()?.sample_rate();
        let (project, buffers, native_rate) = {
            let s = state.session.lock().unwrap();
            let p = s.project.clone().ok_or("no hay ningún proyecto abierto")?;
            (p, s.buffers.clone(), s.native_rate)
        };
        let inputs: Vec<MixInput> = project
            .tracks
            .iter()
            .filter_map(|t| {
                buffers
                    .get(&t.id)
                    .map(|d| MixInput { data: d, gain_db: t.gain_db, pan: t.pan, mute: t.mute, solo: t.solo })
            })
            .collect();
        let mixed = audio::resample(&mixdown(&inputs, rate), native_rate).map_err(err)?;
        let peak = mixed.data.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let path = Path::new(&path).with_extension(format.extension()).display().to_string();
        export_audio(Path::new(&path), &mixed, format).map_err(err)?;
        Ok(MixExport { path, peak_db: 20.0 * peak.max(1e-9).log10(), sample_rate: mixed.sample_rate })
    })
    .await
    .map_err(err)?
}

/// Exporta cada pista (sin efectos de mezcla) a su frecuencia original.
#[tauri::command]
async fn export_stems(app: AppHandle, dir: String, format: ExportFormat) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let (proj_dir, project) = session_snapshot(&state)?;
        let out_dir = PathBuf::from(dir);
        std::fs::create_dir_all(&out_dir).map_err(err)?;
        let stems: Vec<&TrackInfo> = project.tracks.iter().filter(|t| t.kind != TrackKind::Mix).collect();
        let selected: Vec<&TrackInfo> = if stems.is_empty() { project.tracks.iter().collect() } else { stems };
        let results: Vec<Result<String, String>> = std::thread::scope(|s| {
            let handles: Vec<_> = selected
                .iter()
                .map(|t| {
                    let src = project.track_path(&proj_dir, t);
                    let dest = out_dir.join(format!(
                        "{} - {}.{}",
                        sanitize_dir_name(&project.name),
                        sanitize_dir_name(&t.name),
                        format.extension()
                    ));
                    s.spawn(move || {
                        let buf = audio::decode_file(&src).map_err(err)?;
                        export_audio(&dest, &buf, format).map_err(err)?;
                        Ok(dest.display().to_string())
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("fallo al exportar".into()))).collect()
        });
        results.into_iter().collect()
    })
    .await
    .map_err(err)?
}

/// Publica el estado del transporte a ~30 Hz mientras suena (y a 2 Hz en reposo).
fn spawn_transport_events(app: AppHandle) {
    std::thread::Builder::new()
        .name("mn-transport".into())
        .spawn(move || {
            let mut idle_ticks = 0u32;
            loop {
                std::thread::sleep(Duration::from_millis(33));
                let state = app.state::<AppState>();
                let Some(engine) = state.engine.as_ref() else { return };
                let s = engine.state();
                idle_ticks += 1;
                if !s.playing && !s.counting_in && idle_ticks < 15 {
                    continue;
                }
                idle_ticks = 0;
                let rate = engine.sample_rate() as f64;
                let _ = app.emit(
                    "transport",
                    TransportEvent {
                        position: s.position_frames as f64 / rate,
                        duration: s.length_frames as f64 / rate,
                        playing: s.playing,
                        counting_in: s.counting_in,
                        loop_count: s.loop_count,
                        peak_l: s.peak_l,
                        peak_r: s.peak_r,
                    },
                );
            }
        })
        .expect("hilo de transporte");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let (engine, engine_error) = match Engine::start() {
                Ok(e) => (Some(e), None),
                Err(e) => {
                    eprintln!("[musicnova] motor de audio: {e}");
                    (None, Some(e.to_string()))
                }
            };
            let projects_root = app
                .path()
                .audio_dir()
                .or_else(|_| app.path().document_dir())
                .map(|d| d.join("MusicNova"))
                .map_err(|e| format!("no se encontró la carpeta de música: {e}"))?;
            std::fs::create_dir_all(&projects_root)?;

            let worker = Arc::new(Worker::new(app.handle().clone()));
            if let Err(e) = worker.start() {
                eprintln!("[musicnova] {e}");
            }
            app.manage(AppState { engine, engine_error, session: Mutex::new(Session::default()), worker, projects_root });
            spawn_transport_events(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            import_audio,
            open_project,
            list_projects,
            transport_play,
            transport_pause,
            transport_seek,
            track_update,
            worker_call,
            worker_status,
            worker_restart,
            separate_start,
            separate_apply,
            export_mix,
            export_stems,
            practice_set,
            analyze_start,
            analysis_apply,
            analysis_update,
        ])
        .build(tauri::generate_context!())
        .expect("error al iniciar MusicNova");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            handle.state::<AppState>().worker.stop();
        }
    });
}
