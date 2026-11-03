//! Transcripción: notas por pista (audio a MIDI, `analysis/notes.json`), curva
//! de tono de la voz (`analysis/pitch.json`) e instrumentos presentes
//! (`analysis/instruments.json`). Incluye la exportación a MIDI estándar con
//! mapa de tempo y un sintetizador sencillo para escuchar las notas.

use std::collections::HashMap;
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
/// Límite de las curvas de tono (cents): ±2 semitonos, el rango estándar del
/// pitch bend MIDI, que así suena bien aunque el sintetizador ignore el RPN.
pub const BEND_LIMIT: f32 = 200.0;
/// Canal MIDI de la percusión (el 10 contando desde 1).
pub const DRUM_CHANNEL: u8 = 9;
/// Modo de las notas de batería (alturas del mapa de percusión General MIDI).
pub const DRUMS_MODE: &str = "drums";

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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Note {
    /// Inicio y fin en segundos de la línea de tiempo del proyecto.
    pub start: f64,
    pub end: f64,
    /// Nota MIDI (60 = Do central).
    pub pitch: u8,
    pub velocity: u8,
    /// Curva de tono dentro de la nota: puntos [segundos desde el inicio, cents]
    /// unidos por rectas. Vacía = altura fija.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bends: Vec<[f32; 2]>,
}

impl Note {
    pub fn new(start: f64, end: f64, pitch: u8, velocity: u8) -> Self {
        Self { start, end, pitch, velocity, bends: Vec::new() }
    }

    /// Desvío (cents) a `dt` segundos del inicio de la nota.
    pub fn bend_at(&self, dt: f64) -> f32 {
        let b = &self.bends;
        let Some(first) = b.first() else { return 0.0 };
        let dt = dt as f32;
        if dt <= first[0] {
            return first[1];
        }
        for w in b.windows(2) {
            if dt <= w[1][0] {
                let span = w[1][0] - w[0][0];
                return if span > 0.0 { w[0][1] + (w[1][1] - w[0][1]) * (dt - w[0][0]) / span } else { w[1][1] };
            }
        }
        b[b.len() - 1][1]
    }
}

