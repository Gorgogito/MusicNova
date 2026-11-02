//! Transcripción: notas por pista (audio a MIDI, `analysis/notes.json`), curva
//! de tono de la voz (`analysis/pitch.json`) e instrumentos presentes
//! (`analysis/instruments.json`). Incluye la exportación a MIDI estándar con
//! mapa de tempo y un sintetizador sencillo para escuchar las notas.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audio::AudioBuffer;
use crate::{CoreError, Result};

pub const NOTES_FILE: &str = "analysis/notes.json";
pub const PITCH_FILE: &str = "analysis/pitch.json";
pub const INSTRUMENTS_FILE: &str = "analysis/instruments.json";
pub const TRANSCRIPTION_VERSION: u32 = 1;

/// Duración mínima de una nota (s).
pub const MIN_NOTE: f64 = 0.02;

fn load_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&std::fs::read_to_string(path)?)?))
}

fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn check_version(v: u32, what: &str) -> Result<()> {
    if v > TRANSCRIPTION_VERSION {
        return Err(CoreError::Format(format!("{what} de una versión más nueva de MusicNova ({v})")));
    }
    Ok(())
}

// ---- Notas --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Note {
    /// Inicio y fin en segundos de la línea de tiempo del proyecto.
    pub start: f64,
    pub end: f64,
    /// Nota MIDI (60 = Do central).
    pub pitch: u8,
    pub velocity: u8,
}

/// Notas transcritas de una pista.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TrackNotes {
    pub track_id: Uuid,
    /// "poly" o "mono".
    pub mode: String,
    pub notes: Vec<Note>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Transcription {
    pub version: u32,
    pub tracks: Vec<TrackNotes>,
}

impl Default for Transcription {
    fn default() -> Self {
        Self { version: TRANSCRIPTION_VERSION, tracks: Vec::new() }
    }
}

/// Notas válidas y ordenadas: tiempos finitos y no negativos, duración mínima,
/// altura 0–127, velocidad 1–127.
pub fn sanitize_notes(notes: Vec<Note>) -> Vec<Note> {
    let mut out: Vec<Note> = notes
        .into_iter()
        .filter(|n| n.start.is_finite() && n.end.is_finite())
        .map(|mut n| {
            n.start = n.start.max(0.0);
            n.pitch = n.pitch.min(127);
            n.velocity = n.velocity.clamp(1, 127);
            n
        })
        .filter(|n| n.end - n.start >= MIN_NOTE - 1e-9)
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.pitch.cmp(&b.pitch)));
    out
}

impl Transcription {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(NOTES_FILE)
    }

    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let t: Option<Self> = load_json(&Self::path(dir))?;
        if let Some(t) = &t {
            check_version(t.version, "la transcripción es")?;
        }
        Ok(t)
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        save_json(&Self::path(dir), self)
    }

    pub fn track(&self, id: Uuid) -> Option<&TrackNotes> {
        self.tracks.iter().find(|t| t.track_id == id)
    }

    /// Sustituye (o añade) las notas de una pista.
    pub fn set_track(&mut self, track: TrackNotes) {
        let track = TrackNotes { notes: sanitize_notes(track.notes), ..track };
        match self.tracks.iter_mut().find(|t| t.track_id == track.track_id) {
            Some(t) => *t = track,
            None => self.tracks.push(track),
        }
    }

    /// Quita las notas de pistas que ya no existen.
    pub fn retain_tracks(&mut self, keep: impl Fn(Uuid) -> bool) {
        self.tracks.retain(|t| keep(t.track_id));
    }
}

/// Resultado del proceso de IA para una pista (`cache/notes_<pista>.json`).
#[derive(Clone, Debug, Deserialize)]
pub struct NotesResult {
    pub version: u32,
    pub mode: String,
    pub notes: Vec<Note>,
}

impl NotesResult {
    pub fn load(path: &Path) -> Result<Self> {
        let r: Self = load_json(path)?.ok_or_else(|| CoreError::Format("la transcripción no generó resultados".into()))?;
        check_version(r.version, "la transcripción es")?;
        Ok(r)
    }
}

