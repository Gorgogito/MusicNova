//! Ecualizador paramétrico: paso alto, estante grave, dos campanas, estante agudo y paso bajo.

use std::f64::consts::FRAC_1_SQRT_2;

use crate::biquad::{Biquad, Coefs};
use crate::{EffectParams, EqParams, Processor};

pub struct Eq {
    sr: f64,
    params: EqParams,
    /// Filtros activos; los planos se omiten.
    filters: [Biquad; 6],
    active: [bool; 6],
}

impl Eq {
    pub fn new(params: EqParams, sr: f32) -> Self {
        let mut eq = Self {
            sr: sr as f64,
            params,
            filters: std::array::from_fn(|_| Biquad::new(Coefs::IDENTITY)),
            active: [false; 6],
        };
        eq.update();
        eq
    }

    fn update(&mut self) {
        let p = self.params;
        let sr = self.sr;
        let bands: [(bool, Coefs); 6] = [
            (p.highpass > 0.0, Coefs::highpass(p.highpass as f64, FRAC_1_SQRT_2, sr)),
            (p.low_shelf.gain_db != 0.0, Coefs::low_shelf(p.low_shelf.freq as f64, p.low_shelf.gain_db as f64, p.low_shelf.q as f64, sr)),
            (p.mid1.gain_db != 0.0, Coefs::peaking(p.mid1.freq as f64, p.mid1.gain_db as f64, p.mid1.q as f64, sr)),
            (p.mid2.gain_db != 0.0, Coefs::peaking(p.mid2.freq as f64, p.mid2.gain_db as f64, p.mid2.q as f64, sr)),
            (p.high_shelf.gain_db != 0.0, Coefs::high_shelf(p.high_shelf.freq as f64, p.high_shelf.gain_db as f64, p.high_shelf.q as f64, sr)),
            (p.lowpass > 0.0, Coefs::lowpass(p.lowpass as f64, FRAC_1_SQRT_2, sr)),
        ];
        for (i, (on, coefs)) in bands.into_iter().enumerate() {
            if on && !self.active[i] {
                self.filters[i].reset();
            }
            self.active[i] = on;
            self.filters[i].coefs = coefs;
        }
    }
}

impl Processor for Eq {
    fn process(&mut self, buf: &mut [f32]) {
        for (f, on) in self.filters.iter_mut().zip(self.active) {
            if on {
                f.process(buf);
            }
        }
    }

    fn reset(&mut self) {
        self.filters.iter_mut().for_each(Biquad::reset);
    }

    fn set_params(&mut self, params: &EffectParams) -> bool {
        let EffectParams::Eq(p) = params else { return false };
        self.params = *p;
        self.update();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Band;

    fn tone_gain_db(eq: &mut Eq, freq: f64) -> f64 {
        let sr = 48_000.0;
        let n = 24_000;
        let mut buf: Vec<f32> = (0..n)
            .flat_map(|i| {
                let s = (std::f64::consts::TAU * freq * i as f64 / sr).sin() as f32 * 0.5;
                [s, s]
            })
            .collect();
        eq.reset();
        eq.process(&mut buf);
        let tail = &buf[n..];
        let rms = (tail.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
        20.0 * (rms * 2f64.sqrt() / 0.5).log10()
    }

    #[test]
    fn bands_shape_the_spectrum() {
        let p = EqParams {
            highpass: 80.0,
            mid1: Band { freq: 1000.0, gain_db: 6.0, q: 1.0 },
            ..Default::default()
        };
        let mut eq = Eq::new(p, 48_000.0);
        assert!(tone_gain_db(&mut eq, 30.0) < -15.0);
        assert!((tone_gain_db(&mut eq, 1000.0) - 6.0).abs() < 0.2);
        assert!(tone_gain_db(&mut eq, 10_000.0).abs() < 0.3);
    }
}
