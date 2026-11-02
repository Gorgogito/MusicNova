//! Sonido del metrónomo: golpes sintetizados una sola vez al arrancar el motor.

/// Duración del golpe de metrónomo.
const CLICK_SECONDS: f32 = 0.035;

pub struct Clicks {
    pub accent: Vec<f32>,
    pub normal: Vec<f32>,
    pub chirp: Vec<f32>,
}

impl Clicks {
    pub fn new(sample_rate: u32) -> Self {
        Self { accent: synth(sample_rate, 1760.0, 0.9), normal: synth(sample_rate, 1320.0, 0.6), chirp: chirp(sample_rate) }
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

/// Barrido de 400 Hz a 8 kHz en 12 ms para medir la latencia: su
/// autocorrelación tiene un único pico estrecho, así que su llegada se localiza
/// con precisión de muestra incluso tras pasar por altavoz y micrófono.
pub fn chirp(sample_rate: u32) -> Vec<f32> {
    let (f0, f1, dur) = (400.0f64, 8000.0f64, 0.012f64);
    let n = (sample_rate as f64 * dur) as usize;
    let k = (f1 - f0) / dur;
    (0..n)
        .map(|i| {
            let t = i as f64 / sample_rate as f64;
            // Ventana de Hann para que no haya bordes abruptos.
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (n - 1) as f64).cos();
            ((std::f64::consts::TAU * (f0 * t + 0.5 * k * t * t)).sin() * w * 0.8) as f32
        })
        .collect()
}