// ---- Tono de la voz ------------------------------------------------------------------

/// Curva de tono de una pista de voz, a intervalos de `hop` segundos desde 0.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PitchCurve {
    pub version: u32,
    #[serde(default)]
    pub track_id: Option<Uuid>,
    pub source: String,
    pub hop: f64,
    /// Frecuencia fundamental en Hz (0 = sin voz).
    pub f0: Vec<f32>,
    /// Probabilidad de que haya voz (0–1).
    #[serde(default)]
    pub confidence: Vec<f32>,
}

impl PitchCurve {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(PITCH_FILE)
    }

    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let c: Option<Self> = load_json(&Self::path(dir))?;
        if let Some(c) = &c {
            check_version(c.version, "la curva de tono es")?;
            if !(c.hop > 0.0 && c.hop.is_finite()) {
                return Err(CoreError::Format("curva de tono con intervalo no válido".into()));
            }
        }
        Ok(c)
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        save_json(&Self::path(dir), self)
    }

    /// Nota MIDI continua (con decimales) de cada punto, `None` sin voz.
    pub fn midi(&self) -> impl Iterator<Item = Option<f32>> + '_ {
        self.f0.iter().map(|&f| (f > 0.0).then(|| 69.0 + 12.0 * (f / 440.0).log2()))
    }
}

// ---- Instrumentos -------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Instrument {
    pub id: String,
    pub label: String,
    pub confidence: f64,
    /// Fracción de la canción en la que suena.
    pub coverage: f64,
    /// Tramos (inicio, fin) en segundos.
    pub segments: Vec<[f64; 2]>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InstrumentReport {
    pub version: u32,
    pub source: String,
    pub duration: f64,
    pub instruments: Vec<Instrument>,
}

impl InstrumentReport {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(INSTRUMENTS_FILE)
    }

    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let r: Option<Self> = load_json(&Self::path(dir))?;
        if let Some(r) = &r {
            check_version(r.version, "la detección de instrumentos es")?;
        }
        Ok(r)
    }
}

// ---- MIDI estándar ------------------------------------------------------------------

/// Pulsos por negra del archivo MIDI.
pub const PPQ: u32 = 480;

/// Conversión de segundos a pulsos MIDI: o un tempo fijo o el mapa de tempo de
/// los pulsos detectados (cada pulso detectado = una negra), de modo que los
/// compases del archivo coincidan con los de la canción en cualquier DAW.
pub struct TempoMap {
    /// Instantes (s) de cada negra; el primero es 0. Más allá del último se
    /// sigue con el tempo del último tramo.
    beat_times: Vec<f64>,
}

impl TempoMap {
    pub fn constant(bpm: f64) -> Self {
        let bpm = if bpm.is_finite() && bpm > 1.0 { bpm } else { 120.0 };
        Self { beat_times: vec![0.0, 60.0 / bpm] }
    }

    /// A partir de los pulsos detectados. Antes del primero se añaden negras
    /// con el tempo inicial, tantas como hagan falta para que el primer tiempo
    /// fuerte (`first_downbeat`, índice en `beats`) caiga al principio de un compás.
    pub fn from_beats(beats: &[f64], first_downbeat: Option<usize>, beats_per_bar: u32) -> Self {
        let beats: Vec<f64> = beats.iter().copied().filter(|b| b.is_finite() && *b >= 0.0).collect();
        if beats.len() < 2 || beats.windows(2).any(|w| w[1] <= w[0]) {
            return Self::constant(120.0);
        }
        let d0 = beats[1] - beats[0];
        let mut pre = if beats[0] < 0.05 { 0 } else { ((beats[0] / d0).round() as usize).max(1) };
        if let Some(j) = first_downbeat {
            let bpb = beats_per_bar.max(1) as usize;
            pre += (bpb - (pre + j) % bpb) % bpb;
        }
        let mut times = Vec::with_capacity(beats.len() + pre + 1);
        times.push(0.0);
        for k in 1..pre {
            times.push(beats[0] * k as f64 / pre as f64);
        }
        if pre > 0 {
            times.push(beats[0]);
        }
        times.extend_from_slice(if pre > 0 || beats[0] < 0.05 { &beats[1..] } else { &beats[..] });
        if times[1] - times[0] <= 0.0 {
            return Self::constant(120.0);
        }
        Self { beat_times: times }
    }

