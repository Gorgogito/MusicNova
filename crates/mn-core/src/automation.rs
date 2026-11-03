//! Automatización: curvas de volumen, panorama y parámetros de efectos.
//!
//! Un carril guarda puntos (segundo, valor) y se interpola linealmente entre
//! ellos; antes del primero y después del último el valor se mantiene. El
//! motor y la mezcla offline evalúan las curvas por bloques: los parámetros de
//! efectos se actualizan al inicio de cada bloque y el volumen y el panorama
//! se interpolan muestra a muestra dentro del bloque, sin escalones audibles.

use std::sync::atomic::{AtomicU32, Ordering};

use mn_fx::{Chain, EffectParams, EffectSlot};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::mix::{balance_gains, db_to_gain};

/// Puntos por carril como máximo.
pub const MAX_POINTS: usize = 10_000;
/// Carriles por pista como máximo.
pub const MAX_LANES: usize = 64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AutomationTarget {
    /// Volumen en dB (sustituye al fader mientras el carril tiene puntos).
    Volume,
    /// Panorama de −1 (izquierda) a 1 (derecha).
    Pan,
    /// Parámetro `param` (ruta, p. ej. "mid1.gain_db") del efecto con id `slot`.
    Effect { slot: String, param: String },
    /// Parámetro `param` (id) del plugin externo del efecto `slot`, en el
    /// dominio de valores del plugin. Nombre y rango se guardan para dibujar
    /// la curva aunque el plugin no esté cargado.
    Plugin { slot: String, param: u32, name: String, min: f32, max: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutoPoint {
    /// Segundo de la línea de tiempo.
    pub time: f64,
    pub value: f32,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutomationLane {
    pub id: Uuid,
    pub target: AutomationTarget,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub points: Vec<AutoPoint>,
}

impl AutomationTarget {
    /// Mismo destino (para un plugin cuentan el efecto y el id del parámetro).
    pub fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Plugin { slot: a, param: p, .. }, Self::Plugin { slot: b, param: q, .. }) => a == b && p == q,
            _ => self == other,
        }
    }

    /// Limita `v` al rango del destino; `None` si el destino ya no existe.
    pub fn clamp(&self, effects: &[EffectSlot], v: f32) -> Option<f32> {
        match self {
            Self::Volume => Some(v.clamp(-60.0, 12.0)),
            Self::Pan => Some(v.clamp(-1.0, 1.0)),
            Self::Effect { slot, param } => {
                let s = effects.iter().find(|s| &s.id == slot)?;
                s.params.param(param)?;
                s.params.with_param(param, v).param(param)
            }
            Self::Plugin { slot, min, max, .. } => {
                effects.iter().find(|s| &s.id == slot && s.plugin.is_some())?;
                let (lo, hi) = if min <= max { (*min, *max) } else { (*max, *min) };
                Some(if lo.is_finite() && hi.is_finite() { v.clamp(lo, hi) } else { v })
            }
        }
    }
}

/// Deja los carriles en un estado válido para la cadena `effects`: quita los
/// de efectos que ya no existen y los duplicados, ordena los puntos y limita
/// sus valores.
pub fn sanitize_lanes(lanes: Vec<AutomationLane>, effects: &[EffectSlot]) -> Vec<AutomationLane> {
    let mut out: Vec<AutomationLane> = Vec::new();
    for mut lane in lanes {
        if out.len() >= MAX_LANES || out.iter().any(|l| l.target.same(&lane.target) || l.id == lane.id) {
            continue;
        }
        if lane.target.clamp(effects, 0.0).is_none() {
            continue;
        }
        let target = lane.target.clone();
        lane.points = lane
            .points
            .into_iter()
            .filter(|p| p.time.is_finite() && p.value.is_finite())
            .filter_map(|p| Some(AutoPoint { time: p.time.max(0.0), value: target.clamp(effects, p.value)? }))
            .take(MAX_POINTS)
            .collect();
        lane.points.sort_by(|a, b| a.time.total_cmp(&b.time));
        out.push(lane);
    }
    out
}

/// Curva en frames, lista para evaluarse sin reservar memoria.
#[derive(Clone, Debug)]
pub struct Curve {
    frames: Vec<u64>,
    values: Vec<f32>,
}

impl Curve {
    /// `None` si no hay puntos (el carril no actúa).
    pub fn new(points: &[AutoPoint], rate: u32) -> Option<Self> {
        if points.is_empty() {
            return None;
        }
        Some(Self {
            frames: points.iter().map(|p| (p.time.max(0.0) * rate as f64).round() as u64).collect(),
            values: points.iter().map(|p| p.value).collect(),
        })
    }

