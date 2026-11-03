//! Pistas de instrumento: notas MIDI que suenan con el sintetizador integrado o
//! con un plugin instrumento (CLAP o VST3).
//!
//! El audio de la pista se genera al cambiar sus notas o su instrumento y se
//! guarda en `audio/instruments/<pista>.flac`; el motor lo reproduce como
//! cualquier otra pista, así que mezcla, efectos, latencia y exportación
//! funcionan igual. Importar un archivo MIDI crea una pista por parte.

use std::path::Path;
use std::sync::Arc;

use mn_core::audio::AudioBuffer;
use mn_core::export::{export_audio, ExportFormat};
use mn_core::peaks::compute_peaks;
use mn_core::project::{clip_frames, Clip, InstrumentSource, TrackInfo, TrackKind};
use mn_core::transcription::{read_midi, synthesize, Note, Timbre, TrackNotes, Transcription, DRUMS_MODE};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::{err, load_into_engine, session_snapshot, AppState, ProjectView, TrackView};

pub const INSTRUMENTS_DIR: &str = "audio/instruments/";
/// Cola después de la última nota o del final de la canción (liberaciones, reverb del plugin).
const TAIL: f64 = 2.0;
/// Duración mínima de una pista de instrumento vacía (s).
const MIN_LENGTH: f64 = 8.0;

/// Timbre del sintetizador integrado para un programa General MIDI.
pub fn timbre_for_program(program: u8, drums: bool) -> Timbre {
    if drums {
        return Timbre::Drums;
    }
    match program {
        0..=7 => Timbre::Keys,
        24..=31 | 104..=111 => Timbre::Pluck,
        32..=39 => Timbre::Bass,
        40..=55 | 88..=95 => Timbre::Pad,
        _ => Timbre::Lead,
    }
}

/// Programa General MIDI que representa a un timbre integrado (al exportar MIDI).
pub fn program_for_timbre(t: Timbre) -> u8 {
    match t {
        Timbre::Keys => 4,
        Timbre::Bass => 33,
        Timbre::Lead => 80,
        Timbre::Pad => 89,
        Timbre::Pluck => 25,
        Timbre::Drums => 0,
    }
}

/// Genera el audio de las notas con el instrumento (a la frecuencia del motor).
fn render(source: &InstrumentSource, notes: &[Note], rate: u32, length: f64) -> Result<AudioBuffer, String> {
    match source {
        InstrumentSource::Synth { timbre } => Ok(synthesize(notes, rate, Some(length + TAIL), *timbre)),
        InstrumentSource::Plugin { plugin } => {
            let list: Vec<mn_plugin::RenderNote> = notes.iter().map(|n| (n.start, n.end, n.pitch, n.velocity)).collect();
            let data = mn_plugin::render_instrument(plugin, &list, rate, length, TAIL)?;
            Ok(AudioBuffer { sample_rate: rate, data })
        }
    }
}

/// Vuelve a generar el audio de una pista de instrumento y lo cambia en el
/// motor sin recargar el proyecto. Devuelve la vista nueva de la pista.
pub(crate) fn rerender(state: &AppState, track_id: Uuid) -> Result<TrackView, String> {
    let engine = state.engine()?;
    let rate = engine.sample_rate();
    let (dir, project) = session_snapshot(state)?;
    let info = project.tracks.iter().find(|t| t.id == track_id).ok_or("pista desconocida")?.clone();
    let source = info.instrument.clone().ok_or("la pista no es de instrumento")?;
    let notes = Transcription::load(&dir).map_err(err)?.and_then(|t| t.track(track_id).map(|tn| tn.notes.clone())).unwrap_or_default();
    // Hasta el final de la canción (o de la última nota) más la cola.
    let song_end = {
        let s = state.session.lock().unwrap();
        crate::timeline_length(&project, &s.buffers, rate) as f64 / rate as f64
    };
    let last = notes.iter().map(|n| n.end).fold(0.0, f64::max);
    let length = song_end.max(last).max(MIN_LENGTH);
    let buf = render(&source, &notes, rate, length)?;
    let rel = format!("{INSTRUMENTS_DIR}{track_id}.flac");
    let path = dir.join(&rel);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
    export_audio(&path, &buf, ExportFormat::Flac24).map_err(err)?;

    let duration = buf.duration_secs();
    let peaks = compute_peaks(&buf, (rate / 100) as usize);
    let data = Arc::new(buf.data);
    let mut guard = state.session.lock().unwrap();
    let s = &mut *guard;
    let p = s.project.as_mut().ok_or("no hay ningún proyecto abierto")?;
    let t = p.tracks.iter_mut().find(|t| t.id == track_id).ok_or("pista desconocida")?;
    t.file = rel;
    t.clips = vec![Clip::whole(0.0, duration)];
    let frames = clip_frames(&t.clips, rate, (data.len() / 2) as u64);
    let view = TrackView { info: t.clone(), duration, peaks };
    p.save(&dir).map_err(err)?;
    if let Some(&id) = s.engine_ids.get(&track_id) {
        engine.set_track_data(id, data.clone(), rate, frames).map_err(err)?;
    }
    s.buffers.insert(track_id, data);
    Ok(view)
}

