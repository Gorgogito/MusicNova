//! Aplicación de escritorio MusicNova: une la interfaz, el motor de audio,
//! los proyectos y el proceso de IA.

mod plugins;
mod generate;
mod instruments;
mod transcribe;
mod worker;

pub use plugins::scan_subprocess_main;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mn_core::analysis::Analysis;
use mn_core::automation::{sanitize_lanes, AutomationLane, ChannelAutomation};
use mn_core::audio::{self, AudioBuffer};
use mn_core::export::{export_audio, ExportFormat};
use mn_core::ffmpeg;
use mn_core::lyrics::{Lyrics, LyricsFormat};
use mn_core::mix::{mixdown, render_track, ClipFrames, MixInput};
use mn_core::peaks::{compute_peaks, Peaks};
use mn_core::master::{auto_master, with_auto_limiter};
use mn_core::project::{
    clip_frames, sanitize_clips, sanitize_dir_name, BeatSource, Clip, MasterSettings, PracticeSettings, Project, TrackInfo, TrackKind, PROJECT_FILE,
};
use mn_engine::{Engine, InputChannels, InputDeviceInfo, InputInfo, LatencyReport, TrackId};
use mn_fx::{loudness, EffectSlot};
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

#[derive(Clone, Serialize)]
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
    lyrics: Option<Lyrics>,
    /// Plugins que no se pudieron cargar (suenan sin procesar).
    plugin_errors: Vec<String>,
    /// Fase 6: notas transcritas, curva de tono de la voz e instrumentos.
    notes: Option<mn_core::transcription::Transcription>,
    pitch: Option<mn_core::transcription::PitchCurve>,
    instruments: Option<mn_core::transcription::InstrumentReport>,
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
    /// LUFS momentáneo y a corto plazo (`null` en silencio).
    lufs_m: Option<f64>,
    lufs_s: Option<f64>,
    recording: bool,
    /// Picos de la entrada abierta (`null` sin entrada).
    input_peak: Option<[f32; 2]>,
    /// Segundos grabados en la toma actual.
    recorded: f64,
}

fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
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
fn load_into_engine(state: &AppState, dir: PathBuf, mut project: Project) -> Result<ProjectView, String> {
    let engine = state.engine()?;
    let rate = engine.sample_rate();
    // El estado de los plugins del proyecto que se cierra no debe perderse.
    if let Err(e) = sync_plugin_states(state) {
        eprintln!("[musicnova] estado de plugins: {e}");
    }
    let prepared = prepare_tracks(&dir, &project, rate)?;
    // Proyectos anteriores a los clips (o clips que no encajan con el audio).
    let mut normalized = false;
    for (info, (buf, _, _)) in project.tracks.iter_mut().zip(&prepared) {
        normalized |= info.normalize_clips(buf.duration_secs());
    }
    if normalized {
        project.save(&dir).map_err(err)?;
    }

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
        let id = add_to_engine(engine, info, data, rate)?;
        ids.insert(info.id, id);
        tracks.push(TrackView { info: info.clone(), duration, peaks });
    }
    // Sidechains: cuando ya están todas las pistas en el motor.
    for info in &project.tracks {
        if let (Some(&id), Some(src)) = (ids.get(&info.id), sidechain_source(&info.effects)) {
            engine.set_sidechain(id, ids.get(&src).copied()).map_err(err)?;
        }
    }

    // Un análisis ilegible no impide abrir el proyecto: se puede repetir.
    let analysis = Analysis::load(&dir).unwrap_or_else(|e| {
        eprintln!("[musicnova] análisis de {}: {e}", dir.display());
        None
    });
    let lyrics = Lyrics::load(&dir).unwrap_or_else(|e| {
        eprintln!("[musicnova] letras de {}: {e}", dir.display());
        None
    });
    engine.set_effects(None, &project.master.effects).map_err(err)?;
    engine.set_master_gain_db(project.master.gain_db).map_err(err)?;
    engine
        .set_automation(None, ChannelAutomation::build(&project.master.automation, &project.master.effects, rate))
        .map_err(err)?;
    let length = timeline_length(&project, &buffers, rate);
    apply_practice(engine, &project.practice, None, analysis.as_ref(), length)?;

    let slot_ids = plugin_slot_ids(&project);
    mn_plugin::retain_instances(|id| slot_ids.iter().any(|s| s == id));
    let (notes, pitch, instruments) = transcribe::load_all(&dir, &project);
    let view = ProjectView {
        dir: dir.display().to_string(),
        project: project.clone(),
        tracks,
        sample_rate: rate,
        analysis: analysis.clone(),
        lyrics,
        plugin_errors: plugin_errors(&project),
        notes,
        pitch,
        instruments,
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

/// Efectos con plugin externo de todo el proyecto (pistas y máster).
fn plugin_slot_ids(project: &Project) -> Vec<String> {
    project
        .tracks
        .iter()
        .flat_map(|t| t.effects.iter())
        .chain(project.master.effects.iter())
        .filter(|s| s.plugin.is_some())
        .map(|s| s.id.clone())
        .collect()
}

fn plugin_errors(project: &Project) -> Vec<String> {
    let channels = project.tracks.iter().map(|t| (t.name.as_str(), &t.effects)).chain(std::iter::once(("Máster", &project.master.effects)));
    let mut out = Vec::new();
    for (name, effects) in channels {
        for s in effects {
            if let (Some(p), Some(e)) = (&s.plugin, mn_plugin::load_error(&s.id)) {
                out.push(format!("{name} · {}: {e}", p.name));
            }
        }
    }
    out
}

/// Copia al proyecto el estado actual de sus plugins y lo guarda. Nunca
/// espera al hilo principal con el cerrojo de la sesión tomado.
fn sync_plugin_states(state: &AppState) -> Result<(), String> {
    let mut effects: Vec<EffectSlot> = {
        let s = state.session.lock().unwrap();
        let Some(p) = s.project.as_ref() else { return Ok(()) };
        p.tracks.iter().flat_map(|t| t.effects.iter()).chain(p.master.effects.iter()).filter(|e| e.plugin.is_some()).cloned().collect()
    };
    if effects.is_empty() || !mn_plugin::sync_states(&mut effects) {
        return Ok(());
    }
    let mut guard = state.session.lock().unwrap();
    let s = &mut *guard;
    let (Some(p), Some(dir)) = (s.project.as_mut(), s.dir.clone()) else { return Ok(()) };
    for slot in p.tracks.iter_mut().flat_map(|t| t.effects.iter_mut()).chain(p.master.effects.iter_mut()) {
        if let Some(fresh) = effects.iter().find(|e| e.id == slot.id) {
            if let (Some(a), Some(b)) = (slot.plugin.as_mut(), fresh.plugin.as_ref()) {
                if a.path == b.path && a.id == b.id {
                    a.state = b.state.clone();
                }
            }
        }
    }
    p.save(&dir).map_err(err)
}

/// Añade una pista al motor con su desfase, volumen, panorama y efectos.
fn add_to_engine(engine: &Engine, info: &TrackInfo, data: Arc<Vec<f32>>, rate: u32) -> Result<TrackId, String> {
    let clips = clip_frames(&info.clips, rate, (data.len() / 2) as u64);
    let id = engine.add_track_with_clips(data, rate, clips).map_err(err)?;
    engine.set_gain_db(id, info.gain_db).map_err(err)?;
    engine.set_pan(id, info.pan).map_err(err)?;
    engine.set_mute(id, info.mute).map_err(err)?;
    engine.set_solo(id, info.solo).map_err(err)?;
    if !info.effects.is_empty() {
        engine.set_effects(Some(id), &info.effects).map_err(err)?;
    }
    if !info.automation.is_empty() {
        engine.set_automation(Some(id), ChannelAutomation::build(&info.automation, &info.effects, rate)).map_err(err)?;
    }
    Ok(id)
}

/// Efectos y automatización de una pista o, con `None`, del máster.
fn channel_mut(
    project: &mut Project,
    track: Option<Uuid>,
) -> Result<(&mut Vec<EffectSlot>, &mut Vec<AutomationLane>), String> {
    match track {
        Some(id) => {
            let t = project.tracks.iter_mut().find(|t| t.id == id).ok_or("pista desconocida")?;
            Ok((&mut t.effects, &mut t.automation))
        }
        None => Ok((&mut project.master.effects, &mut project.master.automation)),
    }
}

/// Duración de la línea de tiempo en frames del motor (pistas con su desfase).
fn timeline_length(project: &Project, buffers: &HashMap<Uuid, Arc<Vec<f32>>>, rate: u32) -> u64 {
    project
        .tracks
        .iter()
        .filter_map(|t| buffers.get(&t.id).map(|b| mn_core::mix::clips_end(&clip_frames(&t.clips, rate, (b.len() / 2) as u64))))
        .max()
        .unwrap_or(0)
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

/// Crea un proyecto sin audio (para empezar grabando).
#[tauri::command]
async fn create_project(app: AppHandle, name: String) -> Result<ProjectView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let name = if name.trim().is_empty() { "Proyecto nuevo".to_string() } else { name.trim().to_string() };
        let dir = unique_dir(&state.projects_root, &name);
        Project::create_dirs(&dir).map_err(err)?;
        let mut project = Project::new(name);
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
                // Diez minutos más allá del final: se puede grabar después de la última pista.
                let beats = p.manual_beats(engine.sample_rate(), length + engine.sample_rate() as u64 * 600);
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
    let length = s.project.as_ref().map_or(0, |p| timeline_length(p, &s.buffers, engine.sample_rate()));
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
async fn transport_pause(app: AppHandle) -> Result<(), String> {
    let recording = app.state::<AppState>().engine()?.is_recording();
    if recording {
        return finish_recording(app).await;
    }
    app.state::<AppState>().engine()?.pause().map_err(err)
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
    let length = s.project.as_ref().map_or(0, |p| timeline_length(p, &s.buffers, engine.sample_rate()));
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
        let length = s.project.as_ref().map_or(0, |p| timeline_length(p, &s.buffers, engine.sample_rate()));
        if let Some(p) = s.project.as_ref() {
            apply_practice(engine, &p.practice, None, Some(&analysis), length)?;
        }
    }
    s.analysis = Some(analysis.clone());
    Ok(analysis)
}

/// Frecuencia de Whisper.
const LYRICS_RATE: u32 = 16_000;

/// Lanza la transcripción de la letra, sobre el stem de voz si existe.
#[tauri::command]
async fn lyrics_start(
    app: AppHandle,
    model_id: Option<String>,
    language: Option<String>,
    device: Option<String>,
) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let (dir, project) = session_snapshot(&state)?;
    let vocals = project
        .tracks
        .iter()
        .filter(|t| t.file.starts_with("audio/stems/"))
        .map(|t| project.track_path(&dir, t))
        .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some("vocals"));
    let (input, source) = match vocals {
        Some(p) => (p, "vocals"),
        None => (write_mix_input(&dir, &project, "lyrics_input.wav", LYRICS_RATE).await?, "mix"),
    };
    let mut params = json!({
        "input": input,
        "source": source,
        "output": Lyrics::path(&dir),
        "language": language.unwrap_or_else(|| "auto".into()),
        "device": device.unwrap_or_else(|| "auto".into()),
    });
    if let Some(m) = model_id {
        params["model_id"] = json!(m);
    }
    state.worker.clone().call("tasks.run", json!({ "task": "transcribe", "params": params })).await
}

