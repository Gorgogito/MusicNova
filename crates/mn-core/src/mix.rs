//! Reglas de mezcla compartidas por el motor en tiempo real y la exportación.

use mn_fx::{Chain, EffectSlot};

use crate::audio::AudioBuffer;
use crate::project::MasterSettings;

/// Frames por bloque al procesar efectos offline (igual que el motor).
const BLOCK: usize = 512;

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
    pub effects: &'a [EffectSlot],
}

/// Mezcla las pistas exactamente como suenan en el motor: silencio, solo,
/// efectos de cada pista, volumen, panorama y bus máster.
pub fn mixdown(inputs: &[MixInput], master: &MasterSettings, sample_rate: u32) -> AudioBuffer {
    let len = inputs.iter().map(|t| t.data.len()).max().unwrap_or(0);
    let any_solo = inputs.iter().any(|t| t.solo);
    let mut data = vec![0.0f32; len];
    let mut block = vec![0.0f32; BLOCK * 2];
    for t in inputs {
        if t.mute || (any_solo && !t.solo) {
            continue;
        }
        let (gl, gr) = balance_gains(db_to_gain(t.gain_db), t.pan);
        let mut chain = (!t.effects.is_empty()).then(|| Chain::new(t.effects, sample_rate));
        // Se procesa toda la longitud de la mezcla para no cortar colas de reverb o delay.
        for (start, out) in (0..len).step_by(BLOCK * 2).zip(data.chunks_mut(BLOCK * 2)) {
            let n = out.len();
            let src = t.data.get(start..(start + n).min(t.data.len())).unwrap_or(&[]);
            block[..src.len()].copy_from_slice(src);
            block[src.len()..n].fill(0.0);
            if let Some(c) = chain.as_mut() {
                c.process(&mut block[..n]);
            }
            for (o, s) in out.chunks_exact_mut(2).zip(block[..n].chunks_exact(2)) {
                o[0] += s[0] * gl;
                o[1] += s[1] * gr;
            }
        }
    }
    if !master.effects.is_empty() {
        let mut chain = Chain::new(&master.effects, sample_rate);
        for chunk in data.chunks_mut(BLOCK * 2) {
            chain.process(chunk);
        }
    }
    let g = db_to_gain(master.gain_db);
    if g != 1.0 {
        data.iter_mut().for_each(|s| *s *= g);
    }
    AudioBuffer { sample_rate, data }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mn_fx::{EffectParams, LimiterParams};

    fn input<'a>(data: &'a [f32], gain_db: f32, pan: f32, solo: bool) -> MixInput<'a> {
        MixInput { data, gain_db, pan, mute: false, solo, effects: &[] }
    }

    #[test]
    fn mixdown_respects_solo_and_pan() {
        let a = vec![0.5f32; 8];
        let b = vec![0.25f32; 4];
        let m = mixdown(&[input(&a, 0.0, 1.0, false), input(&b, 0.0, 0.0, false)], &MasterSettings::default(), 10);
        assert_eq!(m.data, vec![0.25, 0.75, 0.25, 0.75, 0.0, 0.5, 0.0, 0.5]);

        let m = mixdown(&[input(&a, 0.0, 0.0, false), input(&b, -60.0, 0.0, true)], &MasterSettings::default(), 10);
        assert!(m.data.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn mixdown_applies_track_and_master_effects() {
        let sr = 48_000;
        let sine: Vec<f32> = (0..sr as usize)
            .flat_map(|i| {
                let s = 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / sr as f32).sin();
                [s, s]
            })
            .collect();
        let lim = |ceiling_db| {
            vec![EffectSlot {
                id: "l".into(),
                enabled: true,
                params: EffectParams::Limiter(LimiterParams { input_db: 0.0, ceiling_db, release_ms: 50.0, true_peak: false }),
            }]
        };
        let track_fx = lim(-12.0);
        let peak = |b: &AudioBuffer| b.data.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let t = MixInput { data: &sine, gain_db: 0.0, pan: 0.0, mute: false, solo: false, effects: &track_fx };
        let m = mixdown(&[t], &MasterSettings::default(), sr);
        assert!((0.24..=0.2512).contains(&peak(&m)), "{}", peak(&m));

        let master = MasterSettings { gain_db: -6.0, effects: lim(-3.0) };
        let m = mixdown(&[input(&sine, 0.0, 0.0, false)], &master, sr);
        // Limitado a -3 dBFS (sin efecto: el pico es -6) y después -6 dB de ganancia.
        assert!((peak(&m) - 0.25).abs() < 0.01, "{}", peak(&m));
    }
}
