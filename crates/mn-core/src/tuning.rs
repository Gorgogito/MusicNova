//! Corrección de afinación de una voz a partir de su curva de tono.
//!
//! 1. Para cada punto con voz se calcula su centro local (mediana de ±200 ms
//!    entre los vecinos de la misma nota) y se asigna a la nota permitida más
//!    cercana a ese centro, con histéresis.
//! 2. La transposición de cada punto es `fuerza · [(nota − centro) + velocidad · (centro − tono)]`:
//!    con velocidad 0 se centra cada nota y se conserva la expresión (vibrato,
//!    ligados); con velocidad 1 la voz queda clavada en la nota.
//! 3. Se suaviza, se mantiene en los huecos cortos sin voz y se aplica con
//!    Signalsmith Stretch conservando los formantes.

use mn_stretch::Stretch;
use serde::{Deserialize, Serialize};

use crate::audio::AudioBuffer;
use crate::transcription::PitchCurve;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TuneSettings {
    /// 0–1: cuánto se corrige.
    pub strength: f32,
    /// 0–1: 0 conserva la expresión y solo centra cada nota; 1 la clava.
    pub speed: f32,
    /// Clases de altura permitidas (0 = Do … 11 = Si); vacío = cromática.
    #[serde(default)]
    pub scale: Vec<u8>,
    #[serde(default = "yes")]
    pub preserve_formants: bool,
}

fn yes() -> bool {
    true
}

impl Default for TuneSettings {
    fn default() -> Self {
        Self { strength: 1.0, speed: 0.3, scale: Vec::new(), preserve_formants: true }
    }
}

/// Probabilidad de voz mínima para tener en cuenta un punto.
const MIN_CONFIDENCE: f32 = 0.3;
/// Histéresis: hay que alejarse tanto (semitonos) del centro de la nota actual para cambiar de nota.
const HYSTERESIS: f32 = 0.65;
/// Tramos con voz más cortos que esto (puntos) no se corrigen.
const MIN_RUN: usize = 4;
/// Radio (puntos) del centro local: ±200 ms, más de un periodo de vibrato.
const CENTER_RADIUS: usize = 20;
/// Vecinos más alejados que esto (semitonos) se consideran otra nota.
const SAME_NOTE: f32 = 1.25;
/// Huecos sin voz (puntos) en los que se mantiene la transposición.
const HOLD_GAP: usize = 15;

