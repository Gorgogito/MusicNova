//! Toca un acorde con un plugin instrumento y lo guarda en WAV (validación con
//! instrumentos reales): `cargo run -p mn-plugin --example render_instrument -- Synth.clap salida.wav`

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path, out, ..] = &args[..] else {
        eprintln!("uso: render_instrument plugin.clap|plugin.vst3 salida.wav");
        std::process::exit(2);
    };
    mn_plugin::install();
    let found = mn_plugin::scan_file(Path::new(path)).expect("escaneo");
    let d = found.iter().find(|d| d.instrument).expect("el archivo no contiene ningún instrumento");
    println!("instrumento: {} ({:?}, {})", d.name, d.format, d.category);
    let rate = 48_000u32;
    // Do mayor (Do4 Mi4 Sol4) 0,2–1,2 s y La3 1,3–2,0 s.
    let notes = [(0.2, 1.2, 60u8, 100u8), (0.2, 1.2, 64, 100), (0.2, 1.2, 67, 100), (1.3, 2.0, 57, 110)];
    let t = std::time::Instant::now();
    let audio = mn_plugin::render_instrument(&d.to_ref(), &notes, rate, 2.0, 1.5).expect("render");
    let secs = t.elapsed().as_secs_f64();
    let left: Vec<f32> = audio.iter().step_by(2).copied().collect();
    let rms = |a: f64, b: f64| {
        let s = &left[(a * rate as f64) as usize..(b * rate as f64) as usize];
        20.0 * ((s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt().max(1e-9)).log10()
    };
    println!(
        "render {secs:.2} s · antes {:.1} dBFS · acorde {:.1} dBFS · La3 {:.1} dBFS · cola final {:.1} dBFS",
        rms(0.0, 0.19),
        rms(0.4, 1.1),
        rms(1.4, 1.9),
        rms(3.2, 3.5)
    );
    let mut w = hound_like(out, rate);
    w.extend(audio.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
    write_wav(out, rate, &w);
}

fn hound_like(_: &str, _: u32) -> Vec<i16> {
    Vec::new()
}

fn write_wav(path: &str, rate: u32, data: &[i16]) {
    let mut b = Vec::with_capacity(44 + data.len() * 2);
    let len = (data.len() * 2) as u32;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&len.to_le_bytes());
    for s in data {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, b).expect("escribir");
}
