//! Compresor de realimentación directa con detección enlazada en estéreo,
//! rodilla suave y suavizado de la reducción de ganancia (ataque/liberación).

use crate::{db_to_lin, time_coef, CompressorParams, EffectParams, Processor};

pub struct Compressor {
    sr: f32,
    p: CompressorParams,
    attack: f32,
    release: f32,
    makeup: f32,
    /// Reducción actual en dB (≥ 0).
    gr_db: f32,
}

impl Compressor {
    pub fn new(p: CompressorParams, sr: f32) -> Self {
        let mut c = Self { sr, p, attack: 0.0, release: 0.0, makeup: 1.0, gr_db: 0.0 };
        c.update();
        c
    }

    fn update(&mut self) {
        self.attack = time_coef(self.p.attack_ms, self.sr);
        self.release = time_coef(self.p.release_ms, self.sr);
        self.makeup = db_to_lin(self.p.makeup_db);
    }

    /// Reducción estática (dB) para un nivel de entrada (dB).
    pub fn static_gr(&self, level_db: f32) -> f32 {
        let (t, r, k) = (self.p.threshold_db, self.p.ratio, self.p.knee_db);
        let over = level_db - t;
        let slope = 1.0 - 1.0 / r;
        if k > 0.0 && over.abs() <= k / 2.0 {
            slope * (over + k / 2.0).powi(2) / (2.0 * k)
        } else if over > 0.0 {
            slope * over
        } else {
            0.0
        }
    }

    /// Reducción de ganancia actual en dB, para medidores.
    pub fn gain_reduction_db(&self) -> f32 {
        self.gr_db
    }
}

impl Processor for Compressor {
    fn process(&mut self, buf: &mut [f32]) {
        for frame in buf.chunks_exact_mut(2) {
            let peak = frame[0].abs().max(frame[1].abs()).max(1e-9);
            let target = self.static_gr(20.0 * peak.log10());
            // Más reducción → ataque; menos → liberación.
            let coef = if target > self.gr_db { self.attack } else { self.release };
            self.gr_db = target + coef * (self.gr_db - target);
            let g = db_to_lin(-self.gr_db) * self.makeup;
            frame[0] *= g;
            frame[1] *= g;
        }
    }

    fn reset(&mut self) {
        self.gr_db = 0.0;
    }

    fn set_params(&mut self, params: &EffectParams) -> bool {
        let EffectParams::Compressor(p) = params else { return false };
        self.p = *p;
        self.update();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level_db(buf: &[f32]) -> f32 {
        let rms = (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt();
        20.0 * (rms * 2f32.sqrt()).log10()
    }

    fn sine(amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .flat_map(|i| {
                let s = amp * (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin();
                [s, s]
            })
            .collect()
    }

    #[test]
    fn static_curve_with_knee() {
        let c = Compressor::new(CompressorParams { threshold_db: -20.0, ratio: 4.0, knee_db: 0.0, ..Default::default() }, 48_000.0);
        assert_eq!(c.static_gr(-30.0), 0.0);
        assert!((c.static_gr(-8.0) - 9.0).abs() < 1e-4, "12 dB por encima a 4:1 → 9 dB de reducción");
        let soft = Compressor::new(CompressorParams { threshold_db: -20.0, ratio: 4.0, knee_db: 6.0, ..Default::default() }, 48_000.0);
        assert!(soft.static_gr(-20.0) > 0.0 && soft.static_gr(-20.0) < 1.0);
        assert!((soft.static_gr(-5.0) - 11.25).abs() < 1e-3);
    }

    #[test]
    fn compresses_loud_sine_to_expected_level() {
        // Seno a -6 dBFS de pico, umbral -20, 4:1 → salida ≈ -16,5 dBFS (detección por pico).
        let p = CompressorParams { threshold_db: -20.0, ratio: 4.0, knee_db: 0.0, attack_ms: 5.0, release_ms: 50.0, makeup_db: 0.0 };
        let mut c = Compressor::new(p, 48_000.0);
        let mut buf = sine(db_to_lin(-6.0), 48_000);
        c.process(&mut buf);
        let out = level_db(&buf[48_000..]);
        assert!((-18.0..-15.0).contains(&out), "{out}");
        // Por debajo del umbral no cambia nada.
        let mut c = Compressor::new(p, 48_000.0);
        let mut quiet = sine(db_to_lin(-30.0), 4800);
        let orig = quiet.clone();
        c.process(&mut quiet);
        assert_eq!(quiet, orig);
    }
}