/// Añade pistas de instrumento (con sus notas) y recarga el proyecto.
fn add_tracks(state: &AppState, parts: Vec<(String, InstrumentSource, String, Vec<Note>)>) -> Result<ProjectView, String> {
    let engine = state.engine()?;
    let rate = engine.sample_rate();
    let (dir, mut project) = session_snapshot(state)?;
    let mut notes_file = Transcription::load(&dir).map_err(err)?.unwrap_or_default();
    let song_end = {
        let s = state.session.lock().unwrap();
        crate::timeline_length(&project, &s.buffers, rate) as f64 / rate as f64
    };
    for (name, source, mode, notes) in parts {
        let mut info = TrackInfo::new(name, TrackKind::Instrument, String::new());
        let last = notes.iter().map(|n| n.end).fold(0.0, f64::max);
        let buf = render(&source, &notes, rate, song_end.max(last).max(MIN_LENGTH))?;
        let rel = format!("{INSTRUMENTS_DIR}{}.flac", info.id);
        let path = dir.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
        export_audio(&path, &buf, ExportFormat::Flac24).map_err(err)?;
        info.file = rel;
        info.clips = vec![Clip::whole(0.0, buf.duration_secs())];
        info.instrument = Some(source);
        notes_file.set_track(TrackNotes { track_id: info.id, mode, notes });
        project.tracks.push(info);
    }
    notes_file.save(&dir).map_err(err)?;
    project.save(&dir).map_err(err)?;
    let before = engine.state();
    engine.pause().map_err(err)?;
    let view = load_into_engine(state, dir, project)?;
    engine.seek(before.position_frames).map_err(err)?;
    Ok(view)
}

/// Crea una pista de instrumento vacía para escribir notas en el piano roll.
#[tauri::command]
pub async fn instrument_track_add(app: AppHandle, name: String, source: InstrumentSource) -> Result<ProjectView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let drums = matches!(source, InstrumentSource::Synth { timbre: Timbre::Drums });
        let mode = if drums { DRUMS_MODE } else { "poly" }.to_string();
        let name = if name.trim().is_empty() { "Instrumento".to_string() } else { name.trim().to_string() };
        add_tracks(&state, vec![(name, source, mode, Vec::new())])
    })
    .await
    .map_err(err)?
}

/// Cambia el instrumento de una pista y vuelve a generar su audio.
#[tauri::command]
pub async fn instrument_set(app: AppHandle, track_id: Uuid, source: InstrumentSource) -> Result<TrackView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        {
            let mut s = state.session.lock().unwrap();
            let dir = s.dir.clone().ok_or("no hay ningún proyecto abierto")?;
            let p = s.project.as_mut().ok_or("no hay ningún proyecto abierto")?;
            let t = p.tracks.iter_mut().find(|t| t.id == track_id).ok_or("pista desconocida")?;
            if t.kind != TrackKind::Instrument {
                return Err("la pista no es de instrumento".into());
            }
            t.instrument = Some(source);
            p.save(&dir).map_err(err)?;
        }
        let view = rerender(&state, track_id)?;
        // Armada: lo que se toque suena ya con el instrumento nuevo.
        if crate::live::armed() == Some(track_id) {
            crate::live::arm(&state, Some(track_id))?;
        }
        Ok(view)
    })
    .await
    .map_err(err)?
}

/// Vuelve a generar el audio de una pista de instrumento (tras editar sus notas).
#[tauri::command]
pub async fn instrument_render(app: AppHandle, track_id: Uuid) -> Result<TrackView, String> {
    tauri::async_runtime::spawn_blocking(move || rerender(&app.state::<AppState>(), track_id)).await.map_err(err)?
}

/// Importa un archivo MIDI: una pista de instrumento por parte, con el timbre
/// integrado que corresponde a su programa General MIDI.
#[tauri::command]
pub async fn midi_import(app: AppHandle, path: String) -> Result<ProjectView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let bytes = std::fs::read(Path::new(&path)).map_err(err)?;
        let parts = read_midi(&bytes).map_err(err)?;
        if parts.is_empty() {
            return Err("el archivo MIDI no tiene notas".into());
        }
        let list = parts
            .into_iter()
            .take(16)
            .map(|p| {
                let timbre = timbre_for_program(p.program, p.drums);
                let mode = if p.drums { DRUMS_MODE } else { "poly" }.to_string();
                (p.name, InstrumentSource::Synth { timbre }, mode, p.notes)
            })
            .collect();
        add_tracks(&state, list)
    })
    .await
    .map_err(err)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn general_midi_programs_pick_a_timbre() {
        assert_eq!(timbre_for_program(0, false), Timbre::Keys);
        assert_eq!(timbre_for_program(33, false), Timbre::Bass);
        assert_eq!(timbre_for_program(25, false), Timbre::Pluck);
        assert_eq!(timbre_for_program(48, false), Timbre::Pad);
        assert_eq!(timbre_for_program(56, false), Timbre::Lead);
        assert_eq!(timbre_for_program(0, true), Timbre::Drums);
        // Ida y vuelta: exportar e importar conserva el timbre.
        for t in [Timbre::Keys, Timbre::Bass, Timbre::Lead, Timbre::Pad, Timbre::Pluck] {
            assert_eq!(timbre_for_program(program_for_timbre(t), false), t);
        }
    }
}