    fn last_period(&self) -> f64 {
        let n = self.beat_times.len();
        self.beat_times[n - 1] - self.beat_times[n - 2]
    }

    /// Posición en pulsos MIDI de un instante.
    pub fn ticks(&self, t: f64) -> u64 {
        let t = t.max(0.0);
        let bt = &self.beat_times;
        let beat = match bt.partition_point(|&b| b <= t) {
            0 => 0.0,
            i if i >= bt.len() => (bt.len() - 1) as f64 + (t - bt[bt.len() - 1]) / self.last_period(),
            i => (i - 1) as f64 + (t - bt[i - 1]) / (bt[i] - bt[i - 1]),
        };
        (beat * PPQ as f64).round() as u64
    }

    /// Cambios de tempo (pulso, microsegundos por negra), omitiendo los que
    /// varían menos de un 0,5 % respecto al anterior.
    fn tempo_events(&self) -> Vec<(u64, u32)> {
        let mut out: Vec<(u64, u32)> = Vec::new();
        for (k, w) in self.beat_times.windows(2).enumerate() {
            let us = ((w[1] - w[0]) * 1e6).round().clamp(1.0, 16_777_215.0) as u32;
            if out.last().is_none_or(|&(_, prev)| (us as f64 / prev as f64 - 1.0).abs() > 0.005) {
                out.push((k as u64 * PPQ as u64, us));
            }
        }
        out
    }
}

fn vlq(out: &mut Vec<u8>, mut v: u32) {
    let mut buf = [0u8; 5];
    let mut i = 4;
    buf[i] = (v & 0x7f) as u8;
    v >>= 7;
    while v > 0 {
        i -= 1;
        buf[i] = ((v & 0x7f) as u8) | 0x80;
        v >>= 7;
    }
    out.extend_from_slice(&buf[i..]);
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(data);
}

/// Pista MIDI a partir de eventos (pulso, bytes); los ordena y añade el final.
fn track_bytes(mut events: Vec<(u64, u8, Vec<u8>)>) -> Vec<u8> {
    // A igual pulso: primero los meta, luego los «note off», luego los «note on».
    events.sort_by_key(|e| (e.0, e.1));
    let mut data = Vec::new();
    let mut last = 0u64;
    for (tick, _, bytes) in events {
        vlq(&mut data, (tick - last).min(u32::MAX as u64 >> 4) as u32);
        data.extend_from_slice(&bytes);
        last = tick;
    }
    data.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
    data
}

fn meta_text(kind: u8, text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut v = vec![0xff, kind];
    vlq(&mut v, bytes.len() as u32);
    v.extend_from_slice(bytes);
    v
}

/// Una pista del archivo MIDI.
pub struct MidiPart<'a> {
    pub name: &'a str,
    /// Programa General MIDI (0 = piano).
    pub program: u8,
    pub notes: &'a [Note],
}

