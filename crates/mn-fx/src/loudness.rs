//! Sonoridad según ITU-R BS.1770-4 / EBU R128: ponderación K, sonoridad
//! momentánea (400 ms), a corto plazo (3 s), integrada (con umbrales absoluto
//! y relativo) y pico real (true peak) con sobremuestreo ×4.

use crate::biquad::{Biquad, Coefs};

/// Filtro de ponderación K: estante agudo + paso alto. Se derivan con la
/// transformada bilineal de los prototipos analógicos de BS.1770 (igual que
/// libebur128), de modo que a 48 kHz coinciden con los coeficientes de la norma.
fn k_filters(sr: f64) -> [Biquad; 2] {
    let (f0, g, q) = (1681.974_450_955_533, 3.999_843_853_973_347, 0.707_175_236_955_419_6);
    let k = (std::f64::consts::PI * f0 / sr).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.499_666_774_154_541_6);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Coefs::raw(
        (vh + vb * k / q + k * k) / a0,
        2.0 * (k * k - vh) / a0,
        (vh - vb * k / q + k * k) / a0,
        2.0 * (k * k - 1.0) / a0,
        (1.0 - k / q + k * k) / a0,
    );
    let (f0, q) = (38.135_470_876_024_44, 0.500_327_037_323_877_3);
    let k = (std::f64::consts::PI * f0 / sr).tan();
    let a0 = 1.0 + k / q + k * k;
    let highpass = Coefs::raw(1.0, -2.0, 1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0);
    [Biquad::new(shelf), Biquad::new(highpass)]
}

#[inline]
fn lufs(mean_square: f64) -> f64 {
    if mean_square <= 0.0 {
        f64::NEG_INFINITY
    } else {
        -0.691 + 10.0 * mean_square.log10()
    }
}

/// Medidor en tiempo real (sin memoria dinámica al procesar).
pub struct LoudnessMeter {
    k: [Biquad; 2],
    /// Frames por sub-bloque de 100 ms.
    sub_len: usize,
    sub_pos: usize,
    sub_acc: f64,
    /// Energía media de los últimos 30 sub-bloques (3 s).
    history: [f64; 30],
    hist_pos: usize,
    filled: usize,
}

impl LoudnessMeter {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            k: k_filters(sample_rate as f64),
            sub_len: (sample_rate as usize / 10).max(1),
            sub_pos: 0,
            sub_acc: 0.0,
            history: [0.0; 30],
            hist_pos: 0,
            filled: 0,
        }
    }

    pub fn process(&mut self, buf: &[f32]) {
        for frame in buf.chunks_exact(2) {
            let mut e = 0.0;
            for (ch, &x) in frame.iter().enumerate() {
                let s = self.k[0].tick(ch, x as f64);
                let y = self.k[1].tick(ch, s);
                e += y * y;
            }
            self.sub_acc += e;
            self.sub_pos += 1;
            if self.sub_pos == self.sub_len {
                self.history[self.hist_pos] = self.sub_acc / self.sub_len as f64;
                self.hist_pos = (self.hist_pos + 1) % self.history.len();
                self.filled = (self.filled + 1).min(self.history.len());
                self.sub_pos = 0;
                self.sub_acc = 0.0;
            }
        }
    }

    fn mean_of_last(&self, n: usize) -> f64 {
        let n = n.min(self.filled);
        if n == 0 {
            return 0.0;
        }
        let len = self.history.len();
        (1..=n).map(|k| self.history[(self.hist_pos + len - k) % len]).sum::<f64>() / n as f64
    }

    /// Sonoridad momentánea (ventana de 400 ms), en LUFS.
    pub fn momentary(&self) -> f64 {
        lufs(self.mean_of_last(4))
    }

    /// Sonoridad a corto plazo (ventana de 3 s), en LUFS.
    pub fn short_term(&self) -> f64 {
        lufs(self.mean_of_last(30))
    }

    pub fn reset(&mut self) {
        self.k.iter_mut().for_each(Biquad::reset);
        self.history = [0.0; 30];
        self.filled = 0;
        self.sub_pos = 0;
        self.sub_acc = 0.0;
    }
}

/// Medidas de un audio completo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoudnessReport {
    /// Sonoridad integrada (LUFS); `-inf` si todo está por debajo del umbral absoluto.
    pub integrated: f64,
    /// Rango de sonoridad (LRA, en LU).
    pub range: f64,
    /// Pico real en dBTP.
    pub true_peak_db: f64,
    /// Pico de muestra en dBFS.
    pub sample_peak_db: f64,
}