/// Carga la letra recién transcrita.
#[tauri::command]
fn lyrics_apply(state: State<'_, AppState>) -> Result<Lyrics, String> {
    let (dir, _) = session_snapshot(&state)?;
    Lyrics::load(&dir).map_err(err)?.ok_or_else(|| "la transcripción no generó resultados".into())
}

/// Guarda la letra corregida a mano.
#[tauri::command]
fn lyrics_update(state: State<'_, AppState>, lyrics: Lyrics) -> Result<Lyrics, String> {
    let (dir, _) = session_snapshot(&state)?;
    let mut lyrics = lyrics;
    lyrics.normalize();
    lyrics.save(&dir).map_err(err)?;
    Ok(lyrics)
}

/// Exporta la letra a LRC, LRC por palabra, SRT o texto.
#[tauri::command]
fn lyrics_export(state: State<'_, AppState>, path: String, format: LyricsFormat) -> Result<String, String> {
    let (dir, project) = session_snapshot(&state)?;
    let lyrics = Lyrics::load(&dir).map_err(err)?.ok_or("el proyecto no tiene letra")?;
    let path = Path::new(&path).with_extension(format.extension());
    std::fs::write(&path, lyrics.export(format, &project.name)).map_err(err)?;
    Ok(path.display().to_string())
}

#[derive(Serialize)]
struct MixExport {
    path: String,
    peak_db: f32,
    sample_rate: u32,
    /// Sonoridad integrada (LUFS), pico real (dBTP) y rango de sonoridad (LU).
    lufs: Option<f64>,
    true_peak_db: Option<f64>,
    lra: f64,
}

/// Clips en frames de cada pista cargada.
fn clip_sets(project: &Project, buffers: &HashMap<Uuid, Arc<Vec<f32>>>, rate: u32) -> HashMap<Uuid, Vec<ClipFrames>> {
    project
        .tracks
        .iter()
        .filter_map(|t| buffers.get(&t.id).map(|d| (t.id, clip_frames(&t.clips, rate, (d.len() / 2) as u64))))
        .collect()
}

/// Automatización de cada pista (las que tienen alguna curva activa).
fn automations(project: &Project, rate: u32) -> HashMap<Uuid, ChannelAutomation> {
    project
        .tracks
        .iter()
        .filter_map(|t| ChannelAutomation::build(&t.automation, &t.effects, rate).map(|a| (t.id, a)))
        .collect()
}

/// Carpeta (relativa al proyecto) de los masters generados.
const MASTER_DIR: &str = "audio/master/";

/// Pista que alimenta el sidechain de una cadena (la del primer efecto que lo use).
pub(crate) fn sidechain_source(effects: &[EffectSlot]) -> Option<Uuid> {
    effects.iter().filter(|s| s.enabled).find_map(|s| s.sidechain.as_deref()?.parse().ok())
}

