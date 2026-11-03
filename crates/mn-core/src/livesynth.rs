//! Sintetizador integrado en tiempo real (teclado MIDI).
//!
//! Usa las mismas fórmulas que [`synthesize`](crate::transcription::synthesize)
//! (armónicos, ataque, caída por timbre y liberación lineal), así que lo que se
//! toca suena igual que la pista generada después con las notas grabadas.
//! Las voces se reservan al crearlo: `render` no reserva memoria.

use mn_fx::{Instrument, LiveNote};

use crate::transcription::Timbre;

/// Voces simultáneas; al agotarse se reutiliza la más antigua.
const VOICES: usize = 32;

#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    key: u8,
    amp: f32,
    f0: f32,
    phase: f64,
    /// Muestras desde que empezó.
    k: usize,
    /// Muestra en la que se soltó (liberación), si ya se soltó.
    released: Option<usize>,
    /// Orden de llegada (para robar la voz más antigua).
    serial: u64,
    // Batería: parámetros del golpe.
    drum: Option<Drum>,
    prev: f32,
}

#[derive(Clone, Copy)]
struct Drum {
    len: f32,
    f_hi: f32,
    f_lo: f32,
    mix: f32,
    bright: bool,
}

pub struct LiveSynth {
    timbre: Timbre,
    sr: f32,
    harmonics: &'static [f32],
    rel_frames: usize,
    attack: usize,
    voices: [Voice; VOICES],
    serial: u64,
    noise: u32,
}

/// Armónicos de cada timbre (los mismos que el render offline).
pub(crate) fn harmonics(timbre: Timbre) -> &'static [f32] {
    match timbre {
        Timbre::Keys => &[1.0, 0.45, 0.25, 0.12, 0.06],
        Timbre::Bass => &[1.0, 0.35, 0.08],
        Timbre::Lead => &[1.0, 0.5, 0.33, 0.25, 0.2, 0.16],
        Timbre::Drums => &[],
        Timbre::Pad => &[1.0, 0.3, 0.2, 0.08, 0.05],
        Timbre::Pluck => &[1.0, 0.6, 0.4, 0.3, 0.2, 0.12, 0.08],
    }
}

/// Caída de la envolvente en `t` segundos desde el ataque.
pub(crate) fn decay(timbre: Timbre, t: f32) -> f32 {
    match timbre {
        Timbre::Keys => (-t * 2.2).exp(),
        Timbre::Bass => (-t * 1.2).exp(),
        Timbre::Lead => 0.85 + 0.15 * (-t * 6.0).exp(),
        Timbre::Drums => 0.0,
        Timbre::Pad => 1.0,
        Timbre::Pluck => (-t * 7.0).exp(),
    }
}

fn drum_params(pitch: u8) -> Drum {
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
    Drum { len, f_hi, f_lo, mix, bright }
}

impl LiveSynth {
    pub fn new(timbre: Timbre, rate: u32) -> Self {
        let sr = rate as f32;
        let rel = if timbre == Timbre::Pad { 0.5 } else { 0.08 };
        let attack = ((if timbre == Timbre::Pad { 0.15 } else { 0.005 }) * sr) as usize;
        Self {
            timbre,
            sr,
            harmonics: harmonics(timbre),
            rel_frames: (rel * rate as f64) as usize,
            attack,
            voices: [Voice::default(); VOICES],
            serial: 0,
            noise: 0x9e37_79b9,
        }
    }

    fn note_on(&mut self, key: u8, velocity: u8) {
        let v = velocity.clamp(1, 127) as f32 / 127.0;
        let slot = match self.voices.iter().position(|v| !v.active) {
            Some(i) => i,
            None => (0..VOICES).min_by_key(|&i| self.voices[i].serial).unwrap_or(0),
        };
        self.serial += 1;
        let drums = self.timbre == Timbre::Drums;
        self.voices[slot] = Voice {
            active: true,
            key,
            amp: if drums { 0.5 * v.powf(1.5) } else { 0.18 * v.powf(1.5) },
            f0: 440.0 * 2f32.powf((key as f32 - 69.0) / 12.0),
            phase: 0.0,
            k: 0,
            released: None,
            serial: self.serial,
            drum: drums.then(|| drum_params(key)),
            prev: 0.0,
        };
    }

    fn note_off(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.active && v.key == key && v.released.is_none() && v.drum.is_none()) {
            v.released = Some(v.k);
        }
    }

    fn next_noise(&mut self) -> f32 {
        let mut x = self.noise;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Una muestra de la voz `i` (y avanza su estado).
    fn voice_sample(&mut self, i: usize) -> f32 {
        let sr = self.sr;
        if let Some(d) = self.voices[i].drum {
            let v = self.voices[i];
            let n = (d.len * sr) as usize;
            if v.k >= n {
                self.voices[i].active = false;
                return 0.0;
            }
            let t = v.k as f32 / sr;
            let env = (-t * 5.0 / d.len).exp() * (v.k as f32 / (0.001 * sr)).min(1.0);
            let f = d.f_lo + (d.f_hi - d.f_lo) * (-t * 30.0).exp();
            // La fase del golpe se acumula en f32, como en el render offline.
            let phase = (v.phase as f32) + f / sr;
            let tone = if f > 0.0 { (std::f32::consts::TAU * phase).sin() } else { 0.0 };
            let w = self.next_noise();
            let voice = &mut self.voices[i];
            let nz = if d.bright {
                let hp = w - voice.prev;
                voice.prev = w;
                hp * 0.7
            } else {
                w
            };
            voice.phase = phase as f64;
            voice.k += 1;
            return voice.amp * env * ((1.0 - d.mix) * tone + d.mix * nz);
        }
        let (timbre, attack, rel) = (self.timbre, self.attack, self.rel_frames);
        let harmonics = self.harmonics;
        let v = &mut self.voices[i];
        if let Some(r) = v.released {
            if v.k >= r + rel {
                v.active = false;
                return 0.0;
            }
        }
        let k = v.k;
        let t = k as f32 / sr;
        let mut env = if k < attack { k as f32 / attack as f32 } else { 1.0 };
        env *= decay(timbre, t);
        if let Some(r) = v.released {
            env *= 1.0 - (k - r) as f32 / rel.max(1) as f32;
        }
        v.phase += (v.f0 / sr) as f64;
        let ph = (v.phase.fract() as f32) * std::f32::consts::TAU;
        let mut s = 0.0;
        for (h, &w) in harmonics.iter().enumerate() {
            if v.f0 * (h + 1) as f32 >= sr * 0.45 {
                break;
            }
            s += w * (ph * (h + 1) as f32).sin();
        }
        v.k += 1;
        v.amp * env * s
    }
}

