//! Formato de proyecto abierto: una carpeta con `project.json` y el audio.
//!
//! ```text
//! MiCancion/
//!   project.json
//!   audio/      original y stems
//!   analysis/   resultados de análisis (JSON)
//!   cache/      datos regenerables
//! ```

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{CoreError, Result};

pub const PROJECT_FILE: &str = "project.json";
pub const FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Mix,
    Vocals,
    BackingVocals,
    Drums,
    Bass,
    Guitar,
    Piano,
    Other,
    Generated,
    Recording,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrackInfo {
    pub id: Uuid,
    pub name: String,
    pub kind: TrackKind,
    /// Ruta relativa a la carpeta del proyecto.
    pub file: String,
    pub gain_db: f32,
    /// -1 (izquierda) .. 1 (derecha)
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}

impl TrackInfo {
    pub fn new(name: impl Into<String>, kind: TrackKind, file: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            kind,
            file: file.into(),
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Marker {
    pub id: Uuid,
    pub name: String,
    /// Segundos desde el inicio.
    pub time: f64,
}

/// Origen de la rejilla del metrónomo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeatSource {
    /// Tempo fijo (`bpm` + `beat_offset`).
    #[default]
    Manual,
    /// Pulsos y tiempos fuertes del análisis.
    Detected,
}

/// Ajustes de práctica guardados con el proyecto.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PracticeSettings {
    /// Velocidad (1.0 = original).
    pub tempo: f64,
    /// Transposición en semitonos.
    pub pitch: f32,
    pub loop_enabled: bool,
    pub loop_start: f64,
    pub loop_end: f64,
    pub metronome: bool,
    pub metronome_gain_db: f32,
    /// Tempo de la canción para el metrónomo manual.
    pub bpm: f64,
    /// Segundo en el que cae el primer pulso.
    pub beat_offset: f64,
    pub beats_per_bar: u32,
    /// Compases de cuenta previa (0 = sin cuenta).
    pub count_in_bars: u32,
    pub markers: Vec<Marker>,
    pub trainer_enabled: bool,
    pub trainer_start: f64,
    pub trainer_step: f64,
    pub trainer_target: f64,
    /// Repeticiones del bucle antes de subir el tempo.
    pub trainer_every: u32,
    pub beat_source: BeatSource,
    /// Traste de la cejilla para mostrar los acordes (0 = sin cejilla).
    pub capo: u32,
}

impl Default for PracticeSettings {
    fn default() -> Self {
        Self {
            tempo: 1.0,
            pitch: 0.0,
            loop_enabled: false,
            loop_start: 0.0,
            loop_end: 0.0,
            metronome: false,
            metronome_gain_db: -6.0,
            bpm: 120.0,
            beat_offset: 0.0,
            beats_per_bar: 4,
            count_in_bars: 0,
            markers: Vec::new(),
            trainer_enabled: false,
            trainer_start: 0.7,
            trainer_step: 0.05,
            trainer_target: 1.0,
            trainer_every: 2,
            beat_source: BeatSource::Manual,
            capo: 0,
        }
    }
}

impl PracticeSettings {
    /// Pulsos en frames para un tempo fijo (`bpm`, `beat_offset`) hasta `length` frames.
    pub fn manual_beats(&self, sample_rate: u32, length: u64) -> Vec<u64> {
        let bpm = self.bpm.clamp(20.0, 400.0);
        let step = 60.0 / bpm * sample_rate as f64;
        // Empieza en el primer pulso >= 0 de la rejilla que pasa por `beat_offset`.
        let offset = self.beat_offset * sample_rate as f64;
        let mut t = offset - (offset / step).floor() * step;
        let mut out = Vec::with_capacity((length as f64 / step) as usize + 1);
        while t < length as f64 {
            out.push(t.round() as u64);
            t += step;
        }
        out
    }

    /// Índice del primer tiempo fuerte en la rejilla manual (el pulso en `beat_offset`).
    pub fn manual_downbeat_offset(&self) -> u32 {
        let bpm = self.bpm.clamp(20.0, 400.0);
        let beats_before = (self.beat_offset / (60.0 / bpm)).floor().max(0.0) as u32;
        beats_before % self.beats_per_bar.max(1)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub format_version: u32,
    pub id: Uuid,
    pub name: String,
    pub created: DateTime<Utc>,
    pub modified: DateTime<Utc>,
    /// Ruta del archivo original importado (informativa).
    pub source_path: Option<String>,
    pub tracks: Vec<TrackInfo>,
    #[serde(default)]
    pub practice: PracticeSettings,
}

impl Project {
    pub fn new(name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            format_version: FORMAT_VERSION,
            id: Uuid::new_v4(),
            name: name.into(),
            created: now,
            modified: now,
            source_path: None,
            tracks: Vec::new(),
            practice: PracticeSettings::default(),
        }
    }

    /// Crea la estructura de carpetas de un proyecto nuevo.
    pub fn create_dirs(dir: &Path) -> Result<()> {
        for sub in ["audio", "analysis", "cache"] {
            std::fs::create_dir_all(dir.join(sub))?;
        }
        Ok(())
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(dir.join(PROJECT_FILE))?;
        let p: Project = serde_json::from_str(&text)?;
        if p.format_version > FORMAT_VERSION {
            return Err(CoreError::Format(format!(
                "el proyecto usa la versión de formato {} y esta aplicación solo admite hasta la {}",
                p.format_version, FORMAT_VERSION
            )));
        }
        Ok(p)
    }

    /// Guarda de forma atómica (archivo temporal + renombrado).
    pub fn save(&mut self, dir: &Path) -> Result<()> {
        self.modified = Utc::now();
        let tmp = dir.join(format!("{PROJECT_FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, dir.join(PROJECT_FILE))?;
        Ok(())
    }

    pub fn track_path(&self, dir: &Path, track: &TrackInfo) -> PathBuf {
        dir.join(&track.file)
    }
}

/// Nombre de carpeta seguro para Windows a partir de un título libre.
pub fn sanitize_dir_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches('.').trim();
    if trimmed.is_empty() {
        "Proyecto".into()
    } else {
        trimmed.chars().take(80).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("mn_proj_{}", Uuid::new_v4()));
        Project::create_dirs(&dir).unwrap();
        let mut p = Project::new("Prueba");
        p.tracks.push(TrackInfo::new("Mezcla", TrackKind::Mix, "audio/original.wav"));
        p.save(&dir).unwrap();
        let back = Project::load(&dir).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(back.id, p.id);
        assert_eq!(back.tracks[0].kind, TrackKind::Mix);
    }

    #[test]
    fn manual_beats_grid() {
        let p = PracticeSettings { bpm: 120.0, beat_offset: 1.25, beats_per_bar: 4, ..Default::default() };
        let beats = p.manual_beats(1000, 3000);
        // Un pulso cada 500 frames alineado con 1250: 250, 750, 1250, ...
        assert_eq!(beats, vec![250, 750, 1250, 1750, 2250, 2750]);
        assert_eq!(p.manual_downbeat_offset(), 2);
    }

    #[test]
    fn old_projects_get_default_practice() {
        let json = r#"{"format_version":1,"id":"8f1c7a8e-0000-4000-8000-000000000000","name":"x",
            "created":"2026-10-31T00:00:00Z","modified":"2026-10-31T00:00:00Z","source_path":null,"tracks":[]}"#;
        let p: Project = serde_json::from_str(json).unwrap();
        assert_eq!(p.practice, PracticeSettings::default());
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_dir_name("AC/DC: Back?"), "AC_DC_ Back_");
        assert_eq!(sanitize_dir_name("  ..  "), "Proyecto");
    }
}