/// Curva válida: puntos finitos, tiempos no negativos y ordenados, desvíos
/// dentro de ±BEND_LIMIT. Vacía si nunca se aparta de la nota.
fn sanitize_bends(mut b: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    b.retain(|p| p[0].is_finite() && p[1].is_finite());
    for p in &mut b {
        p[0] = p[0].max(0.0);
        p[1] = p[1].clamp(-BEND_LIMIT, BEND_LIMIT);
    }
    b.sort_by(|x, y| x[0].total_cmp(&y[0]));
    if b.iter().all(|p| p[1].abs() < 0.5) {
        b.clear();
    }
    b
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
            n.bends = sanitize_bends(std::mem::take(&mut n.bends));
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
    /// Stems separados con los que se analizó (vacío = solo la mezcla).
    #[serde(default)]
    pub stems: Vec<String>,
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

// Orden de los eventos que caen en el mismo pulso.
const EV_SETUP: u8 = 0;
const EV_OFF: u8 = 1;
const EV_BEND_RESET: u8 = 2;
const EV_BEND: u8 = 3;
const EV_ON: u8 = 4;

/// Pista MIDI a partir de eventos (pulso, orden, bytes); los ordena y añade el final.
fn track_bytes(mut events: Vec<(u64, u8, Vec<u8>)>) -> Vec<u8> {
    // A igual pulso: meta y ajustes, «note off», vuelta del bend al centro,
    // bend inicial de la nota siguiente y por último los «note on».
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
    /// Percusión: va al canal 10 y las alturas son instrumentos del mapa GM.
    pub drums: bool,
}

/// Valor de pitch bend de 14 bits para un desvío en cents (rango ±BEND_LIMIT).
fn bend_value(cents: f32) -> u16 {
    (8192.0 + cents.clamp(-BEND_LIMIT, BEND_LIMIT) / BEND_LIMIT * 8191.0).round() as u16
}

fn bend_event(ch: u8, v: u16) -> Vec<u8> {
    vec![0xe0 | ch, (v & 0x7f) as u8, (v >> 7) as u8]
}

/// Canal de cada nota de una parte. El pitch bend afecta a todo el canal, así
/// que una nota con curva necesita uno para ella sola mientras suena: se toma
/// el canal propio de la parte si está libre y si no uno de reserva. Sin
/// canales libres la nota pierde la curva (`false`).
fn assign_channels(notes: &[Note], tempo: &TempoMap, own: &mut Vec<u8>, spare: &mut Vec<u8>) -> Vec<(u8, bool)> {
    // (canal, pulso final, con curva) de lo que suena.
    let mut busy: Vec<(u8, u64, bool)> = Vec::new();
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by(|&a, &b| notes[a].start.total_cmp(&notes[b].start));
    let mut out = vec![(own[0], false); notes.len()];
    for i in order {
        let n = &notes[i];
        let (a, b) = (tempo.ticks(n.start), tempo.ticks(n.end).max(tempo.ticks(n.start) + 1));
        busy.retain(|&(_, end, _)| end > a);
        let bent = !n.bends.is_empty();
        let free = |ch: u8, busy: &[(u8, u64, bool)]| busy.iter().filter(|x| x.0 == ch).all(|x| !bent && !x.2);
        let ch = match own.iter().copied().find(|&c| free(c, &busy)) {
            Some(c) => Some(c),
            None => spare.pop().inspect(|&c| own.push(c)),
        };
        let (ch, keep) = match ch {
            Some(c) => (c, bent),
            // Sin canal libre: la nota va sin curva en un canal donde no haya
            // otra con curva sonando, o en el propio si no queda otro remedio.
            None => {
                (own.iter().copied().find(|&c| busy.iter().filter(|x| x.0 == c).all(|x| !x.2)).unwrap_or(own[0]), false)
            }
        };
        busy.push((ch, b, keep));
        out[i] = (ch, keep);
    }
    out
}

/// Archivo MIDI estándar de formato 1: una pista de tempo y una por parte, cada
/// una en su canal (el 10 queda para la percusión). Las notas con curva de tono
/// llevan pitch bend (rango de ±2 semitonos fijado con RPN 0) y, si se solapan,
/// se reparten entre canales libres.
pub fn write_midi(parts: &[MidiPart], tempo: &TempoMap, beats_per_bar: u32, title: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut header = Vec::new();
    header.extend_from_slice(&1u16.to_be_bytes());
    header.extend_from_slice(&((parts.len() + 1) as u16).to_be_bytes());
    header.extend_from_slice(&(PPQ as u16).to_be_bytes());
    chunk(&mut out, b"MThd", &header);

    let mut meta = vec![(0u64, EV_SETUP, meta_text(0x03, title))];
    let num = beats_per_bar.clamp(1, 32) as u8;
    meta.push((0, EV_SETUP, vec![0xff, 0x58, 0x04, num, 2, 24, 8]));
    for (tick, us) in tempo.tempo_events() {
        meta.push((tick, EV_SETUP, vec![0xff, 0x51, 0x03, (us >> 16) as u8, (us >> 8) as u8, us as u8]));
    }
    chunk(&mut out, b"MTrk", &track_bytes(meta));

    // Canal propio de cada parte melódica (se salta el 10); los que sobran
    // quedan de reserva para notas con curva que se solapan.
    let melodic: Vec<usize> = (0..parts.len()).filter(|&i| !parts[i].drums).collect();
    let mut own_channel = vec![DRUM_CHANNEL; parts.len()];
    for (k, &i) in melodic.iter().enumerate() {
        let c = (k % 15) as u8;
        own_channel[i] = if c >= DRUM_CHANNEL { c + 1 } else { c };
    }
    let mut spare: Vec<u8> = (0..16u8).rev().filter(|c| *c != DRUM_CHANNEL && !own_channel.contains(c)).collect();

    for (i, part) in parts.iter().enumerate() {
        let mut ev = vec![(0u64, EV_SETUP, meta_text(0x03, part.name))];
        let mut channels = vec![own_channel[i]];
        let assigned = if part.drums {
            vec![(DRUM_CHANNEL, false); part.notes.len()]
        } else {
            assign_channels(part.notes, tempo, &mut channels, &mut spare)
        };
        let any_bends = assigned.iter().any(|a| a.1);
        for &ch in &channels {
            if !part.drums {
                ev.push((0, EV_SETUP, vec![0xc0 | ch, part.program.min(127)]));
            }
            if any_bends {
                // RPN 0 (sensibilidad del pitch bend) = 2 semitonos, y RPN nulo.
                let semis = (BEND_LIMIT / 100.0) as u8;
                for (cc, v) in [(101, 0), (100, 0), (6, semis), (38, 0), (101, 127), (100, 127)] {
                    ev.push((0, EV_SETUP, vec![0xb0 | ch, cc, v]));
                }
            }
        }
        for (n, &(ch, bent)) in part.notes.iter().zip(&assigned) {
            let (a, b) = (tempo.ticks(n.start), tempo.ticks(n.end));
            let b = b.max(a + 1);
            if bent {
                // La curva se muestrea cada 10 ms y solo se escribe cuando cambia.
                let mut last = None;
                let steps = ((n.end - n.start) / 0.01).ceil().max(1.0) as usize;
                for k in 0..steps {
                    let dt = k as f64 * 0.01;
                    let tick = tempo.ticks(n.start + dt);
                    if k > 0 && tick >= b {
                        break;
                    }
                    let v = bend_value(n.bend_at(dt));
                    if last != Some(v) {
                        ev.push((tick.max(a), EV_BEND, bend_event(ch, v)));
                        last = Some(v);
                    }
                }
                ev.push((b, EV_BEND_RESET, bend_event(ch, 8192)));
            }
            ev.push((a, EV_ON, vec![0x90 | ch, n.pitch.min(127), n.velocity.clamp(1, 127)]));
            ev.push((b, EV_OFF, vec![0x80 | ch, n.pitch.min(127), 0]));
        }
        chunk(&mut out, b"MTrk", &track_bytes(ev));
    }
    out
}

// ---- Lectura de MIDI estándar ---------------------------------------------------------

/// Una pista (o un canal) de un archivo MIDI importado.
#[derive(Clone, Debug, PartialEq)]
pub struct MidiTrackNotes {
    pub name: String,
    pub channel: u8,
    /// Último cambio de programa General MIDI del canal (0 si no hay).
    pub program: u8,
    /// Canal 10: percusión.
    pub drums: bool,
    pub notes: Vec<Note>,
}

struct MidiReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> MidiReader<'a> {
    fn byte(&mut self) -> Result<u8> {
        let b = *self.data.get(self.pos).ok_or_else(|| CoreError::Format("MIDI truncado".into()))?;
        self.pos += 1;
        Ok(b)
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.data.len()).ok_or_else(|| CoreError::Format("MIDI truncado".into()))?;
        let out = &self.data[self.pos..end];
        self.pos = end;
        Ok(out)
    }
    fn vlq(&mut self) -> Result<u32> {
        let mut v = 0u32;
        for _ in 0..4 {
            let b = self.byte()?;
            v = (v << 7) | (b & 0x7f) as u32;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(CoreError::Format("número de longitud variable no válido en el MIDI".into()))
    }
}

