//! Efectos de audio de MusicNova y medición de sonoridad.
//!
//! Todos los procesadores trabajan en estéreo entrelazado, en el sitio, y no
//! reservan memoria al procesar: los buffers se crean en `build` (fuera del
//! hilo de audio). Así el mismo código sirve para el motor en tiempo real y
//! para la exportación, y lo que se oye es exactamente lo que se exporta.

pub mod biquad;
mod compressor;
mod delay;
mod eq;
mod limiter;
pub mod loudness;
mod reverb;

use serde::{Deserialize, Serialize};

pub use compressor::Compressor;
pub use delay::Delay;
pub use eq::Eq;
pub use limiter::Limiter;
pub use reverb::Reverb;

/// Una banda del ecualizador.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Band {
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqParams {
    /// Paso alto (Hz); 0 = desactivado.
    pub highpass: f32,
    pub low_shelf: Band,
    pub mid1: Band,
    pub mid2: Band,
    pub high_shelf: Band,
    /// Paso bajo (Hz); 0 = desactivado.
    pub lowpass: f32,
}

impl Default for EqParams {
    fn default() -> Self {
        Self {
            highpass: 0.0,
            low_shelf: Band { freq: 120.0, gain_db: 0.0, q: 0.707 },
            mid1: Band { freq: 500.0, gain_db: 0.0, q: 1.0 },
            mid2: Band { freq: 2500.0, gain_db: 0.0, q: 1.0 },
            high_shelf: Band { freq: 8000.0, gain_db: 0.0, q: 0.707 },
            lowpass: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompressorParams {
    pub threshold_db: f32,
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub knee_db: f32,
    pub makeup_db: f32,
}

impl Default for CompressorParams {
    fn default() -> Self {
        Self { threshold_db: -18.0, ratio: 3.0, attack_ms: 10.0, release_ms: 120.0, knee_db: 6.0, makeup_db: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReverbParams {
    /// Tamaño de la sala (0–1).
    pub room_size: f32,
    /// Amortiguación de agudos (0–1).
    pub damping: f32,
    /// Anchura estéreo (0–1).
    pub width: f32,
    pub predelay_ms: f32,
    /// Proporción de señal procesada (0–1).
    pub mix: f32,
}

impl Default for ReverbParams {
    fn default() -> Self {
        Self { room_size: 0.6, damping: 0.5, width: 1.0, predelay_ms: 20.0, mix: 0.25 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DelayParams {
    pub time_ms: f32,
    /// Realimentación (0–0,95).
    pub feedback: f32,
    /// Corte de agudos en la realimentación (Hz).
    pub high_cut: f32,
    pub ping_pong: bool,
    pub mix: f32,
}

impl Default for DelayParams {
    fn default() -> Self {
        Self { time_ms: 375.0, feedback: 0.35, high_cut: 6000.0, ping_pong: true, mix: 0.25 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LimiterParams {
    /// Ganancia de entrada (empuja la señal contra el techo).
    pub input_db: f32,
    pub ceiling_db: f32,
    pub release_ms: f32,
}

impl Default for LimiterParams {
    fn default() -> Self {
        Self { input_db: 0.0, ceiling_db: -1.0, release_ms: 80.0 }
    }
}

/// Parámetros de un efecto. Es `Copy` y sin memoria dinámica: puede enviarse
/// al hilo de audio para cambiar un parámetro sin reconstruir el efecto.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EffectParams {
    Eq(EqParams),
    Compressor(CompressorParams),
    Reverb(ReverbParams),
    Delay(DelayParams),
    Limiter(LimiterParams),
}

impl EffectParams {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Eq(_) => "eq",
            Self::Compressor(_) => "compressor",
            Self::Reverb(_) => "reverb",
            Self::Delay(_) => "delay",
            Self::Limiter(_) => "limiter",
        }
    }

    /// Limita cada parámetro a su rango válido.
    pub fn sanitized(self) -> Self {
        fn band(b: Band, lo: f32, hi: f32) -> Band {
            Band { freq: b.freq.clamp(lo, hi), gain_db: b.gain_db.clamp(-18.0, 18.0), q: b.q.clamp(0.1, 10.0) }
        }
        let off_or = |f: f32, lo: f32, hi: f32| if f <= 0.0 { 0.0 } else { f.clamp(lo, hi) };
        match self {
            Self::Eq(p) => Self::Eq(EqParams {
                highpass: off_or(p.highpass, 20.0, 1000.0),
                low_shelf: band(p.low_shelf, 20.0, 1000.0),
                mid1: band(p.mid1, 40.0, 16_000.0),
                mid2: band(p.mid2, 40.0, 16_000.0),
                high_shelf: band(p.high_shelf, 1000.0, 20_000.0),
                lowpass: off_or(p.lowpass, 1000.0, 20_000.0),
            }),
            Self::Compressor(p) => Self::Compressor(CompressorParams {
                threshold_db: p.threshold_db.clamp(-60.0, 0.0),
                ratio: p.ratio.clamp(1.0, 20.0),
                attack_ms: p.attack_ms.clamp(0.1, 200.0),
                release_ms: p.release_ms.clamp(5.0, 2000.0),
                knee_db: p.knee_db.clamp(0.0, 24.0),
                makeup_db: p.makeup_db.clamp(0.0, 24.0),
            }),
            Self::Reverb(p) => Self::Reverb(ReverbParams {
                room_size: p.room_size.clamp(0.0, 1.0),
                damping: p.damping.clamp(0.0, 1.0),
                width: p.width.clamp(0.0, 1.0),
                predelay_ms: p.predelay_ms.clamp(0.0, reverb::MAX_PREDELAY_MS),
                mix: p.mix.clamp(0.0, 1.0),
            }),
            Self::Delay(p) => Self::Delay(DelayParams {
                time_ms: p.time_ms.clamp(1.0, delay::MAX_DELAY_MS),
                feedback: p.feedback.clamp(0.0, 0.95),
                high_cut: p.high_cut.clamp(500.0, 20_000.0),
                ping_pong: p.ping_pong,
                mix: p.mix.clamp(0.0, 1.0),
            }),
            Self::Limiter(p) => Self::Limiter(LimiterParams {
                input_db: p.input_db.clamp(0.0, 24.0),
                ceiling_db: p.ceiling_db.clamp(-12.0, 0.0),
                release_ms: p.release_ms.clamp(5.0, 1000.0),
            }),
        }
    }
}

/// Un efecto en una cadena, tal como se guarda en el proyecto.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectSlot {
    /// Identificador estable (lo genera la interfaz).
    pub id: String,
    pub enabled: bool,
    pub params: EffectParams,
}

/// Procesador de audio estéreo entrelazado.
pub trait Processor: Send {
    fn process(&mut self, buf: &mut [f32]);
    fn reset(&mut self);
    /// Aplica parámetros nuevos sin reservar memoria. Devuelve `false` si son
    /// de otro tipo de efecto.
    fn set_params(&mut self, params: &EffectParams) -> bool;
}

/// Crea el procesador de un efecto (reserva memoria: fuera del hilo de audio).
pub fn build(params: &EffectParams, sample_rate: u32) -> Box<dyn Processor> {
    let sr = sample_rate as f32;
    match params.sanitized() {
        EffectParams::Eq(p) => Box::new(Eq::new(p, sr)),
        EffectParams::Compressor(p) => Box::new(Compressor::new(p, sr)),
        EffectParams::Reverb(p) => Box::new(Reverb::new(p, sr)),
        EffectParams::Delay(p) => Box::new(Delay::new(p, sr)),
        EffectParams::Limiter(p) => Box::new(Limiter::new(p, sr)),
    }
}

/// Cadena de efectos en serie.
pub struct Chain {
    slots: Vec<(bool, Box<dyn Processor>)>,
}

impl Chain {
    pub fn new(slots: &[EffectSlot], sample_rate: u32) -> Self {
        Self { slots: slots.iter().map(|s| (s.enabled, build(&s.params, sample_rate))).collect() }
    }

    pub fn is_empty(&self) -> bool {
        !self.slots.iter().any(|(on, _)| *on)
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for (on, p) in &mut self.slots {
            if *on {
                p.process(buf);
            }
        }
    }

    pub fn reset(&mut self) {
        for (_, p) in &mut self.slots {
            p.reset();
        }
    }

    /// Cambia los parámetros de un efecto sin reconstruir la cadena.
    pub fn set_params(&mut self, index: usize, params: &EffectParams) -> bool {
        self.slots.get_mut(index).is_some_and(|(_, p)| p.set_params(&params.sanitized()))
    }

    pub fn set_enabled(&mut self, index: usize, enabled: bool) {
        if let Some((on, p)) = self.slots.get_mut(index) {
            if enabled && !*on {
                p.reset();
            }
            *on = enabled;
        }
    }
}

/// `dB → lineal`
#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Coeficiente de un filtro de un polo con constante de tiempo `ms`.
#[inline]
pub(crate) fn time_coef(ms: f32, sr: f32) -> f32 {
    if ms <= 0.0 {
        0.0
    } else {
        (-1.0 / (ms * 0.001 * sr)).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_roundtrip_json() {
        let slot = EffectSlot { id: "a".into(), enabled: true, params: EffectParams::Delay(DelayParams::default()) };
        let json = serde_json::to_string(&slot).unwrap();
        assert!(json.contains(r#""type":"delay""#), "{json}");
        let back: EffectSlot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, slot);
        // Los parámetros que falten toman el valor por defecto.
        let p: EffectParams = serde_json::from_str(r#"{"type":"compressor","ratio":8}"#).unwrap();
        assert_eq!(p, EffectParams::Compressor(CompressorParams { ratio: 8.0, ..Default::default() }));
    }

    #[test]
    fn sanitize_clamps() {
        let p = EffectParams::Compressor(CompressorParams { ratio: 100.0, threshold_db: 5.0, ..Default::default() });
        let EffectParams::Compressor(c) = p.sanitized() else { panic!() };
        assert_eq!((c.ratio, c.threshold_db), (20.0, 0.0));
    }

    #[test]
    fn chain_runs_and_updates_in_place() {
        let slots = vec![
            EffectSlot { id: "1".into(), enabled: true, params: EffectParams::Eq(EqParams::default()) },
            EffectSlot { id: "2".into(), enabled: false, params: EffectParams::Reverb(ReverbParams::default()) },
        ];
        let mut chain = Chain::new(&slots, 48_000);
        let mut buf = vec![0.25f32; 1024];
        chain.process(&mut buf);
        // EQ plano y reverb desactivada: la señal no cambia (salvo el transitorio del filtro).
        assert!(buf[512..].iter().all(|s| (s - 0.25).abs() < 1e-4));
        assert!(chain.set_params(0, &EffectParams::Eq(EqParams { highpass: 80.0, ..Default::default() })));
        assert!(!chain.set_params(0, &EffectParams::Limiter(LimiterParams::default())), "otro tipo");
        assert!(!chain.set_params(5, &EffectParams::Eq(EqParams::default())), "índice inexistente");
    }
}
