//! Afina un archivo con una curva de tono (prueba de extremo a extremo de la fase 6b):
//! `cargo run -p mn-core --example tune_file -- voz.wav pitch.json salida.wav [intensidad] [estilo]`

use mn_core::audio::{decode_file, write_wav_f32};
use mn_core::transcription::PitchCurve;
use mn_core::tuning::{tune, TuneSettings};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, curve, output, rest @ ..] = &args[..] else {
        eprintln!("uso: tune_file voz.wav pitch.json salida.wav [intensidad] [estilo]");
        std::process::exit(2);
    };
    let audio = decode_file(input.as_ref()).expect("audio");
    let curve: PitchCurve = serde_json::from_str(&std::fs::read_to_string(curve).expect("curva")).expect("curva válida");
    let settings = TuneSettings {
        strength: rest.first().map_or(1.0, |s| s.parse().unwrap()),
        speed: rest.get(1).map_or(0.0, |s| s.parse().unwrap()),
        ..Default::default()
    };
    let t = std::time::Instant::now();
    let out = tune(&audio, &curve, &settings);
    write_wav_f32(output.as_ref(), &out).expect("escribir");
    println!("{:.1} s de audio afinados en {} ms", audio.duration_secs(), t.elapsed().as_millis());
}