    /// Valor en `frame`: interpolación lineal entre puntos.
    pub fn value_at(&self, frame: u64) -> f32 {
        let i = self.frames.partition_point(|&f| f <= frame);
        let n = self.frames.len();
        if i == 0 {
            self.values[0]
        } else if i == n {
            self.values[n - 1]
        } else {
            let (f0, f1) = (self.frames[i - 1], self.frames[i]);
            let k = (frame - f0) as f32 / (f1 - f0) as f32;
            self.values[i - 1] + (self.values[i] - self.values[i - 1]) * k
        }
    }
}

struct EffectAutomation {
    index: usize,
    /// Parámetros del efecto sin automatizar (los que fija el usuario).
    base: EffectParams,
    params: Vec<(&'static str, Curve)>,
}

/// Curva de un parámetro de plugin y último valor enviado (para no repetir).
struct PluginAutomation {
    index: usize,
    id: u32,
    curve: Curve,
    /// Bits del último valor enviado (NaN: ninguno todavía).
    last: AtomicU32,
}

/// Automatización de un canal (pista o máster) a una frecuencia concreta.
pub struct ChannelAutomation {
    volume: Option<Curve>,
    pan: Option<Curve>,
    effects: Vec<EffectAutomation>,
    plugins: Vec<PluginAutomation>,
}

impl ChannelAutomation {
    /// `None` si ningún carril activo tiene puntos.
    pub fn build(lanes: &[AutomationLane], effects: &[EffectSlot], rate: u32) -> Option<Self> {
        let mut a = Self { volume: None, pan: None, effects: Vec::new(), plugins: Vec::new() };
        for lane in lanes.iter().filter(|l| l.enabled) {
            let Some(curve) = Curve::new(&lane.points, rate) else { continue };
            match &lane.target {
                AutomationTarget::Volume => a.volume = Some(curve),
                AutomationTarget::Pan => a.pan = Some(curve),
                AutomationTarget::Effect { slot, param } => {
                    let Some(index) = effects.iter().position(|s| &s.id == slot) else { continue };
                    let base = effects[index].params;
                    let Some(&path) = base.param_paths().iter().find(|p| **p == param) else { continue };
                    match a.effects.iter_mut().find(|e| e.index == index) {
                        Some(e) => e.params.push((path, curve)),
                        None => a.effects.push(EffectAutomation { index, base, params: vec![(path, curve)] }),
                    }
                }
                AutomationTarget::Plugin { slot, param, .. } => {
                    let Some(index) = effects.iter().position(|s| &s.id == slot && s.plugin.is_some()) else { continue };
                    a.plugins.push(PluginAutomation { index, id: *param, curve, last: AtomicU32::new(f32::NAN.to_bits()) });
                }
            }
        }
        (a.volume.is_some() || a.pan.is_some() || !a.effects.is_empty() || !a.plugins.is_empty()).then_some(a)
    }

    pub fn has_gain(&self) -> bool {
        self.volume.is_some() || self.pan.is_some()
    }

    /// Ganancias izquierda/derecha en `frame`; sin curva se usa el fader.
    pub fn gains(&self, frame: u64, fader_gain: f32, fader_pan: f32) -> (f32, f32) {
        let gain = self.volume.as_ref().map_or(fader_gain, |c| db_to_gain(c.value_at(frame)));
        let pan = self.pan.as_ref().map_or(fader_pan, |c| c.value_at(frame));
        balance_gains(gain, pan)
    }

    /// Ganancia del máster en `frame` (sin panorama).
    pub fn master_gain(&self, frame: u64, fader_gain: f32) -> f32 {
        self.volume.as_ref().map_or(fader_gain, |c| db_to_gain(c.value_at(frame)))
    }

    /// Olvida lo enviado a los plugins: el siguiente bloque les reenvía sus valores.
    pub fn invalidate(&self) {
        for p in &self.plugins {
            p.last.store(f32::NAN.to_bits(), Ordering::Relaxed);
        }
    }