/// Mide la sonoridad integrada, el rango y los picos de un audio estéreo entrelazado.
pub fn measure(buf: &[f32], sample_rate: u32) -> LoudnessReport {
    let (integrated, range) = loudness_only(buf, sample_rate);
    let sample_peak = buf.iter().fold(0.0f32, |m, s| m.max(s.abs())) as f64;
    let to_db = |x: f64| if x > 0.0 { 20.0 * x.log10() } else { f64::NEG_INFINITY };
    LoudnessReport { integrated, range, true_peak_db: to_db(true_peak(buf)), sample_peak_db: to_db(sample_peak) }
}

/// Sonoridad integrada (LUFS) y rango de sonoridad (LU), sin calcular picos.
pub fn loudness_only(buf: &[f32], sample_rate: u32) -> (f64, f64) {
    let sr = sample_rate as f64;
    let mut k = k_filters(sr);
    let step = (sample_rate as usize / 10).max(1); // 100 ms
    // Energía por sub-bloque de 100 ms.
    let mut subs = Vec::with_capacity(buf.len() / 2 / step + 1);
    let mut acc = 0.0;
    let mut n = 0;
    for frame in buf.chunks_exact(2) {
        for (ch, &x) in frame.iter().enumerate() {
            let s = k[0].tick(ch, x as f64);
            let y = k[1].tick(ch, s);
            acc += y * y;
        }
        n += 1;
        if n == step {
            subs.push(acc / step as f64);
            acc = 0.0;
            n = 0;
        }
    }
    let gated = |win: usize| -> Vec<f64> {
        // Bloques de `win` sub-bloques con solape de un sub-bloque (75 % para 400 ms).
        if subs.len() < win {
            return Vec::new();
        }
        (0..=subs.len() - win).map(|i| subs[i..i + win].iter().sum::<f64>() / win as f64).collect()
    };

    // Integrada: bloques de 400 ms, umbral absoluto -70 LUFS y relativo -10 LU.
    let blocks = gated(4);
    let above_abs: Vec<f64> = blocks.iter().copied().filter(|&e| lufs(e) > -70.0).collect();
    let integrated = if above_abs.is_empty() {
        f64::NEG_INFINITY
    } else {
        let rel = lufs(above_abs.iter().sum::<f64>() / above_abs.len() as f64) - 10.0;
        let kept: Vec<f64> = above_abs.iter().copied().filter(|&e| lufs(e) > rel).collect();
        lufs(kept.iter().sum::<f64>() / kept.len() as f64)
    };

    // LRA (EBU Tech 3342): bloques de 3 s, umbral relativo -20 LU, percentiles 10–95.
    let st = gated(30);
    let st_abs: Vec<f64> = st.iter().copied().filter(|&e| lufs(e) > -70.0).collect();
    let range = if st_abs.len() < 2 {
        0.0
    } else {
        let rel = lufs(st_abs.iter().sum::<f64>() / st_abs.len() as f64) - 20.0;
        let mut l: Vec<f64> = st_abs.iter().map(|&e| lufs(e)).filter(|&v| v > rel).collect();
        l.sort_by(f64::total_cmp);
        if l.len() < 2 {
            0.0
        } else {
            let pct = |p: f64| l[((l.len() - 1) as f64 * p).round() as usize];
            pct(0.95) - pct(0.10)
        }
    };

    (integrated, range)
}

