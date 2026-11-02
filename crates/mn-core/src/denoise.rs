//! Reducción de ruido constante (zumbido, siseo, ventilador…) para cualquier
//! pista: sustracción espectral con ganancia de Wiener y SNR a priori
//! «dirigida por decisión» (Ephraim y Malah, 1984), que evita el ruido musical.
//!
//! El perfil de ruido sale de un tramo con solo ruido si se indica; si no, de
//! las tramas más silenciosas de la pista (la cuenta previa, las pausas entre
//! frases). Un zumbido y una nota sostenida son igual de estables, así que sin
//! silencios ni tramo de referencia no se pueden distinguir.

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};
use serde::{Deserialize, Serialize};

use crate::audio::AudioBuffer;

const FFT: usize = 2048;
const HOP: usize = FFT / 4;
/// Peso de la SNR a priori de la trama anterior (Ephraim–Malah).
const DD_ALPHA: f32 = 0.98;
/// Fracción de tramas más silenciosas que forman el perfil sin tramo de referencia.
const QUIET_FRACTION: f32 = 0.05;
const QUIET_MIN_FRAMES: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DenoiseSettings {
    /// Atenuación máxima del ruido (dB): el suelo de la ganancia.
    pub reduction_db: f32,
    /// 0–1: cuánto ruido se supone que hay (0,5 = el estimado). Más alto quita
    /// más ruido y también algo más de señal débil.
    pub sensitivity: f32,
    /// Tramo (s) que solo contiene ruido, para tomar de él el perfil.
    #[serde(default)]
    pub noise_region: Option<(f64, f64)>,
}

impl Default for DenoiseSettings {
    fn default() -> Self {
        // Sensibilidad 0,3 (ruido estimado × 0,57): la mejor SNR medida con
        // instrumentos reales de IRMAS con siseo añadido (+7,8 dB a 10 dB de SNR).
        Self { reduction_db: 18.0, sensitivity: 0.3, noise_region: None }
    }
}

/// Ventana de Hann periódica (raíz) para análisis y síntesis.
fn sqrt_hann() -> Vec<f32> {
    (0..FFT).map(|i| (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT as f32).cos()).sqrt()).collect()
}

/// Espectros (tramas × bins) de un canal, con relleno de FFT/2 a cada lado.
fn stft(x: &[f32], fft: &Arc<dyn RealToComplex<f32>>, win: &[f32]) -> Vec<Vec<realfft::num_complex::Complex32>> {
    let pad = FFT / 2;
    let n = x.len() + 2 * pad;
    let frames = n.saturating_sub(FFT) / HOP + 1;
    let mut buf = fft.make_input_vec();
    let mut out = Vec::with_capacity(frames);
    for t in 0..frames {
        for (i, b) in buf.iter_mut().enumerate() {
            let j = (t * HOP + i) as isize - pad as isize;
            *b = if j >= 0 && (j as usize) < x.len() { x[j as usize] * win[i] } else { 0.0 };
        }
        let mut spec = fft.make_output_vec();
        fft.process(&mut buf, &mut spec).expect("FFT");
        out.push(spec);
    }
    out
}

/// Potencia de ruido por bin: media de las tramas del tramo indicado o de las
/// más silenciosas (sin contar las de silencio digital).
fn noise_psd(power: &[Vec<f32>], region: Option<(usize, usize)>) -> Vec<f32> {
    let bins = power.first().map_or(0, Vec::len);
    let frames: Vec<usize> = match region.filter(|(a, b)| b > a && *a < power.len()) {
        Some((a, b)) => (a..b.min(power.len())).collect(),
        None => {
            let energy: Vec<f32> = power.iter().map(|p| p.iter().sum()).collect();
            let mut order: Vec<usize> = (0..power.len()).filter(|&t| energy[t] > 1e-12).collect();
            order.sort_by(|&a, &b| energy[a].total_cmp(&energy[b]));
            let n = ((order.len() as f32 * QUIET_FRACTION) as usize).max(QUIET_MIN_FRAMES).min(order.len());
            order.truncate(n);
            order
        }
    };
    if frames.is_empty() {
        return vec![0.0; bins];
    }
    let n = frames.len() as f32;
    (0..bins).map(|k| frames.iter().map(|&t| power[t][k]).sum::<f32>() / n).collect()
}

