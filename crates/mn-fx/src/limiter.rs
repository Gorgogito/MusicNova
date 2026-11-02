//! Limitador de picos con anticipación (lookahead), opcionalmente de pico real.
//!
//! La ganancia necesaria para cada muestra se pasa por un filtro de mínimo
//! sobre la ventana de anticipación y después por una media móvil de la misma
//! longitud: así la ganancia ya ha bajado del todo cuando llega el pico
//! (retrasado) y nunca se supera el techo. La recuperación es exponencial.
//!
//! En modo pico real (true peak) el pico de cada muestra incluye los valores
//! interpolados ×4 hasta la muestra siguiente, de modo que tampoco se supera el
//! techo entre muestras (lo que miden las plataformas en dBTP).

use crate::{db_to_lin, time_coef, EffectParams, LimiterParams, Processor};

const LOOKAHEAD_MS: f32 = 1.5;
/// Fases de sobremuestreo y media longitud del filtro interpolador (frames).
const OVERSAMPLE: usize = 4;
const TP_HALF: usize = 16;

/// Filtro sinc enventanado (Blackman) por fase: `kernel[p][t]` interpola el
/// punto `m + p/4` a partir de las muestras `m - TP_HALF + 1 + t`.
fn interp_kernel() -> [[f32; 2 * TP_HALF]; OVERSAMPLE] {
    std::array::from_fn(|p| {
        std::array::from_fn(|t| {
            let x = (TP_HALF as f64 - 1.0 - t as f64) + p as f64 / OVERSAMPLE as f64;
            let sinc = if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
            let u = (x + TP_HALF as f64) / (2.0 * TP_HALF as f64);
            let tau = std::f64::consts::TAU;
            let w = 0.42 - 0.5 * (tau * u).cos() + 0.08 * (2.0 * tau * u).cos();
            (sinc * w) as f32
        })
    })
}

pub struct Limiter {
    sr: f32,
    p: LimiterParams,
    input: f32,
    ceiling: f32,
    release: f32,
    look: usize,
    /// Retardo de detección: 0, o `2·TP_HALF` en modo pico real.
    detect_delay: usize,
    /// Ventana del filtro de mínimo (frames): `look`, o `look + 2·TP_HALF` en
    /// modo pico real para que la ganancia ya esté al mínimo en las muestras
    /// vecinas que intervienen en la interpolación de cada pico.
    min_window: usize,
    kernel: [[f32; 2 * TP_HALF]; OVERSAMPLE],
    /// Últimas `2·TP_HALF` muestras de entrada por canal, en un anillo
    /// duplicado (cada muestra se escribe dos veces) para que la ventana de
    /// interpolación sea siempre una porción contigua.
    hist: [[f32; 4 * TP_HALF]; 2],
    hist_pos: usize,
    /// Señal retrasada (estéreo entrelazado), anillo de `look + detect_delay` frames.
    delay: Vec<f32>,
    pos: usize,
    /// Cola monótona para el mínimo deslizante: (índice absoluto, ganancia).
    deque: Vec<(u64, f32)>,
    dq_head: usize,
    dq_len: usize,
    /// Media móvil de la ganancia mínima.
    avg_buf: Vec<f32>,
    avg_sum: f64,
    gain: f32,
    n: u64,
}

impl Limiter {
    pub fn new(p: LimiterParams, sr: f32) -> Self {
        let look = ((LOOKAHEAD_MS * 0.001 * sr) as usize).max(1);
        let mut l = Self {
            sr,
            p,
            input: 1.0,
            ceiling: 1.0,
            release: 0.0,
            look,
            detect_delay: 0,
            min_window: look,
            kernel: interp_kernel(),
            hist: [[0.0; 4 * TP_HALF]; 2],
            hist_pos: 0,
            delay: Vec::new(),
            pos: 0,
            deque: vec![(0, 1.0); look + 2 * TP_HALF + 1],
            dq_head: 0,
            dq_len: 0,
            avg_buf: vec![1.0; look],
            avg_sum: look as f64,
            gain: 1.0,
            n: 0,
        };
        l.update();
        l.resize_delay();
        l
    }