/// Pico real: interpolación ×4 con un filtro sinc enventanado (48 tomas por fase).
pub fn true_peak(buf: &[f32]) -> f64 {
    const PHASES: usize = 4;
    const TAPS: usize = 48;
    let half = TAPS as f64 / 2.0;
    let kernel: Vec<[f64; TAPS]> = (0..PHASES)
        .map(|ph| {
            std::array::from_fn(|t| {
                let x = t as f64 - half + 1.0 - ph as f64 / PHASES as f64;
                let sinc = if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                // Ventana de Blackman sobre la longitud total del filtro.
                let w = (t as f64 + ph as f64 / PHASES as f64) / TAPS as f64;
                let win = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * w).cos() + 0.08 * (4.0 * std::f64::consts::PI * w).cos();
                sinc * win
            })
        })
        .collect();
    let frames = buf.len() / 2;
    let h = half as usize;
    let mut peak = 0.0f64;
    // Cada canal en un vector contiguo con ceros a los lados: el bucle interno
    // trabaja sobre porciones sin comprobaciones de límites (vectorizable).
    let mut chan = vec![0.0f32; frames + TAPS];
    let kernel32: Vec<[f32; TAPS]> = kernel.iter().map(|k| std::array::from_fn(|t| k[t] as f32)).collect();
    for ch in 0..2 {
        for (i, f) in buf.chunks_exact(2).enumerate() {
            chan[h - 1 + i] = f[ch];
        }
        let mut pk = 0.0f32;
        for n in 0..frames {
            pk = pk.max(chan[h - 1 + n].abs());
            let window = &chan[n..n + TAPS];
            for k in kernel32.iter().skip(1) {
                // 8 acumuladores independientes: permite vectorizar la suma.
                let mut lanes = [0.0f32; 8];
                for (xs, cs) in window.chunks_exact(8).zip(k.chunks_exact(8)) {
                    for l in 0..8 {
                        lanes[l] += xs[l] * cs[l];
                    }
                }
                let acc: f32 = lanes.iter().sum();
                pk = pk.max(acc.abs());
            }
        }
        peak = peak.max(pk as f64);
    }
    peak
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo_sine(freq: f64, amp: f64, secs: f64, sr: u32) -> Vec<f32> {
        let n = (secs * sr as f64) as usize;
        (0..n)
            .flat_map(|i| {
                let s = (amp * (std::f64::consts::TAU * freq * i as f64 / sr as f64).sin()) as f32;
                [s, s]
            })
            .collect()
    }

    #[test]
    fn k_weighting_matches_bs1770_coefficients_at_48k() {
        // Coeficientes publicados en ITU-R BS.1770-4 (tabla 1 y 2).
        let [shelf, hp] = k_filters(48_000.0);
        let expect = Coefs::raw(1.535_124_859_586_97, -2.691_696_189_406_38, 1.198_392_810_852_85, -1.690_659_293_182_41, 0.732_480_774_215_85);
        assert!((shelf.coefs.magnitude(1000.0, 48_000.0) - expect.magnitude(1000.0, 48_000.0)).abs() < 1e-6);
        assert!((shelf.coefs.magnitude(10_000.0, 48_000.0) - expect.magnitude(10_000.0, 48_000.0)).abs() < 1e-6);
        let expect_hp = Coefs::raw(1.0, -2.0, 1.0, -1.990_047_454_833_98, 0.990_072_250_366_21);
        assert!((hp.coefs.magnitude(30.0, 48_000.0) - expect_hp.magnitude(30.0, 48_000.0)).abs() < 1e-6);
    }

    #[test]
    fn ebu_tech_3341_reference_sine() {
        // Seno estéreo de 1 kHz a -23 dBFS → -23,0 LUFS (EBU Tech 3341, prueba 1).
        for sr in [44_100, 48_000] {
            let buf = stereo_sine(1000.0, 10f64.powf(-23.0 / 20.0), 20.0, sr);
            let r = measure(&buf, sr);
            assert!((r.integrated + 23.0).abs() < 0.1, "{sr}: {}", r.integrated);
            let mut m = LoudnessMeter::new(sr);
            m.process(&buf);
            assert!((m.momentary() + 23.0).abs() < 0.1 && (m.short_term() + 23.0).abs() < 0.1);
        }
    }

    #[test]
    fn relative_gate_ignores_quiet_parts() {
        // EBU Tech 3341, prueba 3 (simplificada): -36 / -23 / -36 dBFS → ≈ -23 LUFS.
        let sr = 48_000;
        let mut buf = stereo_sine(1000.0, 10f64.powf(-36.0 / 20.0), 10.0, sr);
        buf.extend(stereo_sine(1000.0, 10f64.powf(-23.0 / 20.0), 60.0, sr));
        buf.extend(stereo_sine(1000.0, 10f64.powf(-36.0 / 20.0), 10.0, sr));
        assert!((measure(&buf, sr).integrated + 23.0).abs() < 0.1);
        assert_eq!(measure(&vec![0.0; 96_000], sr).integrated, f64::NEG_INFINITY);
    }

    #[test]
    fn true_peak_exceeds_sample_peak_between_samples() {
        // Seno a fs/4 con fase de 45°: las muestras valen 0,707 pero el pico real es 1.
        let sr = 48_000;
        let buf: Vec<f32> = (0..4800)
            .flat_map(|i| {
                let s = (std::f64::consts::TAU * 12_000.0 * i as f64 / sr as f64 + std::f64::consts::FRAC_PI_4).sin() as f32;
                [s, s]
            })
            .collect();
        let r = measure(&buf, sr);
        assert!((r.sample_peak_db + 3.01).abs() < 0.05, "{}", r.sample_peak_db);
        assert!(r.true_peak_db.abs() < 0.3, "{}", r.true_peak_db);
    }
}
