//! Transpone un archivo un número fijo de semitonos por el mismo camino que la
//! afinación vocal, con o sin conservar los formantes (para medir el timbre):
//! `cargo run -p mn-core --example shift_file -- voz.wav salida.wav semitonos [formantes 0|1]`

use mn_core::audio::{decode_file, write_wav_f32};
use mn_core::tuning::apply_shifts;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, semis, rest @ ..] = &args[..] else {
        eprintln!("uso: shift_file voz.wav salida.wav semitonos [formantes 0|1]");
        std::process::exit(2);
    };
    let audio = decode_file(input.as_ref()).expect("audio");
    let semis: f32 = semis.parse().expect("semitonos");
    let formants = rest.first().is_none_or(|f| f != "0");
    let hop = 0.01;
    let shifts = vec![semis; (audio.duration_secs() / hop) as usize + 2];
    write_wav_f32(output.as_ref(), &apply_shifts(&audio, &shifts, hop, formants)).expect("escribir");
}