/// Enlaza cada entrada de la mezcla (`ids[i]` es su pista) con la entrada de su sidechain.
fn link_sidechains(project: &Project, ids: &[Uuid], inputs: &mut [MixInput]) {
    for (i, inp) in inputs.iter_mut().enumerate() {
        let src = project.tracks.iter().find(|t| t.id == ids[i]).and_then(|t| sidechain_source(&t.effects));
        inp.sidechain = src.and_then(|s| ids.iter().position(|x| *x == s)).filter(|&j| j != i);
    }
}

/// Premezcla: las pistas con sus efectos, volumen y panorama, sin el bus
/// máster ni los masters ya generados. Devuelve el audio a la frecuencia del motor.
fn render_premaster(state: &AppState) -> Result<(AudioBuffer, Project, PathBuf, u32), String> {
    let rate = state.engine()?.sample_rate();
    sync_plugin_states(state)?;
    let (dir, project, buffers, native_rate) = {
        let s = state.session.lock().unwrap();
        let p = s.project.clone().ok_or("no hay ningún proyecto abierto")?;
        let d = s.dir.clone().ok_or("no hay ningún proyecto abierto")?;
        (d, p, s.buffers.clone(), s.native_rate)
    };
    let clip_sets = clip_sets(&project, &buffers, rate);
    let autos = automations(&project, rate);
    let mut inputs: Vec<MixInput> = project
        .tracks
        .iter()
        .filter(|t| !t.file.starts_with(MASTER_DIR))
        .filter_map(|t| {
            buffers.get(&t.id).map(|d| MixInput {
                data: d,
                clips: &clip_sets[&t.id],
                gain_db: t.gain_db,
                pan: t.pan,
                mute: t.mute,
                solo: t.solo,
                effects: &t.effects,
                automation: autos.get(&t.id),
                sidechain: None,
            })
        })
        .collect();
    let ids: Vec<Uuid> =
        project.tracks.iter().filter(|t| !t.file.starts_with(MASTER_DIR) && buffers.contains_key(&t.id)).map(|t| t.id).collect();
    link_sidechains(&project, &ids, &mut inputs);
    if inputs.is_empty() {
        return Err("no hay pistas que masterizar".into());
    }
    let mixed = mixdown(&inputs, &MasterSettings::default(), None, rate);
    Ok((mixed, project, dir, native_rate))
}

#[derive(Serialize)]
struct LoudnessSummary {
    lufs: Option<f64>,
    true_peak_db: Option<f64>,
    lra: f64,
}

impl From<&loudness::LoudnessReport> for LoudnessSummary {
    fn from(r: &loudness::LoudnessReport) -> Self {
        Self { lufs: finite(r.integrated), true_peak_db: finite(r.true_peak_db), lra: r.range }
    }
}

#[derive(Serialize)]
struct AutoMasterReport {
    master: MasterSettings,
    before: LoudnessSummary,
    after: LoudnessSummary,
    reached: bool,
    passes: usize,
}

/// Masteriza a una sonoridad objetivo con un limitador de pico real al final
/// del bus máster (se conservan los demás efectos del máster).
#[tauri::command]
async fn master_auto(app: AppHandle, target_lufs: f64, ceiling_db: f32) -> Result<AutoMasterReport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let engine = state.engine()?;
        let target = target_lufs.clamp(-30.0, -5.0);
        let ceiling = ceiling_db.clamp(-6.0, 0.0);
        let (premaster, project, dir, _) = render_premaster(&state)?;
        let r = auto_master(&premaster.data, premaster.sample_rate, &project.master.effects, target, ceiling, |_| {});
        if !r.before.integrated.is_finite() {
            return Err("la mezcla está en silencio".into());
        }
        let effects = with_auto_limiter(&project.master.effects, r.limiter_input_db, ceiling);
        let master = MasterSettings {
            gain_db: r.master_gain_db,
            automation: sanitize_lanes(project.master.automation.clone(), &effects),
            effects,
        };
        engine.set_effects(None, &master.effects).map_err(err)?;
        engine.set_master_gain_db(master.gain_db).map_err(err)?;
        engine
            .set_automation(None, ChannelAutomation::build(&master.automation, &master.effects, engine.sample_rate()))
            .map_err(err)?;
        let mut s = state.session.lock().unwrap();
        if let Some(p) = s.project.as_mut() {
            p.master = master.clone();
            p.save(&dir).map_err(err)?;
        }
        Ok(AutoMasterReport { master, before: (&r.before).into(), after: (&r.after).into(), reached: r.reached, passes: r.passes })
    })
    .await
    .map_err(err)?
}

/// Frecuencia a la que trabaja Matchering.
const MATCHERING_RATE: u32 = 44_100;