enum RawEvent {
    NoteOn(u8, u8, u8),
    NoteOff(u8, u8),
    Program(u8, u8),
    Bend(u8, u16),
    Tempo(u32),
    Name(String),
    Rpn(u8, u8),
}

/// Lee un archivo MIDI estándar (formato 0 o 1): notas por pista y canal en
/// segundos (con el mapa de tempo), pitch bend como curva de cada nota
/// (rango por RPN 0, 2 semitonos si no se indica) y el canal 10 como batería.
pub fn read_midi(bytes: &[u8]) -> Result<Vec<MidiTrackNotes>> {
    let mut r = MidiReader { data: bytes, pos: 0 };
    if r.take(4)? != b"MThd" {
        return Err(CoreError::Format("no es un archivo MIDI".into()));
    }
    let hlen = u32::from_be_bytes(r.take(4)?.try_into().unwrap()) as usize;
    let header = r.take(hlen)?;
    if header.len() < 6 {
        return Err(CoreError::Format("cabecera MIDI corta".into()));
    }
    let ntracks = u16::from_be_bytes([header[2], header[3]]) as usize;
    let division = u16::from_be_bytes([header[4], header[5]]);
    if division & 0x8000 != 0 {
        return Err(CoreError::Format("MIDI con tiempo SMPTE: no compatible".into()));
    }
    let ppq = division.max(1) as f64;

    // (pista, pulso, orden, evento)
    let mut events: Vec<(usize, u64, usize, RawEvent)> = Vec::new();
    for track in 0..ntracks {
        if r.pos + 8 > bytes.len() {
            break;
        }
        let id = r.take(4)?;
        let len = u32::from_be_bytes(r.take(4)?.try_into().unwrap()) as usize;
        let chunk = r.take(len)?;
        if id != b"MTrk" {
            continue;
        }
        let mut t = MidiReader { data: chunk, pos: 0 };
        let (mut tick, mut status) = (0u64, 0u8);
        let mut rpn = [[127u8; 2]; 16];
        while t.pos < chunk.len() {
            tick += t.vlq()? as u64;
            let mut b = t.byte()?;
            if b < 0x80 {
                // Estado en curso: el byte ya es un dato.
                t.pos -= 1;
                b = status;
            } else if b < 0xf0 {
                status = b;
            }
            let order = events.len();
            match b {
                0xff => {
                    let kind = t.byte()?;
                    let n = t.vlq()? as usize;
                    let d = t.take(n)?;
                    match kind {
                        0x51 if n == 3 => events.push((track, tick, order, RawEvent::Tempo(u32::from_be_bytes([0, d[0], d[1], d[2]])))),
                        0x03 => events.push((track, tick, order, RawEvent::Name(String::from_utf8_lossy(d).trim().to_string()))),
                        0x2f => break,
                        _ => {}
                    }
                }
                0xf0 | 0xf7 => {
                    let n = t.vlq()? as usize;
                    t.take(n)?;
                }
                _ if b >= 0x80 => {
                    let ch = b & 0x0f;
                    match b & 0xf0 {
                        0x80 => {
                            let (k, _) = (t.byte()?, t.byte()?);
                            events.push((track, tick, order, RawEvent::NoteOff(ch, k)));
                        }
                        0x90 => {
                            let (k, v) = (t.byte()?, t.byte()?);
                            events.push((track, tick, order, if v == 0 { RawEvent::NoteOff(ch, k) } else { RawEvent::NoteOn(ch, k, v) }));
                        }
                        0xa0 => {
                            t.take(2)?;
                        }
                        0xb0 => {
                            let (cc, v) = (t.byte()?, t.byte()?);
                            match cc {
                                101 => rpn[ch as usize][0] = v,
                                100 => rpn[ch as usize][1] = v,
                                6 if rpn[ch as usize] == [0, 0] => events.push((track, tick, order, RawEvent::Rpn(ch, v))),
                                _ => {}
                            }
                        }
                        0xc0 => {
                            let p = t.byte()?;
                            events.push((track, tick, order, RawEvent::Program(ch, p)));
                        }
                        0xd0 => {
                            t.byte()?;
                        }
                        0xe0 => {
                            let (lo, hi) = (t.byte()?, t.byte()?);
                            events.push((track, tick, order, RawEvent::Bend(ch, (hi as u16) << 7 | lo as u16)));
                        }
                        _ => return Err(CoreError::Format("evento MIDI desconocido".into())),
                    }
                }
                _ => return Err(CoreError::Format("evento MIDI sin estado".into())),
            }
        }
    }

    // Mapa de tempo (de cualquier pista) → segundos.
    let mut tempos: Vec<(u64, u32)> = events.iter().filter_map(|(_, tk, _, e)| if let RawEvent::Tempo(us) = e { Some((*tk, *us)) } else { None }).collect();
    tempos.sort_by_key(|t| t.0);
    let seconds = |tick: u64| -> f64 {
        let (mut sec, mut last, mut us) = (0.0, 0u64, 500_000u32);
        for &(tk, t_us) in &tempos {
            if tk >= tick {
                break;
            }
            sec += (tk - last) as f64 / ppq * us as f64 / 1e6;
            last = tk;
            us = t_us;
        }
        sec + (tick - last) as f64 / ppq * us as f64 / 1e6
    };

    events.sort_by_key(|e| (e.0, e.1, e.2));
    let mut out: Vec<MidiTrackNotes> = Vec::new();
    let mut names: HashMap<usize, String> = HashMap::new();
    let mut program = [0u8; 16];
    let mut range = [2.0f32; 16];
    let mut bend = [0.0f32; 16];
    // Notas abiertas: (pista, canal, tecla) → (índice en `out`, posición de la nota)
    let mut open: HashMap<(usize, u8, u8), (usize, usize)> = HashMap::new();
    let mut slot_of: HashMap<(usize, u8), usize> = HashMap::new();
    for (track, tick, _, ev) in &events {
        let t = seconds(*tick);
        match ev {
            RawEvent::Name(n) => {
                names.entry(*track).or_insert_with(|| n.clone());
            }
            RawEvent::Program(ch, p) => program[*ch as usize] = *p,
            RawEvent::Rpn(ch, semis) => range[*ch as usize] = (*semis).max(1) as f32,
            RawEvent::Tempo(_) => {}
            RawEvent::Bend(ch, v) => {
                let cents = (*v as f32 - 8192.0) / 8192.0 * range[*ch as usize] * 100.0;
                bend[*ch as usize] = cents;
                // Se añade a las notas que suenan en ese canal.
                for (&(tr, c, _), &(slot, idx)) in &open {
                    if tr == *track && c == *ch {
                        let n = &mut out[slot].notes[idx];
                        n.bends.push([(t - n.start) as f32, cents]);
                    }
                }
            }
            RawEvent::NoteOn(ch, k, v) => {
                let slot = *slot_of.entry((*track, *ch)).or_insert_with(|| {
                    out.push(MidiTrackNotes {
                        name: String::new(),
                        channel: *ch,
                        program: program[*ch as usize],
                        drums: *ch == 9,
                        notes: Vec::new(),
                    });
                    out.len() - 1
                });
                // Una nota repetida sin cerrar se cierra aquí.
                if let Some((s, idx)) = open.remove(&(*track, *ch, *k)) {
                    out[s].notes[idx].end = t;
                }
                out[slot].program = program[*ch as usize];
                let mut n = Note::new(t, t, *k, *v);
                if bend[*ch as usize] != 0.0 {
                    n.bends.push([0.0, bend[*ch as usize]]);
                }
                out[slot].notes.push(n);
                open.insert((*track, *ch, *k), (slot, out[slot].notes.len() - 1));
            }
            RawEvent::NoteOff(ch, k) => {
                if let Some((s, idx)) = open.remove(&(*track, *ch, *k)) {
                    out[s].notes[idx].end = t;
                }
            }
        }
    }
    // Notas sin cierre: una negra.
    for ((track, _, _), (s, idx)) in open {
        let _ = track;
        let n = &mut out[s].notes[idx];
        n.end = n.start + 60.0 / 120.0;
    }
    for ((track, ch), slot) in slot_of {
        let base = names.get(&track).cloned().unwrap_or_default();
        let m = &mut out[slot];
        m.name = match (base.is_empty(), ch == 9) {
            (true, true) => "Batería".into(),
            (true, false) => format!("Canal {}", ch + 1),
            (false, _) => base,
        };
        // Un bend al final de la nota sin efecto audible se descarta.
        for n in &mut m.notes {
            let dur = (n.end - n.start) as f32;
            n.bends.retain(|p| p[0] <= dur);
        }
        m.notes = sanitize_notes(std::mem::take(&mut m.notes));
    }
    out.retain(|m| !m.notes.is_empty());
    Ok(out)
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
    /// Batería: cada altura es un instrumento del mapa de percusión General MIDI.
    Drums,
    /// Colchón: ataque y liberación lentos.
    Pad,
    /// Cuerda pulsada: ataque inmediato y caída rápida.
    Pluck,
}