    /// Fija los parámetros automatizados de la cadena para `frame`. No reserva memoria.
    pub fn apply_effects(&self, chain: &mut Chain, frame: u64) {
        for e in &self.effects {
            let mut p = e.base;
            for (path, curve) in &e.params {
                if let Some(v) = p.param_mut(path) {
                    *v = curve.value_at(frame);
                }
            }
            chain.set_params(e.index, &p);
        }
        // Plugins: solo se envía cuando la curva cambia el valor.
        for p in &self.plugins {
            let v = p.curve.value_at(frame);
            if f32::from_bits(p.last.load(Ordering::Relaxed)) != v {
                p.last.store(v.to_bits(), Ordering::Relaxed);
                chain.set_plugin_param(p.index, p.id, v as f64);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mn_fx::{EffectParams, ReverbParams};

    fn pts(v: &[(f64, f32)]) -> Vec<AutoPoint> {
        v.iter().map(|&(time, value)| AutoPoint { time, value }).collect()
    }

    #[test]
    fn curve_interpolates_and_holds() {
        let c = Curve::new(&pts(&[(1.0, 0.0), (2.0, 10.0), (2.0, -5.0), (4.0, -5.0)]), 100).unwrap();
        assert_eq!(c.value_at(0), 0.0, "antes del primer punto se mantiene");
        assert_eq!(c.value_at(150), 5.0);
        assert_eq!(c.value_at(200), -5.0, "dos puntos en el mismo instante: salto");
        assert_eq!(c.value_at(1000), -5.0, "después del último se mantiene");
        assert!(Curve::new(&[], 100).is_none());
    }

    fn reverb() -> Vec<EffectSlot> {
        vec![EffectSlot { plugin: None, sidechain: None, id: "rv".into(), enabled: true, params: EffectParams::Reverb(ReverbParams::default()) }]
    }

    #[test]
    fn lanes_are_sanitized() {
        let lane = |target, points| AutomationLane { id: Uuid::new_v4(), target, enabled: true, points };
        let fx = reverb();
        let out = sanitize_lanes(
            vec![
                lane(AutomationTarget::Volume, pts(&[(3.0, 40.0), (-1.0, -100.0), (f64::NAN, 0.0)])),
                lane(AutomationTarget::Volume, pts(&[(0.0, 0.0)])), // duplicado
                lane(AutomationTarget::Effect { slot: "rv".into(), param: "mix".into() }, pts(&[(0.0, 2.0)])),
                lane(AutomationTarget::Effect { slot: "rv".into(), param: "ratio".into() }, vec![]), // no es de la reverb
                lane(AutomationTarget::Effect { slot: "borrado".into(), param: "mix".into() }, vec![]),
            ],
            &fx,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].points, pts(&[(0.0, -60.0), (3.0, 12.0)]));
        assert_eq!(out[1].points, pts(&[(0.0, 1.0)]));
    }

    #[test]
    fn channel_automation_overrides_fader_and_effect_params() {
        let fx = reverb();
        let lanes = vec![
            AutomationLane { id: Uuid::new_v4(), target: AutomationTarget::Volume, enabled: true, points: pts(&[(0.0, -6.0), (1.0, 0.0)]) },
            AutomationLane { id: Uuid::new_v4(), target: AutomationTarget::Pan, enabled: false, points: pts(&[(0.0, 1.0)]) },
            AutomationLane {
                id: Uuid::new_v4(),
                target: AutomationTarget::Effect { slot: "rv".into(), param: "mix".into() },
                enabled: true,
                points: pts(&[(0.0, 0.0), (1.0, 1.0)]),
            },
        ];
        let a = ChannelAutomation::build(&lanes, &fx, 100).unwrap();
        assert!(a.has_gain());
        let (l, r) = a.gains(100, 0.1, 0.0);
        assert_eq!((l, r), (1.0, 1.0), "el volumen automatizado manda; el panorama desactivado no");
        let (l, _) = a.gains(0, 1.0, 0.0);
        assert!((l - 0.501).abs() < 0.001);

        // Sin carriles activos con puntos no hay automatización.
        let none = vec![AutomationLane { id: Uuid::new_v4(), target: AutomationTarget::Volume, enabled: true, points: vec![] }];
        assert!(ChannelAutomation::build(&none, &fx, 100).is_none());

        // La reverb automatizada: con mix 0 no hay señal procesada; con mix 1, sí.
        let mut chain = Chain::new(&fx, 48_000);
        let a = ChannelAutomation::build(&lanes, &fx, 48_000).unwrap();
        a.apply_effects(&mut chain, 0);
        let mut silent = vec![0.0f32; 4096];
        silent[0] = 1.0;
        silent[1] = 1.0;
        chain.process(&mut silent);
        assert!(silent[2000..].iter().all(|s| s.abs() < 1e-6), "mix 0: solo la señal seca");
    }
}