/// Lanza el master por referencia (Matchering) sobre la premezcla.
#[tauri::command]
async fn master_reference_start(app: AppHandle, reference: String) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let worker = state.worker.clone();
    let app2 = app.clone();
    let (target, output) = tauri::async_runtime::spawn_blocking(move || -> Result<(PathBuf, PathBuf), String> {
        let state = app2.state::<AppState>();
        let (premaster, _, dir, _) = render_premaster(&state)?;
        let buf = audio::resample(&premaster, MATCHERING_RATE).map_err(err)?;
        let target = dir.join("cache").join("master_target.wav");
        std::fs::create_dir_all(target.parent().unwrap()).map_err(err)?;
        audio::write_wav_f32(&target, &buf).map_err(err)?;
        Ok((target, dir.join(MASTER_DIR).join("master_ref.wav")))
    })
    .await
    .map_err(err)??;
    worker
        .call(
            "tasks.run",
            json!({ "task": "master_reference", "params": { "target": target, "reference": reference, "output": output } }),
        )
        .await
}

/// Añade el master por referencia como pista (en solo, para comparar A/B).
#[tauri::command]
async fn master_reference_apply(app: AppHandle, output: String, reference_name: String) -> Result<ProjectView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let engine = state.engine()?;
        let (dir, mut project) = session_snapshot(&state)?;
        let before = engine.state();
        engine.pause().map_err(err)?;
        let rel = Path::new(&output)
            .strip_prefix(&dir)
            .map_err(|_| format!("el master {output} está fuera del proyecto"))?
            .to_string_lossy()
            .replace('\\', "/");
        project.tracks.retain(|t| !t.file.starts_with(MASTER_DIR));
        let stem = Path::new(&reference_name).file_stem().and_then(|s| s.to_str()).unwrap_or("referencia");
        let mut track = TrackInfo::new(format!("Máster · {stem}"), TrackKind::Generated, rel);
        track.solo = true;
        project.tracks.push(track);
        project.save(&dir).map_err(err)?;
        let view = load_into_engine(&state, dir, project)?;
        engine.seek(before.position_frames).map_err(err)?;
        Ok(view)
    })
    .await
    .map_err(err)?
}

/// Cambia la cadena de efectos de una pista (o del máster con `track_id = null`).
/// Si solo cambian parámetros o el encendido, se aplican en caliente sin
/// reconstruir los efectos (no se cortan colas ni se oyen clics).
#[tauri::command]
fn effects_set(state: State<'_, AppState>, track_id: Option<Uuid>, effects: Vec<EffectSlot>) -> Result<Vec<EffectSlot>, String> {
    let engine = state.engine()?;
    let mut effects: Vec<EffectSlot> = effects
        .into_iter()
        .take(16)
        .map(|mut s| {
            s.params = s.params.sanitized();
            s
        })
        .collect();
    // Comando síncrono: corre en el hilo principal, así que consultar los plugins no espera a nadie.
    mn_plugin::sync_states(&mut effects);
    let mut guard = state.session.lock().unwrap();
    let s = &mut *guard;
    let engine_id = match track_id {
        Some(id) => Some(*s.engine_ids.get(&id).ok_or("pista desconocida")?),
        None => None,
    };
    let project = s.project.as_mut().ok_or("no hay ningún proyecto abierto")?;
    let (current, lanes) = channel_mut(project, track_id)?;
    let same_layout = current.len() == effects.len()
        && current.iter().zip(&effects).all(|(a, b)| a.id == b.id && a.params.kind() == b.params.kind() && a.sidechain == b.sidechain);
    if same_layout {
        for (i, (old, new)) in current.iter().zip(&effects).enumerate() {
            if old.params != new.params {
                engine.set_effect_params(engine_id, i, new.params).map_err(err)?;
            }
            if old.enabled != new.enabled {
                engine.set_effect_enabled(engine_id, i, new.enabled).map_err(err)?;
            }
        }
    } else {
        engine.set_effects(engine_id, &effects).map_err(err)?;
    }
    if let Some(id) = engine_id {
        let source = sidechain_source(&effects).and_then(|src| s.engine_ids.get(&src).copied());
        engine.set_sidechain(id, source).map_err(err)?;
    }
    *current = effects.clone();
    if !lanes.is_empty() {
        // Se quitan las curvas de efectos eliminados y se reconstruye con los parámetros nuevos.
        *lanes = sanitize_lanes(std::mem::take(lanes), &effects);
        engine.set_automation(engine_id, ChannelAutomation::build(lanes, &effects, engine.sample_rate())).map_err(err)?;
    }
    let slot_ids = plugin_slot_ids(project);
    mn_plugin::retain_instances(|id| slot_ids.iter().any(|s| s == id));
    if let Some(dir) = s.dir.clone() {
        project.save(&dir).map_err(err)?;
    }
    Ok(effects)
}

