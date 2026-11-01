//! Resultado del análisis musical (`analysis/analysis.json` dentro del proyecto).
//!
//! Lo escribe el proceso de IA; el núcleo solo lo lee, lo valida y lo convierte
//! en la rejilla de pulsos del metrónomo. Las ediciones manuales (acordes,
//! secciones) se guardan en el mismo archivo.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{CoreError, Result};

pub const ANALYSIS_FILE: &str = "analysis/analysis.json";
pub const ANALYSIS_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct KeyAlternative {
    pub tonic: u8,
    pub mode: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Key {
    /// 0 = Do ... 11 = Si
    pub tonic: u8,
    /// "major" | "minor"
    pub mode: String,
    pub label: String,
    #[serde(default)]
    pub confidence: f64,
    #[serde(default)]
    pub alternatives: Vec<KeyAlternative>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Chord {
    pub start: f64,
    pub end: f64,
    /// Etiqueta en notación inglesa ("Am7") o "N" sin acorde.
    pub label: String,
    pub root: Option<u8>,
    pub quality: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Section {
    pub start: f64,
    pub end: f64,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Analysis {
    pub version: u32,
    pub duration: f64,
    pub beat_source: String,
    pub chord_source: String,
    pub bpm: f64,
    pub tempo_stable: bool,
    #[serde(default)]
    pub tempo_curve: Vec<[f64; 2]>,
    pub beats_per_bar: u32,
    pub beats: Vec<f64>,
    pub downbeats: Vec<f64>,
    pub key: Key,
    pub chords: Vec<Chord>,
    pub sections: Vec<Section>,
    #[serde(default)]
    pub seconds: f64,
    #[serde(default)]
    pub device: String,
}

impl Analysis {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(ANALYSIS_FILE)
    }

    /// Lee el análisis del proyecto, si existe.
    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let path = Self::path(dir);
        if !path.is_file() {
            return Ok(None);
        }
        let a: Analysis = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        a.validate()?;
        Ok(Some(a))
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        self.validate()?;
        let path = Self::path(dir);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.version > ANALYSIS_VERSION {
            return Err(CoreError::Format(format!(
                "el análisis usa la versión {} y esta aplicación solo admite hasta la {}",
                self.version, ANALYSIS_VERSION
            )));
        }
        let sorted = |v: &[f64]| v.windows(2).all(|w| w[0] <= w[1]) && v.iter().all(|x| x.is_finite());
        if !sorted(&self.beats) || !sorted(&self.downbeats) {
            return Err(CoreError::Format("los pulsos del análisis no están ordenados".into()));
        }
        for c in &self.chords {
            if !(c.end >= c.start) || c.root.is_some_and(|r| r > 11) {
                return Err(CoreError::Format(format!("acorde inválido: {}", c.label)));
            }
        }
        Ok(())
    }

    /// Pulsos en frames a `rate` Hz y si cada uno es tiempo fuerte.
    ///
    /// Si el detector no dio tiempos fuertes, se acentúa cada `beats_per_bar`.
    pub fn beat_grid(&self, rate: u32) -> (Vec<u64>, Vec<bool>) {
        let frames: Vec<u64> = self.beats.iter().map(|&t| (t.max(0.0) * rate as f64).round() as u64).collect();
        let accents: Vec<bool> = if self.downbeats.is_empty() {
            let bpb = self.beats_per_bar.max(1) as usize;
            (0..self.beats.len()).map(|i| i % bpb == 0).collect()
        } else {
            self.beats
                .iter()
                .map(|&b| {
                    let i = self.downbeats.partition_point(|&d| d < b - 0.03);
                    self.downbeats.get(i).is_some_and(|&d| (d - b).abs() <= 0.03)
                })
                .collect()
        };
        (frames, accents)
    }

    /// Primer tiempo fuerte (para alinear la rejilla manual con el análisis).
    pub fn first_downbeat(&self) -> f64 {
        self.downbeats.first().or(self.beats.first()).copied().unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Analysis {
        Analysis {
            version: 1,
            duration: 10.0,
            beat_source: "beat_this".into(),
            chord_source: "stems".into(),
            bpm: 120.0,
            tempo_stable: true,
            tempo_curve: vec![[0.5, 120.0]],
            beats_per_bar: 4,
            beats: vec![0.5, 1.0, 1.5, 2.0, 2.5, 3.0],
            downbeats: vec![1.0, 3.0],
            key: Key { tonic: 5, mode: "major".into(), label: "F".into(), confidence: 0.5, alternatives: vec![] },
            chords: vec![Chord { start: 0.0, end: 2.0, label: "F".into(), root: Some(5), quality: Some("".into()) }],
            sections: vec![Section { start: 0.0, end: 10.0, label: "A".into() }],
            seconds: 1.0,
            device: "cuda".into(),
        }
    }

    #[test]
    fn grid_uses_downbeats() {
        let (frames, accents) = sample().beat_grid(1000);
        assert_eq!(frames, vec![500, 1000, 1500, 2000, 2500, 3000]);
        assert_eq!(accents, vec![false, true, false, false, false, true]);
    }

    #[test]
    fn grid_without_downbeats_counts_bars() {
        let mut a = sample();
        a.downbeats.clear();
        a.beats_per_bar = 3;
        assert_eq!(a.beat_grid(1000).1, vec![true, false, false, true, false, false]);
    }

    #[test]
    fn roundtrip_and_python_format() {
        let dir = std::env::temp_dir().join(format!("mn_an_{}", uuid::Uuid::new_v4()));
        let a = sample();
        a.save(&dir).unwrap();
        assert_eq!(Analysis::load(&dir).unwrap(), Some(a));
        std::fs::remove_dir_all(&dir).ok();
        // Formato tal como lo escribe el proceso de IA (acordes N con nulos).
        let json = r#"{"version":1,"duration":5,"beat_source":"librosa","chord_source":"mix","bpm":100.0,
            "tempo_stable":false,"tempo_curve":[[0.6,99.5]],"beats_per_bar":4,"beats":[0.6,1.2],"downbeats":[],
            "key":{"tonic":9,"mode":"minor","label":"Am","confidence":0.3,"alternatives":[{"tonic":0,"mode":"major","label":"C"}]},
            "chords":[{"start":0.0,"end":0.6,"label":"N","root":null,"quality":null}],
            "sections":[{"start":0.0,"end":5.0,"label":"A"}],"seconds":2.1,"device":"cpu"}"#;
        let a: Analysis = serde_json::from_str(json).unwrap();
        a.validate().unwrap();
        assert_eq!(a.chords[0].root, None);
    }

    #[test]
    fn rejects_unsorted_beats() {
        let mut a = sample();
        a.beats = vec![1.0, 0.5];
        assert!(a.validate().is_err());
    }
}