/// Ruido blanco reproducible (xorshift32) en [−1, 1).
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// Un golpe de batería sintético para la altura GM `pitch` (bombo, caja,
/// charles, toms, platos), añadido a `out` desde la muestra `at`.
fn drum_hit(out: &mut [f32], at: usize, pitch: u8, gain: f32, sr: f32, noise: &mut Noise) {
    use std::f32::consts::TAU;
    // (duración s, tono inicial Hz, tono final Hz, mezcla de ruido 0–1, paso alto del ruido)
    let (len, f_hi, f_lo, mix, bright) = match pitch {
        35 | 36 => (0.35, 160.0, 48.0, 0.04, false),
        37 => (0.06, 900.0, 900.0, 0.5, true),
        38 | 40 => (0.2, 220.0, 170.0, 0.65, false),
        39 => (0.15, 1200.0, 1200.0, 0.9, true),
        41 | 43 => (0.35, 110.0, 80.0, 0.1, false),
        45 | 47 => (0.3, 150.0, 115.0, 0.1, false),
        48 | 50 => (0.25, 200.0, 160.0, 0.1, false),
        42 | 44 => (0.05, 0.0, 0.0, 1.0, true),
        46 => (0.3, 0.0, 0.0, 1.0, true),
        49 | 52 | 55 | 57 => (1.2, 0.0, 0.0, 1.0, true),
        51 | 53 | 59 => (0.6, 3200.0, 3200.0, 0.85, true),
        _ => (0.12, 600.0, 600.0, 0.6, true),
    };
    let n = (len * sr) as usize;
    let mut phase = 0.0f32;
    let mut prev = 0.0f32;
    for k in 0..n {
        let i = at + k;
        if i >= out.len() {
            break;
        }
        let t = k as f32 / sr;
        let env = (-t * 5.0 / len).exp() * (k as f32 / (0.001 * sr)).min(1.0);
        let f = f_lo + (f_hi - f_lo) * (-t * 30.0).exp();
        phase += f / sr;
        let tone = if f > 0.0 { (TAU * phase).sin() } else { 0.0 };
        let w = noise.next();
        let nz = if bright {
            let hp = w - prev;
            prev = w;
            hp * 0.7
        } else {
            w
        };
        out[i] += gain * env * ((1.0 - mix) * tone + mix * nz);
    }
}