    fn update(&mut self) {
        self.input = db_to_lin(self.p.input_db);
        // Margen por debajo del techo frente al redondeo de la media y, en modo
        // pico real, a la aproximación del interpolador.
        self.ceiling = db_to_lin(self.p.ceiling_db) * if self.p.true_peak { 0.994 } else { 0.9999 };
        self.release = time_coef(self.p.release_ms, self.sr);
    }

    /// Ajusta la línea de retardo al modo (reserva memoria: solo al crear o al
    /// cambiar de modo, nunca al procesar en el caso habitual).
    fn resize_delay(&mut self) {
        // Pico real: TP_HALF frames para centrar el interpolador y otros TP_HALF
        // para mantener la ganancia antes del pico (ver `min_window`).
        self.detect_delay = if self.p.true_peak { 2 * TP_HALF } else { 0 };
        self.min_window = self.look + self.detect_delay;
        self.dq_len = 0;
        let frames = self.look + self.detect_delay;
        if self.delay.len() != frames * 2 {
            self.delay = vec![0.0; frames * 2];
            self.pos = 0;
        }
    }

    /// Latencia en frames (anticipación más retardo de detección).
    pub fn latency(&self) -> usize {
        self.look + self.detect_delay
    }

    #[inline]
    fn push_min(&mut self, g: f32) -> f32 {
        let cap = self.deque.len();
        // Retira por detrás las ganancias mayores (nunca serán mínimas).
        while self.dq_len > 0 {
            let back = (self.dq_head + self.dq_len - 1) % cap;
            if self.deque[back].1 >= g {
                self.dq_len -= 1;
            } else {
                break;
            }
        }
        let slot = (self.dq_head + self.dq_len) % cap;
        self.deque[slot] = (self.n, g);
        self.dq_len += 1;
        // Retira por delante lo que salió de la ventana de `min_window + 1` muestras.
        while self.deque[self.dq_head].0 + (self.min_window as u64) < self.n {
            self.dq_head = (self.dq_head + 1) % cap;
            self.dq_len -= 1;
        }
        self.deque[self.dq_head].1
    }

    /// Pico real de la muestra de hace `TP_HALF` frames (incluye los puntos
    /// interpolados hasta la siguiente).
    #[inline]
    fn true_peak_at_center(&self) -> f32 {
        let len = 2 * TP_HALF;
        let mut peak = 0.0f32;
        for ch in 0..2 {
            // Ventana contigua, de la muestra más antigua a la más reciente.
            let window = &self.hist[ch][self.hist_pos..self.hist_pos + len];
            // Muestra central: la de índice TP_HALF - 1 contando desde la más antigua.
            peak = peak.max(window[TP_HALF - 1].abs());
            for k in &self.kernel[1..] {
                // 8 acumuladores independientes: permite vectorizar.
                let mut lanes = [0.0f32; 8];
                for (xs, cs) in window.chunks_exact(8).zip(k.chunks_exact(8)) {
                    for l in 0..8 {
                        lanes[l] += xs[l] * cs[l];
                    }
                }
                peak = peak.max(lanes.iter().sum::<f32>().abs());
            }
        }
        peak
    }
}

impl Processor for Limiter {
    fn process(&mut self, buf: &mut [f32]) {
        let frames = self.delay.len() / 2;
        for frame in buf.chunks_exact_mut(2) {
            let (l, r) = (frame[0] * self.input, frame[1] * self.input);
            let peak = if self.detect_delay > 0 {
                let len = 2 * TP_HALF;
                for (ch, x) in [l, r].into_iter().enumerate() {
                    self.hist[ch][self.hist_pos] = x;
                    self.hist[ch][self.hist_pos + len] = x;
                }
                self.hist_pos = (self.hist_pos + 1) % len;
                self.true_peak_at_center()
            } else {
                l.abs().max(r.abs())
            };
            let need = if peak > self.ceiling { self.ceiling / peak } else { 1.0 };
            let min = self.push_min(need);
            // Media móvil de `look` muestras del mínimo.
            let i = (self.n % self.look as u64) as usize;
            self.avg_sum += min as f64 - self.avg_buf[i] as f64;
            self.avg_buf[i] = min;
            let target = (self.avg_sum / self.look as f64) as f32;
            self.gain = if target < self.gain { target } else { target + self.release * (self.gain - target) };
            // Salida: la muestra de hace `look + detect_delay` frames con la ganancia actual.
            let j = self.pos * 2;
            let (dl, dr) = (self.delay[j], self.delay[j + 1]);
            self.delay[j] = l;
            self.delay[j + 1] = r;
            self.pos = (self.pos + 1) % frames;
            frame[0] = dl * self.gain;
            frame[1] = dr * self.gain;
            self.n += 1;
        }
    }

