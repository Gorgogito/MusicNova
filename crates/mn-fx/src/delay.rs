//! Delay estéreo con realimentación filtrada y modo ping-pong. El tiempo de
//! retardo se suaviza y se lee con interpolación lineal, así que moverlo no
//! produce clics.

use crate::{time_coef, DelayParams, EffectParams, Processor};

pub(crate) const MAX_DELAY_MS: f32 = 2000.0;

pub struct Delay {
    sr: f32,
    p: DelayParams,
    buf: Vec<f32>,
    write: usize,
    /// Retardo actual (frames, fraccionario) y objetivo.
    current: f32,
    target: f32,
    smooth: f32,
    lp_coef: f32,
    lp: [f32; 2],
}

impl Delay {
    pub fn new(p: DelayParams, sr: f32) -> Self {
        let frames = (MAX_DELAY_MS * 0.001 * sr) as usize + 4;
        let mut d = Self {
            sr,
            p,
            buf: vec![0.0; frames * 2],
            write: 0,
            current: 0.0,
            target: 0.0,
            smooth: time_coef(50.0, sr),
            lp_coef: 0.0,
            lp: [0.0; 2],
        };
        d.update();
        d.current = d.target;
        d
    }

    fn update(&mut self) {
        let frames = (self.buf.len() / 2) as f32;
        self.target = (self.p.time_ms * 0.001 * self.sr).clamp(1.0, frames - 2.0);
        self.lp_coef = (-std::f32::consts::TAU * self.p.high_cut / self.sr).exp();
    }

    #[inline]
    fn read(&self, ch: usize, delay: f32) -> f32 {
        let frames = self.buf.len() / 2;
        let pos = self.write as f32 - delay + frames as f32;
        let i = pos.floor() as usize;
        let frac = pos - pos.floor();
        let a = self.buf[(i % frames) * 2 + ch];
        let b = self.buf[((i + 1) % frames) * 2 + ch];
        a + (b - a) * frac
    }
}

impl Processor for Delay {
    fn process(&mut self, buf: &mut [f32]) {
        let frames = self.buf.len() / 2;
        let (fb, mix) = (self.p.feedback, self.p.mix);
        for frame in buf.chunks_exact_mut(2) {
            self.current = self.target + self.smooth * (self.current - self.target);
            let mut wet = [self.read(0, self.current), self.read(1, self.current)];
            for (ch, w) in wet.iter_mut().enumerate() {
                // Paso bajo de un polo en la realimentación: cada repetición es más oscura.
                self.lp[ch] = *w + self.lp_coef * (self.lp[ch] - *w);
                *w = self.lp[ch];
            }
            let (inl, inr) = (frame[0], frame[1]);
            let (wl, wr) = if self.p.ping_pong {
                // La entrada (mono) entra por la izquierda y rebota entre canales.
                (0.5 * (inl + inr) + wet[1] * fb, wet[0] * fb)
            } else {
                (inl + wet[0] * fb, inr + wet[1] * fb)
            };
            self.buf[self.write * 2] = wl;
            self.buf[self.write * 2 + 1] = wr;
            self.write = (self.write + 1) % frames;
            frame[0] = inl * (1.0 - mix) + wet[0] * mix;
            frame[1] = inr * (1.0 - mix) + wet[1] * mix;
        }
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.lp = [0.0; 2];
        self.current = self.target;
    }

    fn set_params(&mut self, params: &EffectParams) -> bool {
        let EffectParams::Delay(p) = params else { return false };
        self.p = *p;
        self.update();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_peak(buf: &[f32], ch: usize, from: usize) -> usize {
        (from..buf.len() / 2).find(|&i| buf[i * 2 + ch].abs() > 0.05).unwrap()
    }

    #[test]
    fn echo_at_delay_time_and_decays() {
        let sr = 48_000.0;
        let p = DelayParams { time_ms: 100.0, feedback: 0.5, high_cut: 20_000.0, ping_pong: false, mix: 1.0 };
        let mut d = Delay::new(p, sr);
        let mut buf = vec![0.0f32; 48_000 * 2];
        buf[0] = 1.0;
        buf[1] = 1.0;
        d.process(&mut buf);
        let e1 = first_peak(&buf, 0, 0);
        assert!((4799..=4801).contains(&e1), "primer eco a 100 ms: {e1}");
        let e2 = first_peak(&buf, 0, e1 + 100);
        assert!((9598..=9602).contains(&e2), "segundo eco: {e2}");
        assert!(buf[e2 * 2].abs() < buf[e1 * 2].abs(), "se atenúa");
    }

    #[test]
    fn ping_pong_alternates_channels() {
        let p = DelayParams { time_ms: 50.0, feedback: 0.6, high_cut: 20_000.0, ping_pong: true, mix: 1.0 };
        let mut d = Delay::new(p, 48_000.0);
        let mut buf = vec![0.0f32; 48_000];
        buf[0] = 1.0;
        buf[1] = 1.0;
        d.process(&mut buf);
        let at = |f: usize| (buf[f * 2].abs(), buf[f * 2 + 1].abs());
        let (l1, r1) = at(2400);
        let (l2, r2) = at(4800);
        assert!(l1 > 0.5 && r1 < 1e-6, "primer eco a la izquierda: {l1} {r1}");
        assert!(r2 > 0.3 && l2 < 1e-6, "segundo a la derecha: {l2} {r2}");
    }
}