impl Instrument for LiveSynth {
    fn render(&mut self, notes: &[LiveNote], buf: &mut [f32]) {
        let frames = buf.len() / 2;
        let mut next = 0;
        for f in 0..frames {
            while next < notes.len() && notes[next].time as usize <= f {
                let n = notes[next];
                if n.on && n.velocity > 0 {
                    self.note_on(n.key, n.velocity);
                } else {
                    self.note_off(n.key);
                }
                next += 1;
            }
            let mut s = 0.0;
            for i in 0..VOICES {
                if self.voices[i].active {
                    s += self.voice_sample(i);
                }
            }
            buf[f * 2] = s;
            buf[f * 2 + 1] = s;
        }
        // Notas fuera del bloque (no debería ocurrir): se aplican al final.
        for n in &notes[next..] {
            if n.on && n.velocity > 0 {
                self.note_on(n.key, n.velocity);
            } else {
                self.note_off(n.key);
            }
        }
    }

    fn all_notes_off(&mut self) {
        for v in self.voices.iter_mut().filter(|v| v.active && v.released.is_none()) {
            v.released = Some(v.k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::{synthesize, Note};

    /// Toca `notes` en bloques de `block` frames como lo haría el motor.
    fn play(timbre: Timbre, notes: &[Note], rate: u32, frames: usize, block: usize) -> Vec<f32> {
        let mut events: Vec<(usize, LiveNote)> = Vec::new();
        for n in notes {
            let a = (n.start * rate as f64) as usize;
            let hold = ((n.end - n.start) * rate as f64) as usize;
            events.push((a, LiveNote { time: 0, key: n.pitch, velocity: n.velocity, on: true }));
            events.push((a + hold, LiveNote { time: 0, key: n.pitch, velocity: 0, on: false }));
        }
        events.sort_by_key(|e| e.0);
        let mut synth = LiveSynth::new(timbre, rate);
        let mut out = vec![0.0f32; frames * 2];
        let mut list = Vec::new();
        let mut e = 0;
        for start in (0..frames).step_by(block) {
            let n = block.min(frames - start);
            list.clear();
            while e < events.len() && events[e].0 < start + n {
                list.push(LiveNote { time: (events[e].0 - start) as u32, ..events[e].1 });
                e += 1;
            }
            synth.render(&list, &mut out[start * 2..(start + n) * 2]);
        }
        out
    }

    #[test]
    fn sounds_like_the_offline_render() {
        let rate = 48_000;
        // Notas que se solapan; velocidad moderada para que el render offline no normalice.
        let notes = [Note::new(0.1, 0.5, 60, 80), Note::new(0.3, 0.9, 67, 70), Note::new(0.95, 1.2, 72, 90)];
        for timbre in [Timbre::Keys, Timbre::Bass, Timbre::Lead, Timbre::Pad, Timbre::Pluck] {
            let offline = synthesize(&notes, rate, Some(2.0), timbre).data;
            let live = play(timbre, &notes, rate, offline.len() / 2, 480);
            let err = offline.iter().zip(&live).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            let peak = offline.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.05, "{timbre:?}: hay sonido");
            assert!(err < 1e-4, "{timbre:?}: diferencia máxima {err}");
        }
    }

    #[test]
    fn drums_hit_and_voices_are_reused() {
        let rate = 48_000;
        let notes = [Note::new(0.0, 0.1, 36, 120), Note::new(0.25, 0.3, 38, 120)];
        let live = play(Timbre::Drums, &notes, rate, rate as usize / 2, 512);
        let energy = |a: f64, b: f64| live[(a * rate as f64) as usize * 2..(b * rate as f64) as usize * 2].iter().map(|s| s * s).sum::<f32>();
        assert!(energy(0.0, 0.05) > 1.0, "bombo");
        assert!(energy(0.25, 0.3) > 1.0, "caja");

        // Más notas que voces: nunca se cuelga ni se pasa del búfer.
        let mut synth = LiveSynth::new(Timbre::Pad, rate);
        let many: Vec<LiveNote> = (0..60).map(|k| LiveNote { time: k, key: 30 + k as u8, velocity: 100, on: true }).collect();
        let mut buf = vec![0.0f32; 1024];
        synth.render(&many, &mut buf);
        synth.all_notes_off();
        let mut tail = vec![0.0f32; 2 * rate as usize];
        synth.render(&[], &mut tail);
        assert!(tail[tail.len() - 200..].iter().all(|&s| s == 0.0), "todo se apaga tras la liberación");
    }
}