/// Sustituye los carriles de automatización de una pista (o del máster con
/// `track_id = null`). Se sanean contra su cadena de efectos.
#[tauri::command]
fn automation_set(state: State<'_, AppState>, track_id: Option<Uuid>, lanes: Vec<AutomationLane>) -> Result<Vec<AutomationLane>, String> {
    let engine = state.engine()?;
    let mut guard = state.session.lock().unwrap();
    let s = &mut *guard;
    let engine_id = match track_id {
        Some(id) => Some(*s.engine_ids.get(&id).ok_or("pista desconocida")?),
        None => None,
    };
    let project = s.project.as_mut().ok_or("no hay ningún proyecto abierto")?;
    let (effects, current) = channel_mut(project, track_id)?;
    let lanes = sanitize_lanes(lanes, effects);
    engine.set_automation(engine_id, ChannelAutomation::build(&lanes, effects, engine.sample_rate())).map_err(err)?;
    *current = lanes.clone();
    if let Some(dir) = s.dir.clone() {
        project.save(&dir).map_err(err)?;
    }
    Ok(lanes)
}

/// Ganancia del bus máster en dB.
#[tauri::command]
fn master_gain_set(state: State<'_, AppState>, gain_db: f32) -> Result<f32, String> {
    let engine = state.engine()?;
    let gain_db = gain_db.clamp(-24.0, 12.0);
    let mut guard = state.session.lock().unwrap();
    let s = &mut *guard;
    let project = s.project.as_mut().ok_or("no hay ningún proyecto abierto")?;
    engine.set_master_gain_db(gain_db).map_err(err)?;
    project.master.gain_db = gain_db;
    if let Some(dir) = s.dir.clone() {
        project.save(&dir).map_err(err)?;
    }
    Ok(gain_db)
}

/// Exporta la mezcla tal como suena (volumen, panorama, silencio y solo).
#[tauri::command]
async fn export_mix(app: AppHandle, path: String, format: ExportFormat) -> Result<MixExport, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let rate = state.engine()?.sample_rate();
        sync_plugin_states(&state)?;
        let (project, buffers, native_rate) = {
            let s = state.session.lock().unwrap();
            let p = s.project.clone().ok_or("no hay ningún proyecto abierto")?;
            (p, s.buffers.clone(), s.native_rate)
        };
        let clip_sets = clip_sets(&project, &buffers, rate);
        let autos = automations(&project, rate);
        let mut inputs: Vec<MixInput> = project
            .tracks
            .iter()
            .filter_map(|t| {
                buffers
                    .get(&t.id)
                    .map(|d| MixInput {
                        data: d,
                        clips: &clip_sets[&t.id],
                        gain_db: t.gain_db,
                        pan: t.pan,
                        mute: t.mute,
                        solo: t.solo,
                        effects: &t.effects,
                        automation: autos.get(&t.id),
                sidechain: None,
                    })
            })
            .collect();
        let ids: Vec<Uuid> = project.tracks.iter().filter(|t| buffers.contains_key(&t.id)).map(|t| t.id).collect();
        link_sidechains(&project, &ids, &mut inputs);
        let master_auto = ChannelAutomation::build(&project.master.automation, &project.master.effects, rate);
        let mixed = audio::resample(&mixdown(&inputs, &project.master, master_auto.as_ref(), rate), native_rate).map_err(err)?;
        let report = loudness::measure(&mixed.data, mixed.sample_rate);
        let path = Path::new(&path).with_extension(format.extension()).display().to_string();
        export_audio(Path::new(&path), &mixed, format).map_err(err)?;
        Ok(MixExport {
            path,
            peak_db: report.sample_peak_db.max(-120.0) as f32,
            sample_rate: mixed.sample_rate,
            lufs: finite(report.integrated),
            true_peak_db: finite(report.true_peak_db),
            lra: report.range,
        })
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
                    let clips = t.clips.clone();
                    s.spawn(move || {
                        let decoded = audio::decode_file(&src).map_err(err)?;
                        // La pista tal como queda editada (clips, fundidos, huecos), alineada desde 0.
                        let frames = clip_frames(&clips, decoded.sample_rate, decoded.frames() as u64);
                        let buf = AudioBuffer { sample_rate: decoded.sample_rate, data: render_track(&decoded.data, &frames) };
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

// ---- Grabación ---------------------------------------------------------------

/// Carpeta (relativa al proyecto) de las tomas grabadas.
const RECORDINGS_DIR: &str = "audio/recordings/";

#[tauri::command]
fn input_devices() -> Vec<InputDeviceInfo> {
    mn_engine::input_devices()
}

/// Abre una entrada de audio (`device = null`: la predeterminada del sistema).
#[tauri::command]
async fn input_open(app: AppHandle, device: Option<String>, channels: InputChannels) -> Result<InputInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        state.engine()?.open_input(device.as_deref(), channels).map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
fn input_close(state: State<'_, AppState>) -> Result<(), String> {
    let engine = state.engine()?;
    if engine.is_recording() {
        return Err("detén la grabación antes de cerrar la entrada".into());
    }
    engine.close_input();
    Ok(())
}

#[tauri::command]
fn input_monitor(state: State<'_, AppState>, enabled: bool, gain_db: f32) -> Result<(), String> {
    state.engine()?.set_monitor(enabled, gain_db.clamp(-60.0, 12.0)).map_err(err)
}

/// Empieza a grabar en la posición actual (con la cuenta previa de práctica).
#[tauri::command]
fn record_start(state: State<'_, AppState>, latency_ms: f64) -> Result<(), String> {
    let engine = state.engine()?;
    let (dir, project) = session_snapshot(&state)?;
    let rec_dir = dir.join(RECORDINGS_DIR);
    std::fs::create_dir_all(&rec_dir).map_err(err)?;
    let path = (1..)
        .map(|n| rec_dir.join(format!("toma_{n:03}.wav")))
        .find(|p| !p.exists())
        .ok_or("no se pudo elegir un nombre para la toma")?;
    let p = &project.practice;
    let count_in = (p.count_in_bars > 0).then(|| (p.count_in_bars * p.beats_per_bar, p.bpm));
    engine.start_recording(path, latency_ms.clamp(-500.0, 1000.0), count_in).map_err(err)
}

#[derive(Clone, Serialize)]
struct RecordFinished {
    track: Option<TrackView>,
    message: Option<String>,
}

/// Detiene la grabación y añade la toma como pista nueva. El resultado se
/// publica en el evento `record-finished` (se puede parar desde varios sitios).
#[tauri::command]
async fn record_stop(app: AppHandle) -> Result<(), String> {
    finish_recording(app).await
}

async fn finish_recording(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let take = state.engine()?.stop_recording().map_err(err)?;
        let result = match take {
            None => RecordFinished { track: None, message: Some("La grabación no llegó a empezar.".into()) },
            Some(take) => add_take(&state, take)?,
        };
        let _ = app.emit("record-finished", result);
        Ok(())
    })
    .await
    .map_err(err)?
}