/// Sintetiza las notas en estéreo a `rate`, con la duración `length` (s) o la
/// de la última nota. Normaliza a −1 dBFS si satura.
pub fn synthesize(notes: &[Note], rate: u32, length: Option<f64>, timbre: Timbre) -> AudioBuffer {
    let rel = if timbre == Timbre::Pad { 0.5 } else { 0.08 };
    let end = length.unwrap_or_else(|| notes.iter().map(|n| n.end).fold(0.0, f64::max) + rel);
    let frames = (end.max(0.0) * rate as f64).ceil() as usize;
    let mut mono = vec![0.0f32; frames];
    // Las mismas tablas que el sintetizador en vivo (`livesynth`).
    let harmonics = crate::livesynth::harmonics(timbre);
    let sr = rate as f32;
    if timbre == Timbre::Drums {
        let mut noise = Noise(0x9e37_79b9);
        for n in notes {
            let gain = 0.5 * (n.velocity as f32 / 127.0).powf(1.5);
            drum_hit(&mut mono, (n.start * rate as f64) as usize, n.pitch, gain, sr, &mut noise);
        }
    }
    for n in notes.iter().filter(|_| timbre != Timbre::Drums) {
        let f0 = 440.0 * 2f32.powf((n.pitch as f32 - 69.0) / 12.0);
        let mut phase = 0.0f64;
        let mut ratio = 1.0f32;
        let amp = 0.18 * (n.velocity as f32 / 127.0).powf(1.5);
        let a = (n.start * rate as f64) as usize;
        let hold = ((n.end - n.start) * rate as f64) as usize;
        let total = hold + (rel * rate as f64) as usize;
        let attack = ((if timbre == Timbre::Pad { 0.15 } else { 0.005 }) * sr) as usize;
        for k in 0..total {
            let i = a + k;
            if i >= frames {
                break;
            }
            let t = k as f32 / sr;
            let mut env = if k < attack { k as f32 / attack as f32 } else { 1.0 };
            env *= crate::livesynth::decay(timbre, t);
            if k >= hold {
                env *= 1.0 - (k - hold) as f32 / (total - hold).max(1) as f32;
            }
            // La curva de tono se evalúa cada 32 muestras; la fase se acumula
            // para que los cambios de frecuencia no produzcan saltos.
            if !n.bends.is_empty() && k % 32 == 0 {
                ratio = 2f32.powf(n.bend_at(t as f64) / 1200.0);
            }
            phase += (f0 * ratio / sr) as f64;
            let ph = (phase.fract() as f32) * std::f32::consts::TAU;
            let mut s = 0.0;
            for (h, &w) in harmonics.iter().enumerate() {
                let f = f0 * ratio * (h + 1) as f32;
                if f >= sr * 0.45 {
                    break;
                }
                s += w * (ph * (h + 1) as f32).sin();
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
        Note::new(start, end, pitch, 100)
    }

    /// El JSON tal como lo escribe el proceso de IA (con campos que aquí no se usan).
    #[test]
    fn reads_worker_output() {
        let dir = std::env::temp_dir().join(format!("mn_tx_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("analysis")).unwrap();
        let notes = dir.join("notes.json");
        std::fs::write(&notes, r#"{"version":1,"source":"basic_pitch","mode":"mono","bends":"crepe","notes":[{"start":0.5,"end":1.0,"pitch":60,"velocity":98,"bends":[[0.0,12.5],[0.25,-30.0]]}],"seconds":0.2}"#).unwrap();
        let r = NotesResult::load(&notes).unwrap();
        assert_eq!((r.mode.as_str(), r.notes[0].pitch, r.notes[0].velocity), ("mono", 60, 98));
        assert_eq!(r.notes[0].bends, vec![[0.0, 12.5], [0.25, -30.0]]);
        let drums = dir.join("drums.json");
        std::fs::write(&drums, r#"{"version":1,"source":"nmf_drums","mode":"drums","notes":[{"start":0.1,"end":0.2,"pitch":36,"velocity":100}],"seconds":0.1}"#).unwrap();
        assert_eq!(NotesResult::load(&drums).unwrap().mode, DRUMS_MODE);

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
            Note::new(f64::NAN, 1.0, 60, 1),
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
                    s if s & 0xf0 == 0xc0 || s & 0xf0 == 0xd0 => 2,
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
            MidiPart { name: "Piano", program: 0, notes: &notes, drums: false },
            MidiPart { name: "Bajo", program: 33, notes: &bass, drums: false },
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

    fn bent(start: f64, end: f64, pitch: u8, bends: &[[f32; 2]]) -> Note {
        Note { bends: bends.to_vec(), ..note(start, end, pitch) }
    }

    /// Valor de 14 bits de un evento de pitch bend.
    fn bend_of(e: &[u8]) -> u16 {
        e[1] as u16 | (e[2] as u16) << 7
    }

    #[test]
    fn bend_curve_interpolates_and_sanitizes() {
        let n = bent(1.0, 2.0, 60, &[[0.0, 0.0], [0.5, 100.0], [1.0, -50.0]]);
        assert_eq!(n.bend_at(-1.0), 0.0);
        assert!((n.bend_at(0.25) - 50.0).abs() < 1e-4);
        assert!((n.bend_at(0.75) - 25.0).abs() < 1e-4);
        assert_eq!(n.bend_at(5.0), -50.0);
        let s = sanitize_notes(vec![
            bent(0.0, 1.0, 60, &[[0.5, 900.0], [f32::NAN, 0.0], [-1.0, 10.0]]),
            bent(1.0, 2.0, 62, &[[0.0, 0.2]]),
        ]);
        assert_eq!(s[0].bends, vec![[0.0, 10.0], [0.5, BEND_LIMIT]]);
        assert!(s[1].bends.is_empty(), "una curva que no se mueve se descarta");
        // Sin curva no se escribe el campo en el JSON (compatible con lo anterior).
        assert!(!serde_json::to_string(&s[1]).unwrap().contains("bends"));
    }

    #[test]
    fn writes_pitch_bends() {
        // Una subida lineal de 0 a +100 cents en medio segundo.
        let notes = [bent(0.0, 0.5, 60, &[[0.0, 0.0], [0.5, 100.0]]), note(0.5, 1.0, 62)];
        let parts = [MidiPart { name: "Voz", program: 53, notes: &notes, drums: false }];
        let (_, tracks) = parse(&write_midi(&parts, &TempoMap::constant(120.0), 4, "x"));
        let t = &tracks[1];
        // RPN 0 = 2 semitonos antes de cualquier nota.
        let rpn: Vec<_> = t.iter().filter(|(_, e)| e[0] == 0xb0).map(|(_, e)| (e[1], e[2])).collect();
        assert_eq!(rpn, vec![(101, 0), (100, 0), (6, 2), (38, 0), (101, 127), (100, 127)]);
        let bends: Vec<_> = t.iter().filter(|(_, e)| e[0] == 0xe0).collect();
        // Empieza centrado antes del «note on», sube y vuelve al centro al acabar la nota.
        assert_eq!(bend_of(&bends[0].1), 8192);
        let on = t.iter().position(|(_, e)| e[0] == 0x90).unwrap();
        assert!(t.iter().position(|(_, e)| e[0] == 0xe0).unwrap() < on);
        let peak = bends.iter().map(|(_, e)| bend_of(e)).max().unwrap();
        assert!((peak as i32 - (8192 + 8191 / 2)).abs() < 200, "{peak}");
        let reset = bends.last().unwrap();
        assert_eq!((reset.0, bend_of(&reset.1)), (480, 8192));
        // La vuelta al centro va antes del «note on» siguiente en el mismo pulso.
        let next_on = t.iter().position(|(tk, e)| *tk == 480 && e[0] == 0x90).unwrap();
        let reset_pos = t.iter().rposition(|(_, e)| e[0] == 0xe0).unwrap();
        assert!(reset_pos < next_on);
        assert!(bends.windows(2).all(|w| w[0].0 <= w[1].0));
    }

    #[test]
    fn overlapping_bent_notes_get_their_own_channels() {
        // Acorde de tres notas: dos con curva distinta y una fija.
        let notes = [
            bent(0.0, 1.0, 60, &[[0.0, 0.0], [1.0, 50.0]]),
            bent(0.0, 1.0, 64, &[[0.0, 0.0], [1.0, -50.0]]),
            note(0.0, 1.0, 67),
        ];
        let drums = [note(0.0, 0.1, 36), note(0.5, 0.6, 38)];
        let parts = [
            MidiPart { name: "Guitarra", program: 25, notes: &notes, drums: false },
            MidiPart { name: "Batería", program: 0, notes: &drums, drums: true },
        ];
        let (_, tracks) = parse(&write_midi(&parts, &TempoMap::constant(120.0), 4, "x"));
        let ch_of =
            |pitch: u8| tracks[1].iter().find(|(_, e)| e[0] & 0xf0 == 0x90 && e[1] == pitch).unwrap().1[0] & 0x0f;
        let (a, b, c) = (ch_of(60), ch_of(64), ch_of(67));
        assert!(a != b && a != c && b != c, "{a} {b} {c}");
        assert!(![a, b, c].contains(&DRUM_CHANNEL));
        // Cada canal usado lleva el programa de la parte.
        for ch in [a, b, c] {
            assert!(tracks[1].iter().any(|(_, e)| e == &vec![0xc0 | ch, 25]));
        }
        // La batería, en el canal 10 sin cambio de programa.
        assert!(tracks[2].iter().filter(|(_, e)| e[0] & 0xf0 == 0x90).all(|(_, e)| e[0] == 0x99));
        assert!(!tracks[2].iter().any(|(_, e)| e[0] & 0xf0 == 0xc0));
    }

    #[test]
    fn synthesizes_bends_and_drums() {
        let rate = 48_000;
        // Nota de La4 que salta un semitono hacia arriba a mitad.
        let n = bent(0.0, 1.0, 69, &[[0.0, 0.0], [0.5, 0.0], [0.501, 100.0], [1.0, 100.0]]);
        let buf = synthesize(&[n], rate, Some(1.2), Timbre::Bass);
        let left: Vec<f32> = buf.data.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
        let hz = |a: f64, b: f64| {
            let seg = &left[(a * rate as f64) as usize..(b * rate as f64) as usize];
            seg.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f64 / (b - a)
        };
        assert!((hz(0.1, 0.45) - 440.0).abs() < 8.0);
        assert!((hz(0.6, 0.95) - 466.16).abs() < 8.0, "{}", hz(0.6, 0.95));

        let drums = synthesize(&[note(0.1, 0.2, 36), note(0.5, 0.6, 42)], rate, Some(1.0), Timbre::Drums);
        let l: Vec<f32> = drums.data.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
        let rms = |a: f64, b: f64| {
            let s = &l[(a * rate as f64) as usize..(b * rate as f64) as usize];
            (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
        };
        assert_eq!(rms(0.0, 0.09), 0.0);
        assert!(rms(0.1, 0.15) > 0.05 && rms(0.5, 0.53) > 0.01);
        // El bombo es grave y el charles agudo: cruces por cero muy distintos.
        let zc = |a: f64, b: f64| {
            l[(a * rate as f64) as usize..(b * rate as f64) as usize]
                .windows(2)
                .filter(|w| w[0].signum() != w[1].signum())
                .count()
        };
        assert!(zc(0.5, 0.53) > 10 * zc(0.1, 0.13), "{} {}", zc(0.5, 0.53), zc(0.1, 0.13));
    }

    #[test]
    fn synthesizes_the_right_pitch() {
        let rate = 48_000;
        let buf = synthesize(&[note(0.1, 0.6, 69)], rate, Some(1.0), Timbre::Bass);
        assert_eq!(buf.frames(), rate as usize);
        let left: Vec<f32> = buf.data.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
        assert!(left[..rate as usize / 20].iter().all(|&s| s == 0.0), "silencio antes de la nota");
        // Frecuencia fundamental por cruces por cero ascendentes en el tramo sostenido.
        let seg = &left[(0.2 * rate as f64) as usize..(0.5 * rate as f64) as usize];
        let ups = seg.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        let hz = ups as f64 / 0.3;
        assert!((hz - 440.0).abs() < 8.0, "{hz}");
        assert!(left[(0.8 * rate as f64) as usize..].iter().all(|&s| s == 0.0), "silencio tras la liberación");
    }

    #[test]
    fn reads_back_written_midi() {
        // Tempo variable, curvas de tono, acorde y batería.
        let beats: Vec<f64> = (0..40).map(|k| 0.5 * k as f64 + 0.01 * (k * k) as f64 / 40.0).collect();
        let tempo = TempoMap::from_beats(&beats, Some(0), 4);
        let keys = [note(0.0, 0.5, 60), note(0.5, 1.5, 64), note(0.5, 1.5, 67), bent(2.0, 3.0, 62, &[[0.0, 0.0], [0.5, 100.0], [1.0, 100.0]])];
        let drums = [note(0.0, 0.1, 36), note(0.5, 0.6, 38)];
        let parts = [
            MidiPart { name: "Teclado", program: 4, notes: &keys, drums: false },
            MidiPart { name: "Batería", program: 0, notes: &drums, drums: true },
        ];
        let bytes = write_midi(&parts, &tempo, 4, "Prueba");
        let tracks = read_midi(&bytes).unwrap();
        assert_eq!(tracks.len(), 2, "{:?}", tracks.iter().map(|t| &t.name).collect::<Vec<_>>());
        let k = tracks.iter().find(|t| t.name == "Teclado").unwrap();
        assert_eq!(k.program, 4);
        assert!(!k.drums);
        assert_eq!(k.notes.len(), 4);
        for (a, b) in k.notes.iter().zip(keys.iter()) {
            assert_eq!(a.pitch, b.pitch);
            // El escritor omite cambios de tempo de menos del 0,5 %: unos ms de error.
            assert!((a.start - b.start).abs() < 0.015 && (a.end - b.end).abs() < 0.015, "{a:?} {b:?}");
        }
        // La curva vuelve con su forma: sube un semitono a mitad de la nota.
        let bn = &k.notes[3];
        assert!(bn.bend_at(0.0).abs() < 5.0 && (bn.bend_at(0.75) - 100.0).abs() < 5.0, "{:?}", bn.bends);
        let d = tracks.iter().find(|t| t.drums).unwrap();
        assert_eq!(d.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![36, 38]);
        assert!(read_midi(b"RIFF....").is_err());
    }
}
