//! Reglas de mezcla compartidas por el motor en tiempo real y la exportación.

use crate::audio::AudioBuffer;

/// Ganancia lineal a partir de dB; -60 dB o menos se considera silencio.
pub fn db_to_gain(db: f32) -> f32 {
    if db <= -60.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Ley de balance: el centro mantiene ganancia unitaria en ambos canales.
pub fn balance_gains(gain: f32, pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    (gain * (1.0 - pan).min(1.0), gain * (1.0 + pan).min(1.0))
}

/// Una pista para la mezcla offline.
pub struct MixInput<'a> {
    pub data: &'a [f32],
    pub gain_db: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}

/// Mezcla las pistas exactamente como suenan en el motor (silencio, solo, volumen y panorama).
pub fn mixdown(inputs: &[MixInput], sample_rate: u32) -> AudioBuffer {
    let len = inputs.iter().map(|t| t.data.len()).max().unwrap_or(0);
    let any_solo = inputs.iter().any(|t| t.solo);
    let mut data = vec![0.0f32; len];
    for t in inputs {
        if t.mute || (any_solo && !t.solo) {
            continue;
        }
        let (gl, gr) = balance_gains(db_to_gain(t.gain_db), t.pan);
        for (out, src) in data.chunks_exact_mut(2).zip(t.data.chunks_exact(2)) {
            out[0] += src[0] * gl;
            out[1] += src[1] * gr;
        }
    }
    AudioBuffer { sample_rate, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixdown_respects_solo_and_pan() {
        let a = vec![0.5f32; 8];
        let b = vec![0.25f32; 4];
        let inputs = [
            MixInput { data: &a, gain_db: 0.0, pan: 1.0, mute: false, solo: false },
            MixInput { data: &b, gain_db: 0.0, pan: 0.0, mute: false, solo: false },
        ];
        let m = mixdown(&inputs, 10);
        assert_eq!(m.data, vec![0.25, 0.75, 0.25, 0.75, 0.0, 0.5, 0.0, 0.5]);

        let inputs = [
            MixInput { data: &a, gain_db: 0.0, pan: 0.0, mute: false, solo: false },
            MixInput { data: &b, gain_db: -60.0, pan: 0.0, mute: false, solo: true },
        ];
        assert!(mixdown(&inputs, 10).data.iter().all(|&s| s == 0.0));
    }
}