/// Archivo MIDI estándar de formato 1: una pista de tempo y una por parte, cada
/// una en su canal (se salta el 10, reservado a la percusión).
pub fn write_midi(parts: &[MidiPart], tempo: &TempoMap, beats_per_bar: u32, title: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut header = Vec::new();
    header.extend_from_slice(&1u16.to_be_bytes());
    header.extend_from_slice(&((parts.len() + 1) as u16).to_be_bytes());
    header.extend_from_slice(&(PPQ as u16).to_be_bytes());
    chunk(&mut out, b"MThd", &header);

    let mut meta = vec![(0u64, 0u8, meta_text(0x03, title))];
    let num = beats_per_bar.clamp(1, 32) as u8;
    meta.push((0, 0, vec![0xff, 0x58, 0x04, num, 2, 24, 8]));
    for (tick, us) in tempo.tempo_events() {
        meta.push((tick, 0, vec![0xff, 0x51, 0x03, (us >> 16) as u8, (us >> 8) as u8, us as u8]));
    }
    chunk(&mut out, b"MTrk", &track_bytes(meta));

    for (i, part) in parts.iter().enumerate() {
        let ch = {
            let c = (i % 15) as u8;
            if c >= 9 { c + 1 } else { c }
        };
        let mut ev = vec![(0u64, 0u8, meta_text(0x03, part.name)), (0, 0, vec![0xc0 | ch, part.program.min(127)])];
        for n in part.notes {
            let (a, b) = (tempo.ticks(n.start), tempo.ticks(n.end));
            let b = b.max(a + 1);
            ev.push((a, 2, vec![0x90 | ch, n.pitch.min(127), n.velocity.clamp(1, 127)]));
            ev.push((b, 1, vec![0x80 | ch, n.pitch.min(127), 0]));
        }
        chunk(&mut out, b"MTrk", &track_bytes(ev));
    }
    out
}

// ---- Síntesis -------------------------------------------------------------------------

/// Timbre del sintetizador para escuchar una transcripción.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Timbre {
    /// Tipo piano eléctrico: armónicos que decaen.
    Keys,
    /// Graves redondos (seno y segundo armónico).
    Bass,
    /// Sostenido, para voces y melodías.
    Lead,
}

