//! Entrada de teclados MIDI con la API multimedia de Windows (winmm), sin
//! bibliotecas externas. Cada mensaje se traduce a [`MidiEvent`] (omni: todos
//! los canales), se sella con [`now_ns`] y se envía al motor.

use std::ptr;

use windows_sys::Win32::Media::Audio::{
    midiInClose, midiInGetDevCapsW, midiInGetNumDevs, midiInOpen, midiInReset, midiInStart, midiInStop, CALLBACK_FUNCTION,
    HMIDIIN, MIDIINCAPSW,
};

use crate::live::{now_ns, MidiEvent, MidiMessage, MidiSink};

const MIM_DATA: u32 = 0x3C3;

/// Nombres de los dispositivos de entrada MIDI (el índice es el que pide [`MidiInput::open`]).
pub fn midi_inputs() -> Vec<String> {
    let n = unsafe { midiInGetNumDevs() };
    (0..n)
        .map(|i| {
            let mut caps: MIDIINCAPSW = unsafe { std::mem::zeroed() };
            let r = unsafe { midiInGetDevCapsW(i as usize, &mut caps, std::mem::size_of::<MIDIINCAPSW>() as u32) };
            if r != 0 {
                return format!("Entrada MIDI {}", i + 1);
            }
            // Estructura empaquetada: se copia el campo antes de leerlo.
            let pname = caps.szPname;
            let len = pname.iter().position(|&c| c == 0).unwrap_or(pname.len());
            String::from_utf16_lossy(&pname[..len])
        })
        .collect()
}

/// Traduce un mensaje corto MIDI (los tres bytes empaquetados de winmm).
pub fn parse_short(msg: u32) -> Option<MidiMessage> {
    let status = (msg & 0xF0) as u8;
    let d1 = ((msg >> 8) & 0x7F) as u8;
    let d2 = ((msg >> 16) & 0x7F) as u8;
    match status {
        0x90 if d2 > 0 => Some(MidiMessage::NoteOn { key: d1, velocity: d2 }),
        0x90 | 0x80 => Some(MidiMessage::NoteOff { key: d1 }),
        0xB0 if d1 == 64 => Some(MidiMessage::Sustain(d2 >= 64)),
        0xB0 if d1 == 120 || d1 == 123 => Some(MidiMessage::AllNotesOff),
        _ => None,
    }
}

unsafe extern "system" fn callback(_h: HMIDIIN, msg: u32, instance: usize, p1: usize, _p2: usize) {
    if msg != MIM_DATA || instance == 0 {
        return;
    }
    let sink = &*(instance as *const MidiSink);
    if let Some(m) = parse_short(p1 as u32) {
        sink.send(MidiEvent { msg: m, time_ns: now_ns() });
    }
}

/// Dispositivo MIDI abierto; al soltarse se cierra.
pub struct MidiInput {
    handle: isize,
    sink: *mut MidiSink,
    pub name: String,
}

unsafe impl Send for MidiInput {}
unsafe impl Sync for MidiInput {}

impl MidiInput {
    pub fn open(index: usize, sink: MidiSink) -> Result<Self, String> {
        let names = midi_inputs();
        let name = names.get(index).cloned().ok_or("ese dispositivo MIDI ya no está conectado")?;
        let sink = Box::into_raw(Box::new(sink));
        let mut h: HMIDIIN = ptr::null_mut();
        let r = unsafe { midiInOpen(&mut h, index as u32, callback as *const () as usize, sink as usize, CALLBACK_FUNCTION) };
        if r != 0 {
            drop(unsafe { Box::from_raw(sink) });
            return Err(format!("no se pudo abrir «{name}» (¿lo usa otro programa?), código {r}"));
        }
        unsafe { midiInStart(h) };
        Ok(Self { handle: h as isize, sink, name })
    }
}

impl Drop for MidiInput {
    fn drop(&mut self) {
        unsafe {
            let h = self.handle as HMIDIIN;
            midiInStop(h);
            midiInReset(h);
            midiInClose(h);
            // Tras cerrar ya no llegan llamadas: se puede liberar el sumidero.
            drop(Box::from_raw(self.sink));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_notes_pedal_and_panic() {
        assert_eq!(parse_short(0x00_64_3C_90), Some(MidiMessage::NoteOn { key: 60, velocity: 100 }));
        assert_eq!(parse_short(0x00_00_3C_93), Some(MidiMessage::NoteOff { key: 60 }), "velocidad 0 = soltar (canal 4)");
        assert_eq!(parse_short(0x00_40_3C_80), Some(MidiMessage::NoteOff { key: 60 }));
        assert_eq!(parse_short(0x00_7F_40_B0), Some(MidiMessage::Sustain(true)));
        assert_eq!(parse_short(0x00_00_40_B0), Some(MidiMessage::Sustain(false)));
        assert_eq!(parse_short(0x00_00_7B_B0), Some(MidiMessage::AllNotesOff));
        assert_eq!(parse_short(0x00_00_10_E0), None, "pitch bend: aún no");
        // Enumerar no falla aunque no haya teclados conectados.
        let _ = midi_inputs();
    }
}
