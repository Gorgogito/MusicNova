//! Filtros biquad (fórmulas del «Audio EQ Cookbook» de R. Bristow-Johnson),
//! en forma directa transpuesta II con estado por canal.

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coefs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coefs {
    pub const IDENTITY: Coefs = Coefs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    /// Coeficientes ya normalizados (a0 = 1).
    pub fn raw(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Self { b0, b1, b2, a1, a2 }
    }

    fn normalized(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
    }

    fn w0(freq: f64, sr: f64) -> (f64, f64) {
        let w0 = 2.0 * PI * freq.clamp(1.0, sr * 0.49) / sr;
        (w0.cos(), w0.sin())
    }

    pub fn highpass(freq: f64, q: f64, sr: f64) -> Self {
        let (cos, sin) = Self::w0(freq, sr);
        let alpha = sin / (2.0 * q);
        Self::normalized((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    pub fn lowpass(freq: f64, q: f64, sr: f64) -> Self {
        let (cos, sin) = Self::w0(freq, sr);
        let alpha = sin / (2.0 * q);
        Self::normalized((1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    pub fn peaking(freq: f64, gain_db: f64, q: f64, sr: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let (cos, sin) = Self::w0(freq, sr);
        let alpha = sin / (2.0 * q);
        Self::normalized(1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a)
    }

    pub fn low_shelf(freq: f64, gain_db: f64, q: f64, sr: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let (cos, sin) = Self::w0(freq, sr);
        let alpha = sin / (2.0 * q);
        let s = 2.0 * a.sqrt() * alpha;
        Self::normalized(
            a * ((a + 1.0) - (a - 1.0) * cos + s),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - s),
            (a + 1.0) + (a - 1.0) * cos + s,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - s,
        )
    }

    pub fn high_shelf(freq: f64, gain_db: f64, q: f64, sr: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let (cos, sin) = Self::w0(freq, sr);
        let alpha = sin / (2.0 * q);
        let s = 2.0 * a.sqrt() * alpha;
        Self::normalized(
            a * ((a + 1.0) + (a - 1.0) * cos + s),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - s),
            (a + 1.0) - (a - 1.0) * cos + s,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - s,
        )
    }

    /// Ganancia (lineal) de la respuesta en frecuencia en `freq`.
    pub fn magnitude(&self, freq: f64, sr: f64) -> f64 {
        let w = 2.0 * PI * freq / sr;
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let num = ((self.b0 + self.b1 * c1 + self.b2 * c2).powi(2) + (self.b1 * s1 + self.b2 * s2).powi(2)).sqrt();
        let den = ((1.0 + self.a1 * c1 + self.a2 * c2).powi(2) + (self.a1 * s1 + self.a2 * s2).powi(2)).sqrt();
        num / den
    }
}

/// Un biquad estéreo.
#[derive(Clone, Debug)]
pub struct Biquad {
    pub coefs: Coefs,
    z1: [f64; 2],
    z2: [f64; 2],
}

impl Biquad {
    pub fn new(coefs: Coefs) -> Self {
        Self { coefs, z1: [0.0; 2], z2: [0.0; 2] }
    }

    #[inline]
    pub fn tick(&mut self, ch: usize, x: f64) -> f64 {
        let c = &self.coefs;
        let y = c.b0 * x + self.z1[ch];
        self.z1[ch] = c.b1 * x - c.a1 * y + self.z2[ch];
        self.z2[ch] = c.b2 * x - c.a2 * y;
        y
    }

    /// Procesa estéreo entrelazado en el sitio.
    pub fn process(&mut self, buf: &mut [f32]) {
        for frame in buf.chunks_exact_mut(2) {
            frame[0] = self.tick(0, frame[0] as f64) as f32;
            frame[1] = self.tick(1, frame[1] as f64) as f32;
        }
    }

    pub fn reset(&mut self) {
        self.z1 = [0.0; 2];
        self.z2 = [0.0; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;
    fn db(x: f64) -> f64 {
        20.0 * x.log10()
    }

    #[test]
    fn responses_match_design() {
        let hp = Coefs::highpass(100.0, std::f64::consts::FRAC_1_SQRT_2, SR);
        assert!((db(hp.magnitude(100.0, SR)) + 3.01).abs() < 0.05);
        assert!(db(hp.magnitude(20.0, SR)) < -27.0);
        assert!(db(hp.magnitude(5000.0, SR)).abs() < 0.01);

        let pk = Coefs::peaking(1000.0, 6.0, 1.0, SR);
        assert!((db(pk.magnitude(1000.0, SR)) - 6.0).abs() < 0.01);
        assert!(db(pk.magnitude(50.0, SR)).abs() < 0.1);

        let ls = Coefs::low_shelf(200.0, -4.0, 0.707, SR);
        assert!((db(ls.magnitude(20.0, SR)) + 4.0).abs() < 0.1);
        assert!(db(ls.magnitude(10_000.0, SR)).abs() < 0.05);

        let hs = Coefs::high_shelf(8000.0, 3.0, 0.707, SR);
        assert!((db(hs.magnitude(20_000.0, SR)) - 3.0).abs() < 0.2);
    }

    #[test]
    fn filtering_matches_magnitude() {
        let mut f = Biquad::new(Coefs::peaking(1000.0, -12.0, 2.0, SR));
        let n = 48_000;
        let mut buf: Vec<f32> = (0..n)
            .flat_map(|i| {
                let s = (2.0 * PI * 1000.0 * i as f64 / SR).sin() as f32;
                [s, s]
            })
            .collect();
        f.process(&mut buf);
        let tail = &buf[n..];
        let rms = (tail.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
        assert!((db(rms * 2f64.sqrt()) + 12.0).abs() < 0.1, "{}", db(rms * 2f64.sqrt()));
    }
}
