//! Sonido del metrónomo: golpes sintetizados una sola vez al arrancar el motor.

/// Duración del golpe de metrónomo.
const CLICK_SECONDS: f32 = 0.035;

pub struct Clicks {
    pub accent: Vec<f32>,
    pub normal: Vec<f32>,
}

impl Clicks {
    pub fn new(sample_rate: u32) -> Self {
        Self { accent: synth(sample_rate, 1760.0, 0.9), normal: synth(sample_rate, 1320.0, 0.6) }
    }
}

/// Seno con ataque corto y caída exponencial: suena nítido sin clics de borde.
fn synth(sample_rate: u32, hz: f32, gain: f32) -> Vec<f32> {
    let n = (sample_rate as f32 * CLICK_SECONDS) as usize;
    let attack = (sample_rate as f32 * 0.001) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / sample_rate as f32;
            let env = if i < attack { i as f32 / attack as f32 } else { (-t / 0.008).exp() };
            (2.0 * std::f32::consts::PI * hz * t).sin() * env * gain
        })
        .collect()
}
