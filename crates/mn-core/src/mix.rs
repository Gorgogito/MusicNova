//! Reglas de mezcla compartidas por el motor en tiempo real y la exportación.

use mn_fx::{Chain, EffectSlot};

use crate::audio::AudioBuffer;
use crate::automation::ChannelAutomation;
use crate::project::MasterSettings;

/// Frames por bloque al procesar efectos offline (igual que el motor).
const BLOCK: usize = 512;

/// Ganancia lineal a partir de dB; -60 dB o menos se considera silencio.
pub fn db_to_gain(db: f32) -> f32 {
    if db <= -60.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Ley de balance: el centro mantiene ganancia unitaria en ambos canales.
pub fn balance_gains(gain: f32, pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    (gain * (1.0 - pan).min(1.0), gain * (1.0 + pan).min(1.0))
}

/// Clip en frames: un tramo del audio de la pista colocado en la línea de
/// tiempo, con fundidos y ganancia. Es la forma que usan el motor y la mezcla.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipFrames {
    /// Frame de la línea de tiempo en el que empieza.
    pub start: u64,
    /// Frame del audio de la pista en el que empieza.
    pub src_start: u64,
    pub len: u64,
    pub fade_in: u64,
    pub fade_out: u64,
    /// Ganancia lineal.
    pub gain: f32,
}

impl ClipFrames {
    /// Todo el audio de la pista desde `start`, sin fundidos.
    pub fn whole(start: u64, frames: u64) -> Self {
        Self { start, src_start: 0, len: frames, fade_in: 0, fade_out: 0, gain: 1.0 }
    }

    pub fn end(&self) -> u64 {
        self.start + self.len
    }

    /// Ganancia en el frame `i` del clip. Los fundidos siguen sen² (entrada) y
    /// cos² (salida): dos clips solapados con fundidos iguales suman 1.
    #[inline]
    pub fn envelope(&self, i: u64) -> f32 {
        let mut g = self.gain;
        if i < self.fade_in {
            let s = ((i as f32 + 0.5) / self.fade_in as f32 * std::f32::consts::FRAC_PI_2).sin();
            g *= s * s;
        }
        let rem = self.len.saturating_sub(i);
        if rem <= self.fade_out && self.fade_out > 0 {
            let s = ((rem as f32 - 0.5) / self.fade_out as f32 * std::f32::consts::FRAC_PI_2).sin();
            g *= s * s;
        }
        g
    }
}

/// Final de la pista en frames (el clip que termina más tarde).
pub fn clips_end(clips: &[ClipFrames]) -> u64 {
    clips.iter().map(|c| c.end()).max().unwrap_or(0)
}

