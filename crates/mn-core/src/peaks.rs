//! Resumen de picos para dibujar formas de onda.

use crate::audio::AudioBuffer;

/// Picos mono (mín./máx. alternados) de un buffer, un par por bloque.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Peaks {
    /// Frames de audio que representa cada par.
    pub frames_per_peak: usize,
    /// `[min0, max0, min1, max1, ...]`
    pub data: Vec<f32>,
}

pub fn compute_peaks(buf: &AudioBuffer, frames_per_peak: usize) -> Peaks {
    let frames_per_peak = frames_per_peak.max(1);
    let mut data = Vec::with_capacity(buf.frames() / frames_per_peak * 2 + 2);
    for block in buf.data.chunks(frames_per_peak * 2) {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for f in block.chunks_exact(2) {
            let m = 0.5 * (f[0] + f[1]);
            lo = lo.min(m);
            hi = hi.max(m);
        }
        if lo > hi {
            lo = 0.0;
            hi = 0.0;
        }
        data.push(lo);
        data.push(hi);
    }
    Peaks { frames_per_peak, data }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_bounds() {
        let buf = AudioBuffer { sample_rate: 10, data: vec![0.1, 0.1, -0.5, -0.5, 0.9, 0.9, 0.0, 0.0, 0.2, 0.2] };
        let p = compute_peaks(&buf, 2);
        assert_eq!(p.data, vec![-0.5, 0.1, 0.0, 0.9, 0.2, 0.2]);
    }
}
