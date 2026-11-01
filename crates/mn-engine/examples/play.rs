//! Prueba manual del motor: `cargo run -p mn-engine --example play [archivo]`.
//! Sin archivo, reproduce un tono de 440 Hz durante 1,5 s.
//! Variables opcionales: `MN_TEMPO` (p. ej. 0.75) y `MN_PITCH` (semitonos).

use std::sync::Arc;
use std::time::Duration;

use mn_core::audio::{decode_file, resample, AudioBuffer};
use mn_engine::Engine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::start()?;
    let rate = engine.sample_rate();
    println!("dispositivo: {} @ {rate} Hz", engine.device_name());

    let buf = match std::env::args().nth(1) {
        Some(path) => resample(&decode_file(path.as_ref())?, rate)?,
        None => {
            let n = (rate as f32 * 1.5) as usize;
            let data = (0..n)
                .flat_map(|i| {
                    let s = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin() * 0.2;
                    [s, s]
                })
                .collect();
            AudioBuffer { sample_rate: rate, data }
        }
    };
    let secs = buf.duration_secs();
    engine.add_track(Arc::new(buf.data), rate)?;
    let tempo: f64 = std::env::var("MN_TEMPO").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    let pitch: f32 = std::env::var("MN_PITCH").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    engine.set_tempo(tempo)?;
    engine.set_pitch(pitch)?;
    engine.play()?;

    let wait = (secs / tempo).min(5.0);
    std::thread::sleep(Duration::from_secs_f64(wait));
    let s = engine.state();
    let pos = s.position_frames as f64 / rate as f64;
    println!("posición tras {wait:.2} s: {pos:.2} s · pico {:.3}", s.peak_l.max(s.peak_r));
    if (pos - wait * tempo).abs() > 0.3 && s.playing {
        return Err("el transporte no avanza al ritmo esperado".into());
    }
    Ok(())
}
