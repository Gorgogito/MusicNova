//! Prueba manual de la grabación: `cargo run -p mn-engine --example record [segundos]`.
//!
//! Lista las entradas, abre la predeterminada, intenta medir la latencia
//! (necesita que la salida llegue a la entrada) y graba unos segundos sobre un
//! tono de referencia. La toma queda en la carpeta temporal.

use std::sync::Arc;
use std::time::Duration;

use mn_engine::{input_devices, Engine, InputChannels};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let secs: f64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(3.0);
    let engine = Engine::start()?;
    let rate = engine.sample_rate();
    println!("salida: {} @ {rate} Hz", engine.device_name());
    for d in input_devices() {
        println!("entrada: {}{} · {} canales @ {} Hz", d.name, if d.is_default { " (predeterminada)" } else { "" }, d.channels, d.sample_rate);
    }
    let info = engine.open_input(None, InputChannels::Mono(0))?;
    println!("abierta: {} @ {} Hz · monitorización {}", info.device, info.sample_rate, if info.monitor_available { "sí" } else { "no" });

    std::thread::sleep(Duration::from_millis(300));
    let latency = match engine.calibrate_latency() {
        Ok(r) => {
            println!(
                "latencia: {:.2} ms (dispersión {:.2} ms, {}/{} golpes, nivel {:.1} dBFS)",
                r.latency_ms, r.spread_ms, r.detected, r.total, r.level_db
            );
            r.latency_ms
        }
        Err(e) => {
            println!("calibración: {e}");
            0.0
        }
    };

    // Pista de referencia: un barrido cada 0,5 s (a partir de 0,25 s). Al
    // terminar se buscan en la toma: con la latencia bien compensada deben
    // caer en los mismos instantes de la línea de tiempo.
    let n = (rate as f64 * secs) as usize;
    let template = chirp(rate);
    let marks: Vec<usize> = (0..).map(|k| rate as usize / 4 + k * rate as usize / 2).take_while(|&m| m + template.len() < n).collect();
    let mut data = vec![0.0f32; n * 2];
    for &m in &marks {
        for (i, s) in template.iter().enumerate() {
            data[(m + i) * 2] = *s;
            data[(m + i) * 2 + 1] = *s;
        }
    }
    engine.add_track(Arc::new(data), rate)?;
    let path = std::env::temp_dir().join("musicnova_toma_prueba.wav");
    engine.start_recording(path, latency, None)?;
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f64() < secs + 1.0 {
        std::thread::sleep(Duration::from_millis(250));
        if let Some(l) = engine.input_levels() {
            println!("  {:.2} s grabados · pico {:.1} dBFS", l.recorded_secs, 20.0 * l.peak_l.max(1e-6).log10());
        }
    }
    match engine.stop_recording()? {
        Some(t) => {
            println!(
                "toma: {} · {:.2} s desde {:.4} s · {} frames perdidos",
                t.path.display(),
                t.frames as f64 / t.sample_rate as f64,
                t.offset_secs,
                t.dropped_frames
            );
            let take = mn_core::audio::decode_file(&t.path)?;
            let mono: Vec<f32> = take.data.chunks_exact(2).map(|f| 0.5 * (f[0] + f[1])).collect();
            let tr = take.sample_rate as f64;
            let tpl = chirp(take.sample_rate);
            // Posición esperada de cada barrido dentro de la toma.
            let expected: Vec<f64> = marks.iter().map(|&m| (m as f64 / rate as f64 - t.offset_secs) * tr).filter(|&e| e > 0.0).collect();
            match mn_engine::measure_latency(&mono, take.sample_rate, &expected, &tpl) {
                Ok(r) => println!(
                    "alineación de la toma: error {:+.2} ms (dispersión {:.2} ms, {}/{} barridos)",
                    r.latency_ms, r.spread_ms, r.detected, r.total
                ),
                Err(e) => println!("alineación de la toma: {e}"),
            }
        }
        None => println!("la toma no llegó a empezar"),
    }
    let s = engine.state();
    println!("transporte: posición {:.2} s, duración {:.2} s", s.position_frames as f64 / rate as f64, s.length_frames as f64 / rate as f64);
    Ok(())
}

/// Mismo barrido que usa el motor para calibrar.
fn chirp(sample_rate: u32) -> Vec<f32> {
    let (f0, f1, dur) = (400.0f64, 8000.0f64, 0.012f64);
    let n = (sample_rate as f64 * dur) as usize;
    let k = (f1 - f0) / dur;
    (0..n)
        .map(|i| {
            let t = i as f64 / sample_rate as f64;
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (n - 1) as f64).cos();
            ((std::f64::consts::TAU * (f0 * t + 0.5 * k * t * t)).sin() * w * 0.8) as f32
        })
        .collect()
}
