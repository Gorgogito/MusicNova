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
use mn_fx::EffectSlot;
use uuid::Uuid;

use crate::automation::AutomationLane;
use crate::mix::ClipFrames;
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
    /// Cadena de efectos (antes del volumen y el panorama).
    #[serde(default)]
    pub effects: Vec<EffectSlot>,
    /// Formato antiguo (fase 5c): segundo en el que empezaba la pista. Al
    /// cargar se convierte en un clip y vuelve a 0.
    #[serde(default)]
    pub offset: f64,
    /// Tramos del audio colocados en la línea de tiempo (edición no destructiva).
    #[serde(default)]
    pub clips: Vec<Clip>,
    /// Curvas de volumen, panorama y parámetros de efectos.
    #[serde(default)]
    pub automation: Vec<AutomationLane>,
}

/// Duración mínima de un clip (s).
pub const MIN_CLIP_SECONDS: f64 = 0.01;
/// Clips por pista como máximo.
pub const MAX_CLIPS: usize = 2000;

/// Tramo del audio de una pista colocado en la línea de tiempo. Editar un clip
/// nunca modifica el archivo de audio.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Clip {
    pub id: Uuid,
    /// Segundo de la línea de tiempo en el que empieza.
    pub start: f64,
    /// Segundo del archivo de la pista en el que empieza.
    pub source_start: f64,
    /// Duración en segundos.
    pub length: f64,
    #[serde(default)]
    pub fade_in: f64,
    #[serde(default)]
    pub fade_out: f64,
    #[serde(default)]
    pub gain_db: f32,
}

impl Clip {
    /// Todo el archivo (de `source_secs` segundos) a partir de `start`.
    pub fn whole(start: f64, source_secs: f64) -> Self {
        Self { id: Uuid::new_v4(), start, source_start: 0.0, length: source_secs, fade_in: 0.0, fade_out: 0.0, gain_db: 0.0 }
    }

    pub fn end(&self) -> f64 {
        self.start + self.length
    }

    /// En frames a `rate` Hz, limitado a los `source_frames` del audio.
    pub fn to_frames(&self, rate: u32, source_frames: u64) -> ClipFrames {
        let f = |s: f64| (s.max(0.0) * rate as f64).round() as u64;
        let src_start = f(self.source_start).min(source_frames);
        let len = f(self.length).min(source_frames - src_start);
        let fade_in = f(self.fade_in).min(len);
        ClipFrames {
            start: f(self.start),
            src_start,
            len,
            fade_in,
            fade_out: f(self.fade_out).min(len - fade_in),
            gain: crate::mix::db_to_gain(self.gain_db),
        }
    }
}

/// Clips en frames para el motor o la mezcla.
pub fn clip_frames(clips: &[Clip], rate: u32, source_frames: u64) -> Vec<ClipFrames> {
    clips.iter().map(|c| c.to_frames(rate, source_frames)).filter(|c| c.len > 0).collect()
}