    fn reset(&mut self) {
        self.delay.fill(0.0);
        self.hist = [[0.0; 4 * TP_HALF]; 2];
        self.dq_len = 0;
        self.avg_buf.fill(1.0);
        self.avg_sum = self.look as f64;
        self.gain = 1.0;
    }

    fn set_params(&mut self, params: &EffectParams) -> bool {
        let EffectParams::Limiter(p) = params else { return false };
        let mode_changed = p.true_peak != self.p.true_peak;
        self.p = *p;
        self.update();
        if mode_changed {
            // Cambiar de modo cambia la latencia: es un cambio poco frecuente
            // (un interruptor), así que se acepta la reserva de memoria.
            self.resize_delay();
            self.reset();
        }
        true
    }

    fn latency(&self) -> usize {
        Limiter::latency(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loudness::true_peak;

    fn noise_with_transients(n: usize) -> Vec<f32> {
        let mut seed = 1u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0
        };
        (0..n).map(|i| rnd() * if i % 9000 < 40 { 1.0 } else { 0.3 }).collect()
    }

    #[test]
    fn never_exceeds_ceiling() {
        let p = LimiterParams { input_db: 12.0, ceiling_db: -1.0, release_ms: 50.0, true_peak: false };
        let mut lim = Limiter::new(p, 48_000.0);
        let mut buf = noise_with_transients(96_000);
        lim.process(&mut buf);
        let ceiling = db_to_lin(-1.0);
        let max = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(max <= ceiling, "pico {max} > techo {ceiling}");
        assert!(max > ceiling * 0.95, "debe acercarse al techo: {max}");
    }

    #[test]
    fn true_peak_mode_keeps_intersample_peaks_under_ceiling() {
        // Senos agudos con fase variable: picos entre muestras muy por encima de las muestras.
        let sr = 48_000.0;
        let src: Vec<f32> = (0..96_000)
            .flat_map(|i| {
                let t = i as f32 / sr;
                // fs/4 con desfase de 45°: las muestras valen 0,707·A pero el pico real es A (+3 dB).
                let s = 0.7 * (std::f32::consts::TAU * 12_000.0 * t + std::f32::consts::FRAC_PI_4).sin()
                    + 0.3 * (std::f32::consts::TAU * 1_000.0 * t).sin();
                [s, s * 0.9]
            })
            .collect();
        let run = |tp: bool| {
            let mut lim = Limiter::new(LimiterParams { input_db: 6.0, ceiling_db: -1.0, release_ms: 50.0, true_peak: tp }, sr);
            let mut buf = src.clone();
            lim.process(&mut buf);
            20.0 * true_peak(&buf[9600..]).log10()
        };
        let sample_mode = run(false);
        let tp_mode = run(true);
        println!("pico real de salida: modo muestra {sample_mode:.2} dBTP, modo pico real {tp_mode:.2} dBTP");
        assert!(sample_mode > 0.0, "sin pico real se pasa del techo entre muestras: {sample_mode}");
        assert!(tp_mode <= -0.98 && tp_mode > -1.6, "con pico real queda en -1 dBTP: {tp_mode}");
    }

    #[test]
    fn transparent_below_ceiling_except_latency() {
        for tp in [false, true] {
            let mut lim = Limiter::new(LimiterParams { input_db: 0.0, ceiling_db: -1.0, release_ms: 50.0, true_peak: tp }, 48_000.0);
            let look = lim.latency();
            let src: Vec<f32> = (0..4000).map(|i| 0.5 * (i as f32 * 0.05).sin()).collect();
            let mut buf = src.clone();
            lim.process(&mut buf);
            for i in 0..(src.len() - look * 2) {
                assert!((buf[i + look * 2] - src[i]).abs() < 1e-6, "tp={tp}");
            }
        }
    }
}
