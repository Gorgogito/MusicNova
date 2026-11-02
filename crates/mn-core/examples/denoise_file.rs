//! Quita el ruido constante de un archivo (para medir el reductor espectral):
//! `cargo run -p mn-core --example denoise_file -- entrada.wav salida.wav [reducción dB] [sensibilidad]`

use mn_core::audio::{decode_file, write_wav_f32};
use mn_core::denoise::{denoise, DenoiseSettings};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, rest @ ..] = &args[..] else {
        eprintln!("uso: denoise_file entrada.wav salida.wav [reducción dB] [sensibilidad]");
        std::process::exit(2);
    };
    let audio = decode_file(input.as_ref()).expect("audio");
    let s = DenoiseSettings {
        reduction_db: rest.first().map_or(18.0, |v| v.parse().unwrap()),
        sensitivity: rest.get(1).map_or(0.5, |v| v.parse().unwrap()),
        noise_region: None,
    };
    write_wav_f32(output.as_ref(), &denoise(&audio, &s)).expect("escribir");
}