/// Sintetiza las notas en estéreo a `rate`, con la duración `length` (s) o la
/// de la última nota. Normaliza a −1 dBFS si satura.
pub fn synthesize(notes: &[Note], rate: u32, length: Option<f64>, timbre: Timbre) -> AudioBuffer {
    let rel = 0.08;
    let end = length.unwrap_or_else(|| notes.iter().map(|n| n.end).fold(0.0, f64::max) + rel);
    let frames = (end.max(0.0) * rate as f64).ceil() as usize;
    let mut mono = vec![0.0f32; frames];
    let harmonics: &[f32] = match timbre {
        Timbre::Keys => &[1.0, 0.45, 0.25, 0.12, 0.06],
        Timbre::Bass => &[1.0, 0.35, 0.08],
        Timbre::Lead => &[1.0, 0.5, 0.33, 0.25, 0.2, 0.16],
    };
    let sr = rate as f32;
    for n in notes {
        let f0 = 440.0 * 2f32.powf((n.pitch as f32 - 69.0) / 12.0);
        let amp = 0.18 * (n.velocity as f32 / 127.0).powf(1.5);
        let a = (n.start * rate as f64) as usize;
        let hold = ((n.end - n.start) * rate as f64) as usize;
        let total = hold + (rel * rate as f64) as usize;
        let attack = (0.005 * sr) as usize;
        for k in 0..total {
            let i = a + k;
            if i >= frames {
                break;
            }
            let t = k as f32 / sr;
            let mut env = if k < attack { k as f32 / attack as f32 } else { 1.0 };
            env *= match timbre {
                Timbre::Keys => (-t * 2.2).exp(),
                Timbre::Bass => (-t * 1.2).exp(),
                Timbre::Lead => 0.85 + 0.15 * (-t * 6.0).exp(),
            };
            if k >= hold {
                env *= 1.0 - (k - hold) as f32 / (total - hold).max(1) as f32;
            }
            let mut s = 0.0;
            for (h, &w) in harmonics.iter().enumerate() {
                let f = f0 * (h + 1) as f32;
                if f >= sr * 0.45 {
                    break;
                }
                s += w * (std::f32::consts::TAU * f * t).sin();
            }
            mono[i] += amp * env * s;
        }
    }
    let peak = mono.iter().fold(0f32, |m, x| m.max(x.abs()));
    let limit = 10f32.powf(-1.0 / 20.0);
    if peak > limit {
        let g = limit / peak;
        mono.iter_mut().for_each(|x| *x *= g);
    }
    AudioBuffer { sample_rate: rate, data: mono.iter().flat_map(|&s| [s, s]).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: f64, end: f64, pitch: u8) -> Note {
        Note { start, end, pitch, velocity: 100 }
    }

    /// El JSON tal como lo escribe el proceso de IA (con campos que aquí no se usan).
    #[test]
    fn reads_worker_output() {
        let dir = std::env::temp_dir().join(format!("mn_tx_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("analysis")).unwrap();
        let notes = dir.join("notes.json");
        std::fs::write(&notes, r#"{"version":1,"source":"basic_pitch","mode":"mono","notes":[{"start":0.5,"end":1.0,"pitch":60,"velocity":98}],"seconds":0.2}"#).unwrap();
        let r = NotesResult::load(&notes).unwrap();
        assert_eq!((r.mode.as_str(), r.notes[0].pitch, r.notes[0].velocity), ("mono", 60, 98));

        std::fs::write(dir.join(PITCH_FILE), r#"{"version":1,"source":"pyin","hop":0.01,"f0":[0.0,440.0,0.0],"confidence":[0.0,0.9,0.1],"seconds":1.2}"#).unwrap();
        let c = PitchCurve::load(&dir).unwrap().unwrap();
        assert_eq!(c.track_id, None);
        assert_eq!(c.midi().collect::<Vec<_>>(), vec![None, Some(69.0), None]);

        std::fs::write(
            dir.join(INSTRUMENTS_FILE),
            r#"{"version":1,"source":"panns_cnn14","window":3.0,"hop":1.0,"duration":12.5,"instruments":[{"id":"piano","label":"Piano","confidence":0.8,"coverage":0.6,"segments":[[0.0,6.0]]}],"seconds":3.0,"device":"cpu"}"#,
        )
        .unwrap();
        let i = InstrumentReport::load(&dir).unwrap().unwrap();
        assert_eq!(i.instruments[0].segments, vec![[0.0, 6.0]]);

        let mut t = Transcription::default();
        let id = Uuid::new_v4();
        t.set_track(TrackNotes { track_id: id, mode: "poly".into(), notes: r.notes });
        t.save(&dir).unwrap();
        assert_eq!(Transcription::load(&dir).unwrap().unwrap().track(id).unwrap().notes.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sanitizes_and_sorts_notes() {
        let notes = sanitize_notes(vec![
            note(1.0, 1.5, 64),
            note(-0.5, 0.2, 200),
            note(0.5, 0.505, 60),
            Note { start: f64::NAN, end: 1.0, pitch: 60, velocity: 1 },
        ]);
        assert_eq!(notes.len(), 2);
        assert_eq!((notes[0].start, notes[0].pitch), (0.0, 127));
        assert_eq!(notes[1].pitch, 64);
    }

    #[test]
    fn tempo_map_follows_beats_and_aligns_bars() {
        // Pulsos a 100 BPM desde 0,9 s; el primer tiempo fuerte es el segundo pulso.
        let beats: Vec<f64> = (0..20).map(|k| 0.9 + 0.6 * k as f64).collect();
        let map = TempoMap::from_beats(&beats, Some(1), 4);
        // Antes de 0,9 s caben 2 negras (0,45 s cada una) y se añade 1 para
        // que el tiempo fuerte (pulso 1) caiga en la negra 4 = compás 2.
        assert_eq!(map.ticks(beats[1]), 4 * PPQ as u64);
        assert_eq!(map.ticks(beats[1] + 0.3), 4 * PPQ as u64 + PPQ as u64 / 2);
        // Más allá del último pulso sigue el último tempo.
        assert_eq!(map.ticks(beats[19] + 0.6), map.ticks(beats[19]) + PPQ as u64);
        let ev = map.tempo_events();
        assert_eq!(ev.last().unwrap().1, 600_000);
    }

    #[test]
    fn constant_tempo_ticks() {
        let map = TempoMap::constant(120.0);
        assert_eq!(map.ticks(1.0), 2 * PPQ as u64);
        assert_eq!(map.tempo_events(), vec![(0, 500_000)]);
    }

    /// Lector mínimo de MIDI para comprobar lo escrito: (pista, pulso, estado, datos).
    fn parse(bytes: &[u8]) -> (u16, Vec<Vec<(u64, Vec<u8>)>>) {
        assert_eq!(&bytes[..4], b"MThd");
        let ntracks = u16::from_be_bytes([bytes[10], bytes[11]]);
        let mut pos = 14;
        let mut tracks = Vec::new();
        while pos < bytes.len() {
            assert_eq!(&bytes[pos..pos + 4], b"MTrk");
            let len = u32::from_be_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
            let data = &bytes[pos + 8..pos + 8 + len];
            pos += 8 + len;
            let mut i = 0;
            let mut tick = 0u64;
            let mut events = Vec::new();
            while i < data.len() {
                let mut delta = 0u64;
                loop {
                    let b = data[i];
                    i += 1;
                    delta = (delta << 7) | (b & 0x7f) as u64;
                    if b & 0x80 == 0 {
                        break;
                    }
                }
                tick += delta;
                let status = data[i];
                let n = match status {
                    0xff => 3 + data[i + 2] as usize,
                    s if s & 0xf0 == 0xc0 => 2,
                    _ => 3,
                };
                events.push((tick, data[i..i + n].to_vec()));
                i += n;
            }
            tracks.push(events);
        }
        (ntracks, tracks)
    }

    #[test]
    fn writes_valid_midi() {
        let notes = [note(0.0, 0.5, 60), note(0.5, 1.0, 64), note(0.5, 1.0, 67)];
        let bass = [note(0.0, 1.0, 36)];
        let parts = [
            MidiPart { name: "Piano", program: 0, notes: &notes },
            MidiPart { name: "Bajo", program: 33, notes: &bass },
        ];
        let bytes = write_midi(&parts, &TempoMap::constant(120.0), 4, "Canción");
        let (n, tracks) = parse(&bytes);
        assert_eq!(n, 3);
        assert_eq!(tracks.len(), 3);
        // Pista de tempo: título, compás 4/4 y 120 BPM.
        assert!(tracks[0].iter().any(|(_, e)| e == &vec![0xff, 0x51, 0x03, 0x07, 0xa1, 0x20]));
        assert!(tracks[0].iter().any(|(_, e)| e[..4] == [0xff, 0x58, 0x04, 4]));
        // Piano: el «note off» de la primera nota va antes de los «note on» del mismo pulso.
        let piano: Vec<_> = tracks[1].iter().filter(|(_, e)| e[0] & 0xf0 == 0x90 || e[0] & 0xf0 == 0x80).collect();
        assert_eq!(piano.len(), 6);
        assert_eq!(piano[0], &(0, vec![0x90, 60, 100]));
        assert_eq!(piano[1], &(480, vec![0x80, 60, 0]));
        assert_eq!(piano[2].1[0], 0x90);
        // Bajo en el canal 2 con el programa 33.
        assert!(tracks[2].iter().any(|(_, e)| e == &vec![0xc1, 33]));
        assert!(tracks[2].iter().any(|(t, e)| *t == 960 && e == &vec![0x81, 36, 0]));
        assert_eq!(tracks[2].last().unwrap().1, vec![0xff, 0x2f, 0x00]);
    }

    #[test]
    fn synthesizes_the_right_pitch() {
        let rate = 48_000;
        let buf = synthesize(&[note(0.1, 0.6, 69)], rate, Some(1.0), Timbre::Bass);
        assert_eq!(buf.frames(), rate as usize);
        let left: Vec<f32> = buf.data.chunks_exact(2).map(|f| f[0]).collect();
        assert!(left[..rate as usize / 20].iter().all(|&s| s == 0.0), "silencio antes de la nota");
        // Frecuencia fundamental por cruces por cero ascendentes en el tramo sostenido.
        let seg = &left[(0.2 * rate as f64) as usize..(0.5 * rate as f64) as usize];
        let ups = seg.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        let hz = ups as f64 / 0.3;
        assert!((hz - 440.0).abs() < 8.0, "{hz}");
        assert!(left[(0.8 * rate as f64) as usize..].iter().all(|&s| s == 0.0), "silencio tras la liberación");
    }
}
