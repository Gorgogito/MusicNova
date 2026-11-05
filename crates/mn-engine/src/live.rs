//! Instrumentos en vivo: eventos MIDI que llegan al hilo de audio, el reloj con
//! el que se sellan y lo que se graba de ellos.
//!
//! Cada evento lleva el instante (QPC en ns, el mismo reloj que usa cpal para
//! decir cuándo suena cada bloque) en que se pulsó la tecla. Con él, el hilo de
//! audio calcula qué posición de la canción se oía en ese momento, y la nota
//! grabada queda donde el músico la tocó, compensando la latencia de salida.

use std::sync::{Arc, Mutex};

/// Mensaje MIDI (de cualquier canal) que entiende el motor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiMessage {
    NoteOn { key: u8, velocity: u8 },
    NoteOff { key: u8 },
    /// Pedal de sostenido (CC 64).
    Sustain(bool),
    /// Soltar todo (CC 120/123).
    AllNotesOff,
    /// Rueda de tono, 0–16383 (8192 = centro).
    PitchBend(u16),
    /// Otro controlador (CC), 0–127.
    Control { cc: u8, value: u8 },
    /// Aftertouch de canal, 0–127.
    Pressure(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiEvent {
    pub msg: MidiMessage,
    /// Instante del evento en ns del reloj de [`now_ns`]; 0 = «ahora».
    pub time_ns: u64,
}

/// Nota tocada durante la grabación: posición de la línea de tiempo (frames
/// del motor) en la que se oía la canción al pulsar o soltar la tecla.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecEvent {
    pub frame: u64,
    pub key: u8,
    pub velocity: u8,
    pub on: bool,
}

/// Por dónde llegan los eventos al motor (teclado MIDI, teclado del ordenador).
/// Se puede clonar y usar desde cualquier hilo.
#[derive(Clone)]
pub struct MidiSink {
    pub(crate) tx: Arc<Mutex<rtrb::Producer<MidiEvent>>>,
}

impl MidiSink {
    /// Envía un evento; `false` si la cola está llena.
    pub fn send(&self, ev: MidiEvent) -> bool {
        self.tx.lock().map(|mut t| t.push(ev).is_ok()).unwrap_or(false)
    }
}

/// Instante actual en ns del reloj de rendimiento de Windows (el de cpal en WASAPI).
#[cfg(windows)]
pub fn now_ns() -> u64 {
    use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut c, mut f) = (0i64, 0i64);
    unsafe {
        QueryPerformanceCounter(&mut c);
        QueryPerformanceFrequency(&mut f);
    }
    if f <= 0 {
        return 0;
    }
    (c as i128 * 1_000_000_000 / f as i128) as u64
}

#[cfg(not(windows))]
pub fn now_ns() -> u64 {
    0
}

/// Empareja pulsaciones y liberaciones en notas (inicio s, fin s, tecla,
/// velocidad). Las que siguen pulsadas terminan en `end_frame`.
pub fn pair_notes(events: &[RecEvent], rate: u32, end_frame: u64) -> Vec<(f64, f64, u8, u8)> {
    let secs = |f: u64| f as f64 / rate as f64;
    let mut open: Vec<(u8, u64, u8)> = Vec::new();
    let mut out = Vec::new();
    let close = |start: u64, end: u64, key: u8, vel: u8, out: &mut Vec<(f64, f64, u8, u8)>| {
        // Al menos 10 ms, para que una nota muy corta no desaparezca.
        let end = end.max(start + rate as u64 / 100);
        out.push((secs(start), secs(end), key, vel.clamp(1, 127)));
    };
    for e in events {
        if e.on {
            open.push((e.key, e.frame, e.velocity));
        } else if let Some(i) = open.iter().position(|o| o.0 == e.key) {
            let (key, start, vel) = open.remove(i);
            close(start, e.frame.max(start), key, vel, &mut out);
        }
    }
    for (key, start, vel) in open {
        close(start, end_frame.max(start), key, vel, &mut out);
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_presses_and_releases() {
        let ev = |frame, key, on| RecEvent { frame, key, velocity: if on { 100 } else { 0 }, on };
        let notes = pair_notes(
            &[ev(100, 60, true), ev(200, 64, true), ev(300, 60, false), ev(350, 60, true), ev(400, 60, false), ev(500, 67, true)],
            1000,
            900,
        );
        assert_eq!(notes, vec![(0.1, 0.3, 60, 100), (0.2, 0.9, 64, 100), (0.35, 0.4, 60, 100), (0.5, 0.9, 67, 100)]);
        // Liberación sin pulsación: se ignora; nota de duración nula: 10 ms.
        assert_eq!(pair_notes(&[ev(5, 1, false), ev(10, 2, true), ev(10, 2, false)], 1000, 50), vec![(0.01, 0.02, 2, 100)]);
    }

    #[cfg(windows)]
    #[test]
    fn clock_advances() {
        let a = now_ns();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = now_ns();
        assert!(b > a && b - a >= 4_000_000, "{a} {b}");
    }
}
