//! Tocar en vivo: teclado MIDI (o el del ordenador) sobre una pista de
//! instrumento armada, con grabación de lo tocado como notas de la pista.
//!
//! La pista armada suena con su instrumento en tiempo real (el sintetizador
//! integrado o su plugin, en su propio proceso) a través de sus efectos y su
//! fader. Lo grabado vuelve a la interfaz como notas, que entran en deshacer y
//! regeneran el audio de la pista como cualquier edición del piano roll.

use std::sync::Mutex;

use mn_core::livesynth::LiveSynth;
use mn_core::project::{InstrumentSource, TrackKind};
use mn_core::transcription::Note;
use mn_engine::{now_ns, pair_notes, MidiEvent, MidiMessage};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::{err, AppState};

struct LiveState {
    #[cfg(windows)]
    midi: Option<mn_engine::MidiInput>,
    armed: Option<Uuid>,
}

static LIVE: Mutex<LiveState> = Mutex::new(LiveState {
    #[cfg(windows)]
    midi: None,
    armed: None,
});

/// Id con el que se registra la instancia en vivo del plugin de una pista.
pub fn instrument_key(track_id: Uuid) -> String {
    format!("{}{track_id}", mn_plugin::INSTRUMENT_PREFIX)
}

#[derive(Serialize)]
pub struct LiveStatus {
    /// Teclados MIDI conectados.
    devices: Vec<String>,
    /// El que está abierto.
    open: Option<String>,
    /// Pista armada (la que suena al tocar).
    armed: Option<Uuid>,
}

fn status() -> LiveStatus {
    let live = LIVE.lock().unwrap();
    #[cfg(windows)]
    let (devices, open) = (mn_engine::midi_inputs(), live.midi.as_ref().map(|m| m.name.clone()));
    #[cfg(not(windows))]
    let (devices, open) = (Vec::new(), None);
    LiveStatus { devices, open, armed: live.armed }
}

#[tauri::command]
pub fn live_status() -> LiveStatus {
    status()
}

/// Abre el teclado MIDI `index` (o ninguno con `None`).
#[tauri::command]
pub fn midi_open(state: State<'_, AppState>, index: Option<usize>) -> Result<LiveStatus, String> {
    let engine = state.engine()?;
    {
        let mut live = LIVE.lock().unwrap();
        #[cfg(windows)]
        {
            live.midi = None; // cierra el anterior antes de abrir otro
            if let Some(i) = index {
                live.midi = Some(mn_engine::MidiInput::open(i, engine.midi_sink())?);
            }
        }
        #[cfg(not(windows))]
        if index.is_some() {
            return Err("la entrada MIDI solo está disponible en Windows por ahora".into());
        }
        let _ = (&mut live, &engine);
    }
    Ok(status())
}

/// Arma la pista `track_id` (o desarma con `None`): lo que se toque sonará con su instrumento.
pub(crate) fn arm(state: &AppState, track_id: Option<Uuid>) -> Result<(), String> {
    let engine = state.engine()?;
    let rate = engine.sample_rate();
    let previous = LIVE.lock().unwrap().armed;
    let Some(track_id) = track_id else {
        engine.set_live_instrument(None).map_err(err)?;
        if let Some(p) = previous {
            mn_plugin::release_instrument(&instrument_key(p));
        }
        LIVE.lock().unwrap().armed = None;
        return Ok(());
    };
    let (source, engine_id) = {
        let s = state.session.lock().unwrap();
        let p = s.project.as_ref().ok_or("no hay ningún proyecto abierto")?;
        let t = p.tracks.iter().find(|t| t.id == track_id).ok_or("pista desconocida")?;
        if t.kind != TrackKind::Instrument {
            return Err("solo se pueden tocar en vivo las pistas de instrumento".into());
        }
        let source = t.instrument.clone().ok_or("la pista no tiene instrumento")?;
        (source, *s.engine_ids.get(&track_id).ok_or("pista desconocida")?)
    };
    let key = instrument_key(track_id);
    let instrument: Box<dyn mn_fx::Instrument> = match &source {
        InstrumentSource::Synth { timbre } => {
            mn_plugin::release_instrument(&key);
            Box::new(LiveSynth::new(*timbre, rate))
        }
        InstrumentSource::Plugin { plugin } => mn_plugin::live_instrument(plugin, rate, &key)?,
    };
    engine.set_live_instrument(Some((engine_id, instrument))).map_err(err)?;
    if let Some(p) = previous.filter(|p| *p != track_id) {
        mn_plugin::release_instrument(&instrument_key(p));
    }
    LIVE.lock().unwrap().armed = Some(track_id);
    Ok(())
}

/// Pista armada, si la hay.
pub(crate) fn armed() -> Option<Uuid> {
    LIVE.lock().unwrap().armed
}

/// El proyecto se recarga en el motor (los ids de pista cambian): se desarma.
pub(crate) fn forget(state: &AppState) {
    if armed().is_some() {
        if let Err(e) = arm(state, None) {
            eprintln!("[musicnova] desarmar: {e}");
        }
    }
}

#[tauri::command]
pub async fn live_arm(app: AppHandle, track_id: Option<Uuid>) -> Result<LiveStatus, String> {
    tauri::async_runtime::spawn_blocking(move || arm(&app.state::<AppState>(), track_id)).await.map_err(err)??;
    Ok(status())
}

/// Nota desde la interfaz (teclado del ordenador o teclado en pantalla).
#[tauri::command]
pub fn live_note(state: State<'_, AppState>, key: u8, velocity: u8, on: bool) -> Result<(), String> {
    let engine = state.engine()?;
    let msg = if on { MidiMessage::NoteOn { key: key.min(127), velocity: velocity.clamp(1, 127) } } else { MidiMessage::NoteOff { key: key.min(127) } };
    engine.midi_sink().send(MidiEvent { msg, time_ns: now_ns() });
    Ok(())
}

/// Suelta todas las notas (por si se queda alguna colgada).
#[tauri::command]
pub fn live_panic(state: State<'_, AppState>) -> Result<(), String> {
    state.engine()?.midi_sink().send(MidiEvent { msg: MidiMessage::AllNotesOff, time_ns: 0 });
    Ok(())
}

/// Empieza o termina de grabar lo que se toca (solo cuenta con el transporte en marcha).
#[tauri::command]
pub fn live_record(state: State<'_, AppState>, on: bool) -> Result<(), String> {
    let engine = state.engine()?;
    if on {
        engine.take_midi_recording(); // descarta lo que quedara de antes
    }
    engine.set_midi_record(on).map_err(err)
}

/// Notas grabadas desde la última lectura (las que siguen pulsadas terminan en el cabezal).
#[tauri::command]
pub fn live_take(state: State<'_, AppState>) -> Result<Vec<Note>, String> {
    let engine = state.engine()?;
    let events = engine.take_midi_recording();
    let end = engine.state().position_frames;
    Ok(pair_notes(&events, engine.sample_rate(), end).into_iter().map(|(s, e, k, v)| Note::new(s, e, k, v)).collect())
}
