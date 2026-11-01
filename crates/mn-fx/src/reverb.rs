//! Reverb basada en Freeverb (Jezar at Dreampoint, dominio público): 8 filtros
//! peine con amortiguación y 4 pasa-todo por canal, más pre-retardo.

use crate::{EffectParams, Processor, ReverbParams};

pub(crate) const MAX_PREDELAY_MS: f32 = 200.0;

const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASSES: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
const FIXED_GAIN: f32 = 0.015;
const SCALE_ROOM: f32 = 0.28;
const OFFSET_ROOM: f32 = 0.7;
const SCALE_DAMP: f32 = 0.4;

struct Comb {
    buf: Vec<f32>,
    idx: usize,
    store: f32,
}

impl Comb {
    #[inline]
    fn tick(&mut self, x: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.buf[self.idx];
        self.store = out * (1.0 - damp) + self.store * damp;
        self.buf[self.idx] = x + self.store * feedback;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
}

struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}

impl Allpass {
    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let delayed = self.buf[self.idx];
        let out = delayed - x;
        self.buf[self.idx] = x + delayed * 0.5;
        self.idx = (self.idx + 1) % self.buf.len();
        out
    }
}

pub struct Reverb {
    sr: f32,
    p: ReverbParams,
    combs: [Vec<Comb>; 2],
    allpasses: [Vec<Allpass>; 2],
    predelay: Vec<f32>,
    pd_write: usize,
    pd_len: usize,
    feedback: f32,
    damp: f32,
    wet1: f32,
    wet2: f32,
}

impl Reverb {
    pub fn new(p: ReverbParams, sr: f32) -> Self {
        let scale = sr / 44_100.0;
        let len = |n: usize, ch: usize| (((n + ch * STEREO_SPREAD) as f32 * scale) as usize).max(1);
        let combs = std::array::from_fn(|ch| COMBS.iter().map(|&n| Comb { buf: vec![0.0; len(n, ch)], idx: 0, store: 0.0 }).collect());
        let allpasses = std::array::from_fn(|ch| ALLPASSES.iter().map(|&n| Allpass { buf: vec![0.0; len(n, ch)], idx: 0 }).collect());
        let predelay = vec![0.0; (MAX_PREDELAY_MS * 0.001 * sr) as usize * 2 + 2];
        let mut r = Self { sr, p, combs, allpasses, predelay, pd_write: 0, pd_len: 0, feedback: 0.0, damp: 0.0, wet1: 0.0, wet2: 0.0 };
        r.update();
        r
    }

    fn update(&mut self) {
        self.feedback = self.p.room_size * SCALE_ROOM + OFFSET_ROOM;
        self.damp = self.p.damping * SCALE_DAMP;
        // Ganancia de salida de Freeverb (wet = 3) repartida según la anchura.
        let wet = 3.0;
        self.wet1 = wet * (self.p.width / 2.0 + 0.5);
        self.wet2 = wet * ((1.0 - self.p.width) / 2.0);
        let frames = self.predelay.len() / 2;
        self.pd_len = ((self.p.predelay_ms * 0.001 * self.sr) as usize).min(frames - 1);
    }
}

impl Processor for Reverb {
    fn process(&mut self, buf: &mut [f32]) {
        let frames = self.predelay.len() / 2;
        let mix = self.p.mix;
        for frame in buf.chunks_exact_mut(2) {
            let (dl, dr) = (frame[0], frame[1]);
            // Pre-retardo sobre la entrada mono de la reverb.
            self.predelay[self.pd_write * 2] = dl;
            self.predelay[self.pd_write * 2 + 1] = dr;
            let read = (self.pd_write + frames - self.pd_len) % frames;
            let input = (self.predelay[read * 2] + self.predelay[read * 2 + 1]) * FIXED_GAIN;
            self.pd_write = (self.pd_write + 1) % frames;

            let mut out = [0.0f32; 2];
            for ch in 0..2 {
                let mut acc = 0.0;
                for c in &mut self.combs[ch] {
                    acc += c.tick(input, self.feedback, self.damp);
                }
                for a in &mut self.allpasses[ch] {
                    acc = a.tick(acc);
                }
                out[ch] = acc;
            }
            let wl = out[0] * self.wet1 + out[1] * self.wet2;
            let wr = out[1] * self.wet1 + out[0] * self.wet2;
            frame[0] = dl * (1.0 - mix) + wl * mix;
            frame[1] = dr * (1.0 - mix) + wr * mix;
        }
    }

    fn reset(&mut self) {
        for ch in 0..2 {
            for c in &mut self.combs[ch] {
                c.buf.fill(0.0);
                c.store = 0.0;
            }
            for a in &mut self.allpasses[ch] {
                a.buf.fill(0.0);
            }
        }
        self.predelay.fill(0.0);
    }

    fn set_params(&mut self, params: &EffectParams) -> bool {
        let EffectParams::Reverb(p) = params else { return false };
        self.p = *p;
        self.update();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_produces_decaying_tail_and_stays_stable() {
        let sr = 48_000.0;
        let p = ReverbParams { room_size: 0.8, damping: 0.4, width: 1.0, predelay_ms: 10.0, mix: 1.0 };
        let mut r = Reverb::new(p, sr);
        let n = 48_000 * 4;
        let mut buf = vec![0.0f32; n * 2];
        buf[0] = 1.0;
        buf[1] = 1.0;
        r.process(&mut buf);
        let energy = |a: usize, b: usize| buf[a * 2..b * 2].iter().map(|s| s * s).sum::<f32>();
        // Nada antes del pre-retardo (10 ms = 480 frames).
        assert_eq!(energy(0, 470), 0.0);
        let early = energy(480, 48_000);
        let late = energy(48_000 * 3, 48_000 * 4);
        assert!(early > 0.01, "{early}");
        assert!(late < early * 0.05, "la cola decae: {late} vs {early}");
        assert!(buf.iter().all(|s| s.is_finite() && s.abs() < 2.0));
        // Estéreo: los canales difieren (dispersión estéreo).
        assert!(buf[4800..9600].chunks(2).any(|f| (f[0] - f[1]).abs() > 1e-4));
    }

    #[test]
    fn dry_when_mix_zero() {
        let mut r = Reverb::new(ReverbParams { mix: 0.0, ..Default::default() }, 44_100.0);
        let mut buf: Vec<f32> = (0..2000).map(|i| (i as f32 * 0.01).sin()).collect();
        let orig = buf.clone();
        r.process(&mut buf);
        assert_eq!(buf, orig);
    }
}
