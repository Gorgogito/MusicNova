//! Limitador de picos con anticipación (lookahead).
//!
//! La ganancia necesaria para cada muestra se pasa por un filtro de mínimo
//! sobre la ventana de anticipación y después por una media móvil de la misma
//! longitud: así la ganancia ya ha bajado del todo cuando llega el pico
//! (retrasado) y nunca se supera el techo. La recuperación es exponencial.

use crate::{db_to_lin, time_coef, EffectParams, LimiterParams, Processor};

const LOOKAHEAD_MS: f32 = 1.5;

pub struct Limiter {
    sr: f32,
    p: LimiterParams,
    input: f32,
    ceiling: f32,
    release: f32,
    look: usize,
    /// Señal retrasada (estéreo entrelazado), anillo de `look` frames.
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
            delay: vec![0.0; look * 2],
            pos: 0,
            deque: vec![(0, 1.0); look + 1],
            dq_head: 0,
            dq_len: 0,
            avg_buf: vec![1.0; look],
            avg_sum: look as f64,
            gain: 1.0,
            n: 0,
        };
        l.update();
        l
    }

    fn update(&mut self) {
        self.input = db_to_lin(self.p.input_db);
        // Pequeño margen por debajo del techo frente al redondeo de la media.
        self.ceiling = db_to_lin(self.p.ceiling_db) * 0.9999;
        self.release = time_coef(self.p.release_ms, self.sr);
    }

    /// Latencia en frames (la anticipación).
    pub fn latency(&self) -> usize {
        self.look
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
        // Retira por delante lo que salió de la ventana de `look + 1` muestras.
        while self.deque[self.dq_head].0 + (self.look as u64) < self.n {
            self.dq_head = (self.dq_head + 1) % cap;
            self.dq_len -= 1;
        }
        self.deque[self.dq_head].1
    }
}

impl Processor for Limiter {
    fn process(&mut self, buf: &mut [f32]) {
        for frame in buf.chunks_exact_mut(2) {
            let (l, r) = (frame[0] * self.input, frame[1] * self.input);
            let peak = l.abs().max(r.abs());
            let need = if peak > self.ceiling { self.ceiling / peak } else { 1.0 };
            let min = self.push_min(need);
            // Media móvil de `look` muestras del mínimo.
            let i = (self.n % self.look as u64) as usize;
            self.avg_sum += min as f64 - self.avg_buf[i] as f64;
            self.avg_buf[i] = min;
            let target = (self.avg_sum / self.look as f64) as f32;
            self.gain = if target < self.gain { target } else { target + self.release * (self.gain - target) };
            // Salida: la muestra de hace `look` frames con la ganancia actual.
            let j = self.pos * 2;
            let (dl, dr) = (self.delay[j], self.delay[j + 1]);
            self.delay[j] = l;
            self.delay[j + 1] = r;
            self.pos = (self.pos + 1) % self.look;
            frame[0] = dl * self.gain;
            frame[1] = dr * self.gain;
            self.n += 1;
        }
    }

    fn reset(&mut self) {
        self.delay.fill(0.0);
        self.dq_len = 0;
        self.avg_buf.fill(1.0);
        self.avg_sum = self.look as f64;
        self.gain = 1.0;
    }

    fn set_params(&mut self, params: &EffectParams) -> bool {
        let EffectParams::Limiter(p) = params else { return false };
        self.p = *p;
        self.update();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_exceeds_ceiling() {
        let p = LimiterParams { input_db: 12.0, ceiling_db: -1.0, release_ms: 50.0 };
        let mut lim = Limiter::new(p, 48_000.0);
        // Ruido con transitorios fuertes.
        let mut seed = 1u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32 * 2.0 - 1.0
        };
        let mut buf: Vec<f32> = (0..96_000).map(|i| rnd() * if i % 9000 < 40 { 1.0 } else { 0.3 }).collect();
        lim.process(&mut buf);
        let ceiling = db_to_lin(-1.0);
        let max = buf.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(max <= ceiling, "pico {max} > techo {ceiling}");
        assert!(max > ceiling * 0.95, "debe acercarse al techo: {max}");
    }

    #[test]
    fn transparent_below_ceiling_except_latency() {
        let mut lim = Limiter::new(LimiterParams { input_db: 0.0, ceiling_db: -1.0, release_ms: 50.0 }, 48_000.0);
        let look = lim.latency();
        let src: Vec<f32> = (0..4000).map(|i| 0.5 * (i as f32 * 0.05).sin()).collect();
        let mut buf = src.clone();
        lim.process(&mut buf);
        for i in 0..(src.len() - look * 2) {
            assert!((buf[i + look * 2] - src[i]).abs() < 1e-6);
        }
    }
}