/// Nota permitida más cercana a `p` (nota MIDI continua).
pub fn nearest_allowed(p: f32, scale: &[u8]) -> f32 {
    let base = p.round();
    if scale.is_empty() {
        return base;
    }
    let mut best = base;
    let mut dist = f32::MAX;
    for d in -6i32..=6 {
        let n = base + d as f32;
        let pc = (n as i32).rem_euclid(12) as u8;
        if scale.contains(&pc) && (n - p).abs() < dist {
            dist = (n - p).abs();
            best = n;
        }
    }
    best
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// Tono (nota MIDI continua) por punto, sin los puntos dudosos y con un filtro
/// de mediana de 5 puntos que quita saltos de octava aislados.
fn clean_pitch(curve: &PitchCurve) -> Vec<Option<f32>> {
    let raw: Vec<Option<f32>> = curve
        .midi()
        .enumerate()
        .map(|(i, p)| p.filter(|_| curve.confidence.get(i).is_none_or(|&c| c >= MIN_CONFIDENCE)))
        .collect();
    (0..raw.len())
        .map(|i| {
            raw[i]?;
            let mut w: Vec<f32> = raw[i.saturating_sub(2)..(i + 3).min(raw.len())].iter().flatten().copied().collect();
            Some(median(&mut w))
        })
        .collect()
}

/// Transposición (semitonos) para cada punto de la curva.
pub fn plan_shifts(curve: &PitchCurve, s: &TuneSettings) -> Vec<f32> {
    let strength = s.strength.clamp(0.0, 1.0);
    let speed = s.speed.clamp(0.0, 1.0);
    let pitch = clean_pitch(curve);
    let n = pitch.len();

    // Tramos seguidos con voz.
    let mut runs = Vec::new();
    let mut i = 0;
    while i < n {
        if pitch[i].is_none() {
            i += 1;
            continue;
        }
        let a = i;
        while i < n && pitch[i].is_some() {
            i += 1;
        }
        runs.push((a, i));
    }

    let mut shift = vec![f32::NAN; n];
    for (a, b) in runs {
        if b - a < MIN_RUN {
            shift[a..b].iter_mut().for_each(|v| *v = 0.0);
            continue;
        }
        let p: Vec<f32> = pitch[a..b].iter().map(|v| v.unwrap()).collect();
        // 1. Centro local: mediana de los vecinos de la misma nota (sigue la
        //    deriva lenta, no el vibrato ni los saltos de nota).
        let center: Vec<f32> = (0..p.len())
            .map(|k| {
                let lo = k.saturating_sub(CENTER_RADIUS);
                let hi = (k + CENTER_RADIUS + 1).min(p.len());
                let mut w: Vec<f32> = p[lo..hi].iter().copied().filter(|x| (x - p[k]).abs() <= SAME_NOTE).collect();
                median(&mut w)
            })
            .collect();
        // 2. Nota objetivo del centro, con histéresis.
        let mut current: Option<f32> = None;
        for k in 0..p.len() {
            let c = center[k];
            let t = match current {
                Some(t) if (c - t).abs() < HYSTERESIS => t,
                _ => nearest_allowed(c, &s.scale),
            };
            current = Some(t);
            shift[a + k] = strength * ((t - c) + speed * (c - p[k]));
        }
    }

    // 3. Huecos: los cortos se interpolan entre sus extremos; en los largos se
    //    vuelve a 0 en rampa de `HOLD_GAP` puntos.
    let mut out = vec![0.0f32; n];
    let mut k = 0;
    while k < n {
        if !shift[k].is_nan() {
            out[k] = shift[k];
            k += 1;
            continue;
        }
        let a = k;
        while k < n && shift[k].is_nan() {
            k += 1;
        }
        let left = if a > 0 { shift[a - 1] } else { 0.0 };
        let right = if k < n { shift[k] } else { 0.0 };
        let len = k - a;
        for (q, v) in out[a..k].iter_mut().enumerate() {
            let x = (q + 1) as f32 / (len + 1) as f32;
            *v = if len <= HOLD_GAP {
                left + (right - left) * x
            } else {
                let from_left = left * (1.0 - (q + 1) as f32 / HOLD_GAP as f32).max(0.0);
                let from_right = right * (1.0 - (len - q) as f32 / HOLD_GAP as f32).max(0.0);
                from_left + from_right
            };
        }
    }

    // 4. Suavizado (media de 3 puntos) para que los cambios no chasqueen.
    (0..n)
        .map(|i| {
            let w = &out[i.saturating_sub(1)..(i + 2).min(n)];
            w.iter().sum::<f32>() / w.len() as f32
        })
        .collect()
}

/// Bloque de proceso (frames): resolución temporal con la que cambia la transposición.
const BLOCK: usize = 64;

/// Aplica la transposición variable a audio estéreo alineado con la curva
/// (el punto `k` de `shifts` corresponde al instante `k · hop`).
pub fn apply_shifts(audio: &AudioBuffer, shifts: &[f32], hop: f64, preserve_formants: bool) -> AudioBuffer {
    let rate = audio.sample_rate;
    let frames = audio.frames();
    let mut st = Stretch::new(rate);
    st.set_preserve_formants(preserve_formants);
    let (in_lat, out_lat) = (st.input_latency(), st.output_latency());
    let total = in_lat + out_lat;
    let shift_at = |t: f64| -> f32 {
        if t < 0.0 || shifts.is_empty() {
            return 0.0;
        }
        let x = t / hop;
        let i = x.floor() as usize;
        if i + 1 >= shifts.len() {
            return *shifts.last().unwrap_or(&0.0);
        }
        let f = (x - i as f64) as f32;
        shifts[i] * (1.0 - f) + shifts[i + 1] * f
    };

    let mut out = vec![0.0f32; (frames + total) * 2];
    let zeros = vec![0.0f32; BLOCK * 2];
    let mut pos = 0;
    while pos < frames + total {
        let n = BLOCK.min(frames + total - pos);
        // Lo que se sintetiza ahora corresponde a la entrada de hace `in_lat` frames.
        let t = (pos as f64 + n as f64 / 2.0 - in_lat as f64) / rate as f64;
        st.set_transpose_semitones(shift_at(t));
        let input = if pos < frames {
            let end = (pos + n).min(frames);
            let mut v = audio.data[pos * 2..end * 2].to_vec();
            v.resize(n * 2, 0.0);
            v
        } else {
            zeros[..n * 2].to_vec()
        };
        st.process(&input, &mut out[pos * 2..(pos + n) * 2]);
        pos += n;
    }
    AudioBuffer { sample_rate: rate, data: out[total * 2..].to_vec() }
}

/// Curva + ajustes → voz afinada.
pub fn tune(audio: &AudioBuffer, curve: &PitchCurve, settings: &TuneSettings) -> AudioBuffer {
    let shifts = plan_shifts(curve, settings);
    apply_shifts(audio, &shifts, curve.hop, settings.preserve_formants)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const HOP: f64 = 0.01;

    /// Seno estéreo con frecuencia variable `f(t)` y la curva de tono exacta.
    fn voice(seconds: f64, f: impl Fn(f64) -> f64) -> (AudioBuffer, PitchCurve) {
        let frames = (seconds * RATE as f64) as usize;
        let mut phase = 0.0f64;
        let mut data = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let t = i as f64 / RATE as f64;
            phase += std::f64::consts::TAU * f(t) / RATE as f64;
            let s = (0.4 * phase.sin()) as f32;
            data.extend([s, s]);
        }
        let points = (seconds / HOP) as usize;
        let curve = PitchCurve {
            version: 1,
            track_id: None,
            source: "test".into(),
            hop: HOP,
            f0: (0..points).map(|k| f(k as f64 * HOP) as f32).collect(),
            confidence: vec![1.0; points],
        };
        (AudioBuffer { sample_rate: RATE, data }, curve)
    }

    /// Frecuencia media por cruces por cero ascendentes entre `a` y `b` segundos.
    fn freq(buf: &AudioBuffer, a: f64, b: f64) -> f64 {
        let left: Vec<f32> = buf.data.chunks_exact(2).map(|f| f[0]).collect();
        let seg = &left[(a * RATE as f64) as usize..(b * RATE as f64) as usize];
        let ups: Vec<usize> = seg.windows(2).enumerate().filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0).map(|(i, _)| i).collect();
        (ups.len() - 1) as f64 / ((ups[ups.len() - 1] - ups[0]) as f64 / RATE as f64)
    }

    fn cents(f: f64, reference: f64) -> f64 {
        1200.0 * (f / reference).log2()
    }

    #[test]
    fn nearest_note_respects_the_scale() {
        let c_major = [0, 2, 4, 5, 7, 9, 11];
        assert_eq!(nearest_allowed(61.2, &[]), 61.0);
        assert_eq!(nearest_allowed(60.8, &c_major), 60.0, "Do# no está en Do mayor: Do es el más cercano");
        assert_eq!(nearest_allowed(61.2, &c_major), 62.0);
        assert_eq!(nearest_allowed(69.8, &c_major), 69.0, "La#→La");
    }

    #[test]
    fn flat_and_sharp_notes_are_centered() {
        // +40 cents durante 2 s y −40 cents otros 2 s respecto a La 440.
        let (audio, curve) = voice(4.0, |t| if t < 2.0 { 440.0 * 2f64.powf(0.4 / 12.0) } else { 440.0 * 2f64.powf(-0.4 / 12.0) });
        let out = tune(&audio, &curve, &TuneSettings { strength: 1.0, speed: 0.0, ..Default::default() });
        assert_eq!(out.frames(), audio.frames(), "misma duración");
        let a = cents(freq(&out, 0.4, 1.8), 440.0);
        let b = cents(freq(&out, 2.3, 3.7), 440.0);
        assert!(a.abs() < 5.0 && b.abs() < 5.0, "{a:.1} y {b:.1} cents");
        // Sin corregir sigue igual.
        let same = tune(&audio, &curve, &TuneSettings { strength: 0.0, ..Default::default() });
        assert!((cents(freq(&same, 0.4, 1.8), 440.0) - 40.0).abs() < 3.0);
    }

    #[test]
    fn speed_zero_keeps_vibrato_and_speed_one_flattens_it() {
        // Centro +30 cents con vibrato de ±50 cents a 5 Hz.
        let f = |t: f64| 440.0 * 2f64.powf((0.3 + 0.5 * (std::f64::consts::TAU * 5.0 * t).sin()) / 12.0);
        let (audio, curve) = voice(4.0, f);
        let natural = tune(&audio, &curve, &TuneSettings { strength: 1.0, speed: 0.0, ..Default::default() });
        let robot = tune(&audio, &curve, &TuneSettings { strength: 1.0, speed: 1.0, ..Default::default() });
        // Media sobre periodos completos del vibrato: centrada en La.
        let mean = cents(freq(&natural, 1.0, 3.0), 440.0);
        assert!(mean.abs() < 8.0, "media {mean:.1} cents");
        // Profundidad del vibrato: alrededor de un pico (t = 1,05 s) frente a un valle (1,15 s).
        let peak = cents(freq(&natural, 1.03, 1.07), 440.0);
        let valley = cents(freq(&natural, 1.13, 1.17), 440.0);
        assert!(peak - valley > 60.0, "el vibrato se conserva: {peak:.0} / {valley:.0}");
        let rp = cents(freq(&robot, 1.03, 1.07), 440.0);
        let rv = cents(freq(&robot, 1.13, 1.17), 440.0);
        assert!((rp - rv).abs() < 25.0 && rp.abs() < 20.0, "clavado en La: {rp:.0} / {rv:.0}");
    }

    #[test]
    fn short_gaps_hold_and_long_gaps_release() {
        let mut curve = PitchCurve { version: 1, track_id: None, source: "t".into(), hop: HOP, f0: vec![], confidence: vec![] };
        let sharp = 440.0 * 2f32.powf(0.4 / 12.0);
        // 50 puntos con voz, 10 sin voz, 50 con voz, 100 sin voz.
        curve.f0 = [vec![sharp; 50], vec![0.0; 10], vec![sharp; 50], vec![0.0; 100]].concat();
        curve.confidence = vec![1.0; curve.f0.len()];
        let s = plan_shifts(&curve, &TuneSettings { strength: 1.0, speed: 0.0, ..Default::default() });
        assert!((s[25] + 0.4).abs() < 0.01);
        assert!((s[55] + 0.4).abs() < 0.01, "hueco corto: se mantiene");
        assert!(s[200].abs() < 1e-6, "hueco largo: vuelve a 0");
    }

    /// Vocal sintética: armónicos de `f0` con la envolvente de una /a/
    /// (formantes en 700 y 1200 Hz).
    fn vowel(f0: f32, secs: f32) -> AudioBuffer {
        let n = (secs * RATE as f32) as usize;
        let env = |f: f32| {
            let peak = |c: f32, w: f32| (-((f - c) / w).powi(2)).exp();
            peak(700.0, 150.0) + 0.7 * peak(1200.0, 200.0) + 0.02
        };
        let mut mono = vec![0.0f32; n];
        let mut h = 1;
        while f0 * h as f32 <= 5000.0 {
            let (f, a) = (f0 * h as f32, env(f0 * h as f32));
            for (i, s) in mono.iter_mut().enumerate() {
                *s += 0.1 * a * (std::f32::consts::TAU * f * i as f32 / RATE as f32).sin();
            }
            h += 1;
        }
        AudioBuffer { sample_rate: RATE, data: mono.iter().flat_map(|&s| [s, s]).collect() }
    }

    /// Centroide espectral (Hz) entre 300 y 3000 Hz del tramo central (DFT directa).
    fn centroid(buf: &AudioBuffer) -> f32 {
        let left: Vec<f32> = buf.data.as_chunks::<2>().0.iter().map(|f| f[0]).collect();
        let n = 8192;
        let start = left.len() / 2 - n / 2;
        let x = &left[start..start + n];
        let (mut num, mut den) = (0.0f64, 0.0f64);
        let mut f = 300.0f64;
        while f <= 3000.0 {
            let w = std::f64::consts::TAU * f / RATE as f64;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, &v) in x.iter().enumerate() {
                let hann = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
                re += v as f64 * hann * (w * i as f64).cos();
                im += v as f64 * hann * (w * i as f64).sin();
            }
            let p = re * re + im * im;
            num += f * p;
            den += p;
            f += 10.0;
        }
        (num / den) as f32
    }

    #[test]
    fn preserving_formants_keeps_the_vowel() {
        let voice = vowel(150.0, 1.0);
        let shifts = vec![5.0; 102];
        let c0 = centroid(&voice);
        let kept = centroid(&apply_shifts(&voice, &shifts, 0.01, true));
        let moved = centroid(&apply_shifts(&voice, &shifts, 0.01, false));
        // +5 semitonos = ×1,335. Sin conservarlos, los formantes suben con la nota;
        // conservándolos, el centroide apenas se mueve.
        assert!((moved / c0 - 1.335).abs() < 0.08, "{c0} → {moved}");
        assert!((kept / c0 - 1.0).abs() < 0.08, "{c0} → {kept}");
    }
}