/// Suma en `out` (estéreo entrelazado) el audio de los clips para cada
/// posición de `pos` (`u64::MAX` = silencio). Los clips solapados se suman.
/// No reserva memoria: apto para el hilo de audio.
pub fn render_clips(data: &[f32], clips: &[ClipFrames], pos: &[u64], out: &mut [f32]) {
    let (mut lo, mut hi) = (u64::MAX, 0u64);
    for &p in pos {
        if p != u64::MAX {
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    if lo > hi {
        return;
    }
    for c in clips {
        if c.end() <= lo || c.start > hi || c.len == 0 {
            continue;
        }
        for (f, &p) in pos.iter().enumerate() {
            if p < c.start || p >= c.end() {
                continue;
            }
            let i = p - c.start;
            let s = ((c.src_start + i) * 2) as usize;
            if s + 1 < data.len() {
                let g = c.envelope(i);
                out[f * 2] += data[s] * g;
                out[f * 2 + 1] += data[s + 1] * g;
            }
        }
    }
}

/// Renderiza los clips de una pista desde el frame 0 hasta el final del último.
pub fn render_track(data: &[f32], clips: &[ClipFrames]) -> Vec<f32> {
    let len = clips_end(clips) as usize;
    let mut out = vec![0.0f32; len * 2];
    let mut pos = vec![0u64; BLOCK];
    for (b, chunk) in out.chunks_mut(BLOCK * 2).enumerate() {
        let n = chunk.len() / 2;
        for (k, p) in pos[..n].iter_mut().enumerate() {
            *p = (b * BLOCK + k) as u64;
        }
        render_clips(data, clips, &pos[..n], chunk);
    }
    out
}

/// Una pista para la mezcla offline.
pub struct MixInput<'a> {
    pub data: &'a [f32],
    /// Clips de la pista (en frames a la frecuencia de la mezcla).
    pub clips: &'a [ClipFrames],
    pub gain_db: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub effects: &'a [EffectSlot],
    /// Automatización de la pista (a la frecuencia de la mezcla).
    pub automation: Option<&'a ChannelAutomation>,
}

/// Mezcla las pistas exactamente como suenan en el motor: silencio, solo,
/// efectos de cada pista, volumen, panorama, automatización y bus máster.
pub fn mixdown(
    inputs: &[MixInput],
    master: &MasterSettings,
    master_automation: Option<&ChannelAutomation>,
    sample_rate: u32,
) -> AudioBuffer {
    let len = inputs.iter().map(|t| clips_end(t.clips) as usize * 2).max().unwrap_or(0);
    let any_solo = inputs.iter().any(|t| t.solo);
    let mut data = vec![0.0f32; len];
    let mut block = vec![0.0f32; BLOCK * 2];
    let mut pos = vec![0u64; BLOCK];
    for t in inputs {
        if t.mute || (any_solo && !t.solo) {
            continue;
        }
        let (fader, pan) = (db_to_gain(t.gain_db), t.pan);
        let (gl, gr) = balance_gains(fader, t.pan);
        let mut chain = (!t.effects.is_empty()).then(|| Chain::new_offline(t.effects, sample_rate));
        // Compensación de latencia: la cadena se procesa `lat` frames más allá
        // del final y su salida se adelanta `lat` frames, así todas las pistas
        // quedan alineadas sin retrasar ninguna.
        let lat = chain.as_ref().map_or(0, |c| c.latency());
        let frames = len / 2;
        let total = frames + lat;
        let mut aligned = vec![0.0f32; len];
        let mut start = 0;
        while start < total {
            let n = BLOCK.min(total - start);
            block[..n * 2].fill(0.0);
            for (k, p) in pos[..n].iter_mut().enumerate() {
                *p = (start + k) as u64;
            }
            render_clips(t.data, t.clips, &pos[..n], &mut block[..n * 2]);
            if let Some(c) = chain.as_mut() {
                if let Some(a) = t.automation {
                    a.apply_effects(c, start as u64);
                }
                c.process(&mut block[..n * 2]);
            }
            // Frame procesado `start + k` → posición `start + k − lat`.
            let from = lat.saturating_sub(start);
            for k in from..n {
                let dst = start + k - lat;
                if dst >= frames {
                    break;
                }
                aligned[dst * 2] = block[k * 2];
                aligned[dst * 2 + 1] = block[k * 2 + 1];
            }
            start += n;
        }
        // Volumen, panorama y automatización sobre la señal ya alineada.
        for (start, out) in (0..len).step_by(BLOCK * 2).zip(data.chunks_mut(BLOCK * 2)) {
            let n = out.len();
            let (f0, f1) = ((start / 2) as u64, ((start + n) / 2) as u64);
            let src = &aligned[start..start + n];
            match t.automation.filter(|a| a.has_gain()) {
                Some(a) => mix_ramp(out, src, a.gains(f0, fader, pan), a.gains(f1, fader, pan)),
                None => mix_ramp(out, src, (gl, gr), (gl, gr)),
            }
        }
    }
    let mut chain = (!master.effects.is_empty()).then(|| Chain::new_offline(&master.effects, sample_rate));
    let g = db_to_gain(master.gain_db);
    if let Some(c) = chain.as_mut() {
        // Igual que en las pistas: se procesa la latencia del máster de más y se adelanta.
        let lat = c.latency();
        data.resize(len + lat * 2, 0.0);
        for (b, chunk) in data.chunks_mut(BLOCK * 2).enumerate() {
            if let Some(a) = master_automation {
                a.apply_effects(c, (b * BLOCK) as u64);
            }
            c.process(chunk);
        }
        data.drain(..lat * 2);
        data.truncate(len);
    }
    for (b, chunk) in data.chunks_mut(BLOCK * 2).enumerate() {
        let f0 = (b * BLOCK) as u64;
        let (g0, g1) = match master_automation {
            Some(a) => (a.master_gain(f0, g), a.master_gain(f0 + (chunk.len() / 2) as u64, g)),
            None => (g, g),
        };
        if g0 != 1.0 || g1 != 1.0 {
            ramp_in_place(chunk, g0, g1);
        }
    }
    AudioBuffer { sample_rate, data }
}

/// Suma `src` a `out` con ganancias que pasan linealmente de `from` a `to`.
pub fn mix_ramp(out: &mut [f32], src: &[f32], from: (f32, f32), to: (f32, f32)) {
    let n = (src.len() / 2).max(1) as f32;
    let (dl, dr) = ((to.0 - from.0) / n, (to.1 - from.1) / n);
    for (i, (o, s)) in out.chunks_exact_mut(2).zip(src.chunks_exact(2)).enumerate() {
        let k = i as f32;
        o[0] += s[0] * (from.0 + dl * k);
        o[1] += s[1] * (from.1 + dr * k);
    }
}

/// Multiplica en el sitio con una ganancia que pasa linealmente de `from` a `to`.
pub fn ramp_in_place(buf: &mut [f32], from: f32, to: f32) {
    let n = (buf.len() / 2).max(1) as f32;
    let d = (to - from) / n;
    for (i, f) in buf.chunks_exact_mut(2).enumerate() {
        let g = from + d * i as f32;
        f[0] *= g;
        f[1] *= g;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mn_fx::{EffectParams, LimiterParams};

    fn whole(data: &[f32]) -> Vec<ClipFrames> {
        vec![ClipFrames::whole(0, (data.len() / 2) as u64)]
    }

    fn input<'a>(data: &'a [f32], clips: &'a [ClipFrames], gain_db: f32, pan: f32, solo: bool) -> MixInput<'a> {
        MixInput { data, clips, gain_db, pan, mute: false, solo, effects: &[], automation: None }
    }

    #[test]
    fn mixdown_respects_solo_and_pan() {
        let a = vec![0.5f32; 8];
        let b = vec![0.25f32; 4];
        let (ca, cb) = (whole(&a), whole(&b));
        let m = mixdown(&[input(&a, &ca, 0.0, 1.0, false), input(&b, &cb, 0.0, 0.0, false)], &MasterSettings::default(), None, 10);
        assert_eq!(m.data, vec![0.25, 0.75, 0.25, 0.75, 0.0, 0.5, 0.0, 0.5]);

        let m = mixdown(&[input(&a, &ca, 0.0, 0.0, false), input(&b, &cb, -60.0, 0.0, true)], &MasterSettings::default(), None, 10);
        assert!(m.data.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn automation_ramps_volume_and_master() {
        use crate::automation::{AutoPoint, AutomationLane, AutomationTarget, ChannelAutomation};
        let lane = |points: Vec<(f64, f32)>| {
            vec![AutomationLane {
                id: uuid::Uuid::new_v4(),
                target: AutomationTarget::Volume,
                enabled: true,
                points: points.into_iter().map(|(time, value)| AutoPoint { time, value }).collect(),
            }]
        };
        let rate = 48_000;
        let data = vec![0.5f32; 2 * 2 * 48_000];
        let clips = whole(&data);
        // Pista: de -60 dB (silencio) a 0 dB entre 0,5 y 1,5 s.
        let auto = ChannelAutomation::build(&lane(vec![(0.5, -60.0), (1.5, 0.0)]), &[], rate).unwrap();
        let mut t = input(&data, &clips, 0.0, 0.0, false);
        t.automation = Some(&auto);
        let m = mixdown(&[t], &MasterSettings::default(), None, rate);
        let l = |s: f64| m.data[(s * rate as f64) as usize * 2];
        assert_eq!(l(0.2), 0.0);
        assert!((l(1.9) - 0.5).abs() < 1e-6);
        assert!(l(1.0) > 0.0 && l(1.0) < 0.5, "sube de forma gradual: {}", l(1.0));
        // Sin escalones: la diferencia entre muestras consecutivas es pequeña.
        let max_step = m.data.chunks_exact(2).map(|f| f[0]).collect::<Vec<_>>().windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(max_step < 0.01, "{max_step}");

        // Máster: -6 dB fijos por automatización (el fader a 0 dB no cuenta).
        let master_auto = ChannelAutomation::build(&lane(vec![(0.0, -6.0)]), &[], rate).unwrap();
        let m = mixdown(&[input(&data, &clips, 0.0, 0.0, false)], &MasterSettings::default(), Some(&master_auto), rate);
        assert!(m.data.iter().all(|s| (s - 0.5 * 0.501_187).abs() < 1e-4));
    }

    #[test]
    fn clips_trim_move_fade_and_overlap() {
        // Audio de la pista: el frame n vale n (en ambos canales).
        let data: Vec<f32> = (0..100).flat_map(|n| [n as f32, n as f32]).collect();
        // Tramo 10..20 del audio colocado en el frame 5; tramo 50..53 en el 30.
        let clips = [
            ClipFrames { start: 5, src_start: 10, len: 10, fade_in: 0, fade_out: 0, gain: 1.0 },
            ClipFrames { start: 30, src_start: 50, len: 3, fade_in: 0, fade_out: 0, gain: 0.5 },
        ];
        let out = render_track(&data, &clips);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(left.len(), 33);
        assert!(left[..5].iter().all(|&s| s == 0.0));
        assert_eq!(&left[5..15], &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 19.0]);
        assert!(left[15..30].iter().all(|&s| s == 0.0));
        assert_eq!(&left[30..33], &[25.0, 25.5, 26.0]);

        // Fundidos: suben desde casi 0 y bajan hasta casi 0; el centro queda intacto.
        let c = ClipFrames { start: 0, src_start: 0, len: 1000, fade_in: 100, fade_out: 200, gain: 1.0 };
        assert!(c.envelope(0) < 0.001 && c.envelope(999) < 0.001);
        assert_eq!(c.envelope(500), 1.0);
        assert!((c.envelope(50) - 0.5).abs() < 0.02);
        // Fundido cruzado: salida de un clip + entrada del siguiente = 1.
        let a = ClipFrames { start: 0, src_start: 0, len: 300, fade_in: 0, fade_out: 100, gain: 1.0 };
        let b = ClipFrames { start: 200, src_start: 0, len: 300, fade_in: 100, fade_out: 0, gain: 1.0 };
        for t in 200..300u64 {
            let sum = a.envelope(t - a.start) + b.envelope(t - b.start);
            assert!((sum - 1.0).abs() < 1e-4, "{t}: {sum}");
        }
        // Posiciones con silencio (u64::MAX) y fuera del audio no suman nada.
        let mut out = vec![0.0f32; 6];
        render_clips(&data, &[ClipFrames { start: 0, src_start: 98, len: 10, fade_in: 0, fade_out: 0, gain: 1.0 }], &[0, u64::MAX, 5], &mut out);
        assert_eq!(out, vec![98.0, 98.0, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn mixdown_places_offset_tracks() {
        let a = vec![0.5f32; 4];
        let b = vec![0.25f32; 4];
        let ca = whole(&a);
        let cb = vec![ClipFrames::whole(3, 2)];
        let m = mixdown(&[input(&a, &ca, 0.0, 0.0, false), input(&b, &cb, 0.0, 0.0, false)], &MasterSettings::default(), None, 10);
        assert_eq!(m.data, vec![0.5, 0.5, 0.5, 0.5, 0.0, 0.0, 0.25, 0.25, 0.25, 0.25]);
    }

    #[test]
    fn mixdown_applies_track_and_master_effects() {
        let sr = 48_000;
        let sine: Vec<f32> = (0..sr as usize)
            .flat_map(|i| {
                let s = 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / sr as f32).sin();
                [s, s]
            })
            .collect();
        let lim = |ceiling_db| {
            vec![EffectSlot { plugin: None,
                id: "l".into(),
                enabled: true,
                params: EffectParams::Limiter(LimiterParams { input_db: 0.0, ceiling_db, release_ms: 50.0, true_peak: false }),
            }]
        };
        let track_fx = lim(-12.0);
        let peak = |b: &AudioBuffer| b.data.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let cs = whole(&sine);
        let t = MixInput { data: &sine, clips: &cs, gain_db: 0.0, pan: 0.0, mute: false, solo: false, effects: &track_fx, automation: None };
        let m = mixdown(&[t], &MasterSettings::default(), None, sr);
        assert!((0.24..=0.2512).contains(&peak(&m)), "{}", peak(&m));

        let master = MasterSettings { gain_db: -6.0, effects: lim(-3.0), ..Default::default() };
        let m = mixdown(&[input(&sine, &cs, 0.0, 0.0, false)], &master, None, sr);
        // Limitado a -3 dBFS (sin efecto: el pico es -6) y después -6 dB de ganancia.
        assert!((peak(&m) - 0.25).abs() < 0.01, "{}", peak(&m));
    }
}