/// Quita el ruido constante de audio estéreo entrelazado.
pub fn denoise(audio: &AudioBuffer, s: &DenoiseSettings) -> AudioBuffer {
    let frames = audio.frames();
    if frames == 0 {
        return audio.clone();
    }
    let mut planner = RealFftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(FFT);
    let inv = planner.plan_fft_inverse(FFT);
    let win = sqrt_hann();
    // Suma de ventana² con solape del 75 %: Hann con salto N/4 suma 2.
    let ola_norm = 1.0 / 2.0;
    let floor = 10f32.powf(-s.reduction_db.abs() / 20.0);
    // Sensibilidad 0–1 → el ruido estimado se multiplica por 0,5–4 (1 en 0,5).
    let over = 2f32.powf((s.sensitivity.clamp(0.0, 1.0) - 0.5) * 4.0);
    let region = s.noise_region.map(|(a, b)| {
        let to_frame = |t: f64| ((t * audio.sample_rate as f64 + (FFT / 2) as f64) / HOP as f64).max(0.0) as usize;
        (to_frame(a), to_frame(b))
    });

    let mut out = vec![0.0f32; audio.data.len()];
    for ch in 0..2 {
        let x: Vec<f32> = audio.data.iter().skip(ch).step_by(2).copied().collect();
        let mut spec = stft(&x, &fwd, &win);
        let power: Vec<Vec<f32>> = spec.iter().map(|f| f.iter().map(|c| c.norm_sqr()).collect()).collect();
        let noise: Vec<f32> = noise_psd(&power, region).into_iter().map(|n| n * over + 1e-12).collect();
        let bins = noise.len();
        let mut prev_clean = vec![0.0f32; bins];
        let mut gains = vec![0.0f32; bins];
        for (frame, p) in spec.iter_mut().zip(&power) {
            for k in 0..bins {
                let post = p[k] / noise[k];
                let prio = DD_ALPHA * prev_clean[k] / noise[k] + (1.0 - DD_ALPHA) * (post - 1.0).max(0.0);
                gains[k] = (prio / (1.0 + prio)).max(floor);
            }
            // Suavizado en frecuencia (3 bins) contra el ruido musical.
            let g: Vec<f32> = (0..bins)
                .map(|k| {
                    let a = gains[k.saturating_sub(1)];
                    let c = gains[(k + 1).min(bins - 1)];
                    (0.25 * a + 0.5 * gains[k] + 0.25 * c).max(floor)
                })
                .collect();
            for k in 0..bins {
                frame[k] *= g[k];
                prev_clean[k] = g[k] * g[k] * p[k];
            }
        }
        // Síntesis por solapamiento y suma.
        let pad = FFT / 2;
        let mut y = vec![0.0f32; x.len() + 2 * pad + FFT];
        let mut tbuf = inv.make_output_vec();
        for (t, frame) in spec.iter_mut().enumerate() {
            frame[0].im = 0.0;
            frame[bins - 1].im = 0.0;
            inv.process(frame, &mut tbuf).expect("IFFT");
            for i in 0..FFT {
                y[t * HOP + i] += tbuf[i] * win[i] * ola_norm / FFT as f32;
            }
        }
        for (i, v) in y[pad..pad + x.len()].iter().enumerate() {
            out[i * 2 + ch] = *v;
        }
    }
    AudioBuffer { sample_rate: audio.sample_rate, data: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn stereo(mono: &[f32]) -> AudioBuffer {
        AudioBuffer { sample_rate: RATE, data: mono.iter().flat_map(|&s| [s, s]).collect() }
    }

    fn left(b: &AudioBuffer) -> Vec<f32> {
        b.data.iter().step_by(2).copied().collect()
    }

    /// Ruido blanco reproducible.
    fn noise(n: usize, amp: f32) -> Vec<f32> {
        let mut x = 0x1234_5678u32;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                amp * ((x as f32 / u32::MAX as f32) * 2.0 - 1.0)
            })
            .collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    /// Tono con armónicos (como una nota de guitarra sostenida).
    fn tone(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                [(220.0, 0.3), (440.0, 0.15), (660.0, 0.08)]
                    .iter()
                    .map(|(f, a)| a * (std::f32::consts::TAU * f * t).sin())
                    .sum::<f32>()
            })
            .collect()
    }

    #[test]
    fn removes_steady_noise_and_keeps_the_tone() {
        // Medio segundo de solo ruido (como una cuenta previa) y luego la nota.
        let n = RATE as usize * 3;
        let clean: Vec<f32> =
            tone(n).into_iter().enumerate().map(|(i, v)| if i < RATE as usize / 2 { 0.0 } else { v }).collect();
        let hiss = noise(n, 0.05);
        let noisy: Vec<f32> = clean.iter().zip(&hiss).map(|(a, b)| a + b).collect();
        let out = left(&denoise(&stereo(&noisy), &DenoiseSettings::default()));
        let mid = RATE as usize..n - RATE as usize / 2;
        let snr = |y: &[f32]| {
            let err: Vec<f32> = y[mid.clone()].iter().zip(&clean[mid.clone()]).map(|(a, b)| a - b).collect();
            20.0 * (rms(&clean[mid.clone()]) / rms(&err)).log10()
        };
        let (before, after) = (snr(&noisy), snr(&out));
        assert!(after > before + 10.0, "SNR {before:.1} → {after:.1} dB");
        // El tono conserva su nivel (±1 dB).
        let level = 20.0 * (rms(&out[mid.clone()]) / rms(&clean[mid.clone()])).log10();
        assert!(level.abs() < 1.0, "{level:.2} dB");
    }

    #[test]
    fn pure_noise_is_attenuated_by_the_reduction_amount() {
        let n = RATE as usize * 2;
        let hiss = noise(n, 0.05);
        for db in [12.0, 24.0] {
            // Con el ruido estimado tal cual (0,5), el ruido solo baja hasta el suelo.
            let s = DenoiseSettings { reduction_db: db, sensitivity: 0.5, ..Default::default() };
            let out = left(&denoise(&stereo(&hiss), &s));
            let got = 20.0 * (rms(&hiss[4096..n - 4096]) / rms(&out[4096..n - 4096])).log10();
            assert!((got - db).abs() < 3.0, "pedido {db} dB, obtenido {got:.1} dB");
        }
    }

    #[test]
    fn noise_region_sets_the_profile() {
        // 1 s de solo ruido y luego tono con ruido: con el tramo como perfil.
        let n = RATE as usize * 3;
        let hiss = noise(n, 0.05);
        let t = tone(n);
        let noisy: Vec<f32> = (0..n).map(|i| hiss[i] + if i >= RATE as usize { t[i] } else { 0.0 }).collect();
        let s = DenoiseSettings { noise_region: Some((0.1, 0.9)), ..Default::default() };
        let out = left(&denoise(&stereo(&noisy), &s));
        let quiet = rms(&out[RATE as usize / 4..RATE as usize * 3 / 4]);
        assert!(20.0 * (rms(&hiss[..RATE as usize]) / quiet).log10() > 12.0);
        let loud =
            20.0 * (rms(&out[RATE as usize * 3 / 2..n - 4096]) / rms(&t[RATE as usize * 3 / 2..n - 4096])).log10();
        assert!(loud.abs() < 1.0, "{loud:.2} dB");
    }

    #[test]
    fn silence_and_empty_input() {
        assert_eq!(denoise(&stereo(&[]), &DenoiseSettings::default()).frames(), 0);
        let out = denoise(&stereo(&vec![0.0; 10_000]), &DenoiseSettings::default());
        assert!(out.data.iter().all(|v| v.abs() < 1e-6) && out.frames() == 10_000);
    }
}