/// Deja una lista de clips en un estado válido para un audio de `source_secs`
/// segundos: dentro del archivo, sin duraciones mínimas, fundidos que caben en
/// el clip, identificadores únicos y orden por inicio.
pub fn sanitize_clips(clips: Vec<Clip>, source_secs: f64) -> Vec<Clip> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Clip> = clips
        .into_iter()
        .filter(|c| [c.start, c.source_start, c.length, c.fade_in, c.fade_out].iter().all(|v| v.is_finite()))
        .filter_map(|mut c| {
            c.start = c.start.max(0.0);
            c.source_start = c.source_start.clamp(0.0, source_secs.max(0.0));
            c.length = c.length.min(source_secs - c.source_start);
            if c.length < MIN_CLIP_SECONDS - 1e-9 {
                return None;
            }
            c.fade_in = c.fade_in.clamp(0.0, c.length);
            c.fade_out = c.fade_out.clamp(0.0, c.length);
            let total = c.fade_in + c.fade_out;
            if total > c.length {
                let k = c.length / total;
                c.fade_in *= k;
                c.fade_out *= k;
            }
            c.gain_db = if c.gain_db.is_finite() { c.gain_db.clamp(-60.0, 12.0) } else { 0.0 };
            if !seen.insert(c.id) {
                c.id = Uuid::new_v4();
            }
            Some(c)
        })
        .take(MAX_CLIPS)
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
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
            effects: Vec::new(),
            offset: 0.0,
            clips: Vec::new(),
            automation: Vec::new(),
        }
    }

    /// Convierte el formato antiguo (sin clips) en un clip con todo el audio y
    /// sanea los clips. Devuelve `true` si cambió algo.
    pub fn normalize_clips(&mut self, source_secs: f64) -> bool {
        let before = self.clips.clone();
        if self.clips.is_empty() {
            self.clips.push(Clip::whole(self.offset.max(0.0), source_secs));
        }
        let changed_offset = self.offset != 0.0;
        self.offset = 0.0;
        self.clips = sanitize_clips(std::mem::take(&mut self.clips), source_secs);
        changed_offset || self.clips != before
    }

    /// Final del último clip (s).
    pub fn end(&self) -> f64 {
        self.clips.iter().map(Clip::end).fold(0.0, f64::max)
    }
}

/// Bus máster: efectos y ganancia final (después del tempo y el tono).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MasterSettings {
    pub gain_db: f32,
    pub effects: Vec<EffectSlot>,
    /// Curvas de volumen del máster y de parámetros de sus efectos.
    pub automation: Vec<AutomationLane>,
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
    #[serde(default)]
    pub master: MasterSettings,
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
            master: MasterSettings::default(),
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
        assert_eq!(p.master, MasterSettings::default());
    }

    #[test]
    fn legacy_tracks_become_one_clip() {
        let mut t = TrackInfo::new("Toma", TrackKind::Recording, "audio/recordings/toma_001.wav");
        t.offset = 2.5;
        assert!(t.normalize_clips(10.0));
        assert_eq!(t.offset, 0.0);
        assert_eq!(t.clips.len(), 1);
        assert_eq!((t.clips[0].start, t.clips[0].source_start, t.clips[0].length), (2.5, 0.0, 10.0));
        assert_eq!(t.end(), 12.5);
        assert!(!t.normalize_clips(10.0), "segunda vez no cambia nada");
    }

    #[test]
    fn clips_are_sanitized() {
        let id = Uuid::new_v4();
        let clip = |start: f64, src: f64, len: f64, fi: f64, fo: f64| Clip {
            id,
            start,
            source_start: src,
            length: len,
            fade_in: fi,
            fade_out: fo,
            gain_db: 0.0,
        };
        let out = sanitize_clips(
            vec![
                clip(5.0, 8.0, 5.0, 0.0, 0.0),   // se recorta al final del audio (10 s)
                clip(-1.0, 0.0, 1.0, 0.8, 0.8),  // empieza en 0; fundidos escalados a 1 s
                clip(3.0, 9.999, 1.0, 0.0, 0.0), // queda por debajo del mínimo: fuera
                clip(f64::NAN, 0.0, 1.0, 0.0, 0.0),
            ],
            10.0,
        );
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].start, out[0].fade_in, out[0].fade_out), (0.0, 0.5, 0.5));
        assert_eq!((out[1].start, out[1].length), (5.0, 2.0));
        assert_ne!(out[0].id, out[1].id, "identificadores duplicados se renuevan");

        let f = out[1].to_frames(100, 1000);
        assert_eq!((f.start, f.src_start, f.len), (500, 800, 200));
        // Un audio más corto de lo previsto limita el clip sin desbordar.
        let f = out[1].to_frames(100, 900);
        assert_eq!((f.src_start, f.len), (800, 100));
        assert_eq!(out[1].to_frames(100, 700).len, 0);
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_dir_name("AC/DC: Back?"), "AC_DC_ Back_");
        assert_eq!(sanitize_dir_name("  ..  "), "Proyecto");
    }
}