fn add_take(state: &AppState, take: mn_engine::Take) -> Result<RecordFinished, String> {
    if take.frames < take.sample_rate as u64 / 4 {
        let _ = std::fs::remove_file(&take.path);
        return Ok(RecordFinished { track: None, message: Some("La toma duraba menos de 0,25 s y se descartó.".into()) });
    }
    let engine = state.engine()?;
    let rate = engine.sample_rate();
    let decoded = audio::decode_file(&take.path).map_err(err)?;
    let buf = audio::resample(&decoded, rate).map_err(err)?;
    let peaks = compute_peaks(&buf, (rate / 100) as usize);
    let duration = buf.duration_secs();

    let mut guard = state.session.lock().unwrap();
    let s = &mut *guard;
    let dir = s.dir.clone().ok_or("no hay ningún proyecto abierto")?;
    let rel = take
        .path
        .strip_prefix(&dir)
        .map_err(|_| "la toma está fuera del proyecto".to_string())?
        .to_string_lossy()
        .replace('\\', "/");
    let project = s.project.as_mut().ok_or("no hay ningún proyecto abierto")?;
    let n = project.tracks.iter().filter(|t| t.kind == TrackKind::Recording).count() + 1;
    let mut info = TrackInfo::new(format!("Toma {n}"), TrackKind::Recording, rel);
    info.clips = vec![Clip::whole(take.offset_secs, duration)];
    let data = Arc::new(buf.data);
    let id = add_to_engine(engine, &info, data.clone(), rate)?;
    project.tracks.push(info.clone());
    project.save(&dir).map_err(err)?;
    s.engine_ids.insert(info.id, id);
    s.buffers.insert(info.id, data);
    let message = (take.dropped_frames > 0).then(|| {
        format!(
            "Se perdieron {:.0} ms de audio porque el disco no siguió el ritmo de la grabación.",
            take.dropped_frames as f64 / take.sample_rate as f64 * 1000.0
        )
    });
    Ok(RecordFinished { track: Some(TrackView { info, duration, peaks }), message })
}

/// Sustituye los clips de una pista (mover, recortar, dividir, fundidos...).
/// Se sanean contra la duración del audio y se aplican sin cortar el sonido.
#[tauri::command]
fn clips_set(state: State<'_, AppState>, track_id: Uuid, clips: Vec<Clip>) -> Result<Vec<Clip>, String> {
    let engine = state.engine()?;
    let rate = engine.sample_rate();
    let mut guard = state.session.lock().unwrap();
    let s = &mut *guard;
    let id = *s.engine_ids.get(&track_id).ok_or("pista desconocida")?;
    let frames = (s.buffers.get(&track_id).ok_or("pista desconocida")?.len() / 2) as u64;
    let project = s.project.as_mut().ok_or("no hay ningún proyecto abierto")?;
    let t = project.tracks.iter_mut().find(|t| t.id == track_id).ok_or("pista desconocida")?;
    let clips = sanitize_clips(clips, frames as f64 / rate as f64);
    engine.set_clips(id, clip_frames(&clips, rate, frames)).map_err(err)?;
    t.clips = clips.clone();
    if let Some(dir) = s.dir.clone() {
        project.save(&dir).map_err(err)?;
    }
    Ok(clips)
}

/// Mide la latencia de ida y vuelta de la entrada y la salida actuales.
#[tauri::command]
async fn record_calibrate(app: AppHandle) -> Result<LatencyReport, String> {
    tauri::async_runtime::spawn_blocking(move || app.state::<AppState>().engine()?.calibrate_latency().map_err(err))
        .await
        .map_err(err)?
}

/// Quita una pista del proyecto. El audio de las tomas grabadas se borra; el
/// de las demás pistas se conserva en la carpeta del proyecto.
#[tauri::command]
fn track_remove(state: State<'_, AppState>, track_id: Uuid) -> Result<(), String> {
    let engine = state.engine()?;
    let mut guard = state.session.lock().unwrap();
    let s = &mut *guard;
    let dir = s.dir.clone().ok_or("no hay ningún proyecto abierto")?;
    let project = s.project.as_mut().ok_or("no hay ningún proyecto abierto")?;
    let index = project.tracks.iter().position(|t| t.id == track_id).ok_or("pista desconocida")?;
    if let Some(id) = s.engine_ids.remove(&track_id) {
        engine.remove_track(id).map_err(err)?;
    }
    s.buffers.remove(&track_id);
    let track = project.tracks.remove(index);
    project.save(&dir).map_err(err)?;
    let slot_ids = plugin_slot_ids(project);
    mn_plugin::retain_instances(|id| slot_ids.iter().any(|s| s == id));
    if [RECORDINGS_DIR, transcribe::MIDI_DIR, transcribe::TUNED_DIR, generate::DENOISED_DIR, generate::GENERATED_DIR, instruments::INSTRUMENTS_DIR]
        .iter()
        .any(|d| track.file.starts_with(d))
    {
        let _ = std::fs::remove_file(project.track_path(&dir, &track));
    }
    Ok(())
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
                let input = engine.input_levels();
                idle_ticks += 1;
                if !s.playing && !s.counting_in && input.is_none() && idle_ticks < 15 {
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
                        lufs_m: finite(s.lufs_momentary),
                        lufs_s: finite(s.lufs_short_term),
                        recording: s.recording,
                        input_peak: input.map(|i| [i.peak_l, i.peak_r]),
                        recorded: input.map_or(0.0, |i| i.recorded_secs),
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
            // Plugins: las llamadas de «hilo principal» van al bucle de la ventana.
            let handle = app.handle().clone();
            mn_plugin::set_main_thread_executor(move |f| {
                let _ = handle.run_on_main_thread(f);
            });
            mn_plugin::install();
            plugins::on_editor_closed(app.handle().clone());
            #[cfg(windows)]
            if let Some(hwnd) = app.get_webview_window("main").and_then(|w| w.hwnd().ok()) {
                mn_plugin::set_owner_window(hwnd.0 as isize);
            }
            spawn_transport_events(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            import_audio,
            create_project,
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
            lyrics_start,
            lyrics_apply,
            lyrics_update,
            lyrics_export,
            effects_set,
            master_gain_set,
            master_auto,
            master_reference_start,
            master_reference_apply,
            input_devices,
            input_open,
            input_close,
            input_monitor,
            record_start,
            record_stop,
            record_calibrate,
            track_remove,
            clips_set,
            automation_set,
            plugins::plugins_list,
            plugins::plugins_scan,
            plugins::plugin_open_editor,
            plugins::plugin_params,
            plugins::plugin_param_set,
            plugins::plugins_sync,
            transcribe::notes_start,
            transcribe::notes_apply,
            transcribe::notes_update,
            transcribe::notes_remove,
            transcribe::notes_export_midi,
            transcribe::notes_render,
            transcribe::pitch_start,
            transcribe::pitch_apply,
            transcribe::tune_render,
            transcribe::instruments_start,
            transcribe::instruments_apply,
            generate::denoise_steady,
            generate::denoise_voice_start,
            generate::denoise_voice_apply,
            generate::generate_start,
            generate::generate_apply,
            instruments::instrument_track_add,
            instruments::instrument_set,
            instruments::instrument_render,
            instruments::midi_import,
        ])
        .build(tauri::generate_context!())
        .expect("error al iniciar MusicNova");

    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Err(e) = sync_plugin_states(&handle.state::<AppState>()) {
                eprintln!("[musicnova] estado de plugins: {e}");
            }
            if let Some(e) = handle.state::<AppState>().engine.as_ref() {
                e.close_input();
            }
            handle.state::<AppState>().worker.stop();
        }
    });
}
