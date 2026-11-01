//! Exportación de audio a WAV, FLAC (flacenc, Apache-2.0) y MP3 (LAME, LGPL-3.0).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::audio::{write_wav, AudioBuffer, WavFormat};
use crate::{CoreError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Wav16,
    Wav24,
    Wav32f,
    Flac16,
    Flac24,
    Mp3_320,
    Mp3_256,
    Mp3_192,
    Mp3_128,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav16 | Self::Wav24 | Self::Wav32f => "wav",
            Self::Flac16 | Self::Flac24 => "flac",
            _ => "mp3",
        }
    }
}

/// Escribe `buf` en `path` con el formato indicado.
pub fn export_audio(path: &Path, buf: &AudioBuffer, format: ExportFormat) -> Result<()> {
    match format {
        ExportFormat::Wav16 => write_wav(path, buf, WavFormat::Pcm16),
        ExportFormat::Wav24 => write_wav(path, buf, WavFormat::Pcm24),
        ExportFormat::Wav32f => write_wav(path, buf, WavFormat::Float32),
        ExportFormat::Flac16 => write_flac(path, buf, 16),
        ExportFormat::Flac24 => write_flac(path, buf, 24),
        ExportFormat::Mp3_320 => write_mp3(path, buf, mp3lame_encoder::Bitrate::Kbps320),
        ExportFormat::Mp3_256 => write_mp3(path, buf, mp3lame_encoder::Bitrate::Kbps256),
        ExportFormat::Mp3_192 => write_mp3(path, buf, mp3lame_encoder::Bitrate::Kbps192),
        ExportFormat::Mp3_128 => write_mp3(path, buf, mp3lame_encoder::Bitrate::Kbps128),
    }
}

/// Cuantiza a enteros de `bits`; a 16 bits aplica dither TPDF.
fn quantize(data: &[f32], bits: u32) -> Vec<i32> {
    let max = ((1i64 << (bits - 1)) - 1) as f64;
    let min = -max - 1.0;
    let mut seed: u32 = 0x9E37_79B9;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f64 / u32::MAX as f64
    };
    data.iter()
        .map(|&s| {
            let dither = if bits <= 16 { rand() - rand() } else { 0.0 };
            (s as f64 * max + dither).round().clamp(min, max) as i32
        })
        .collect()
}

fn write_flac(path: &Path, buf: &AudioBuffer, bits: u32) -> Result<()> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;

    let samples = quantize(&buf.data, bits);
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|e| CoreError::Format(format!("configuración FLAC: {e:?}")))?;
    let source = flacenc::source::MemSource::from_samples(&samples, 2, bits as usize, buf.sample_rate as usize);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| CoreError::Format(format!("codificación FLAC: {e:?}")))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).map_err(|e| CoreError::Format(format!("escritura FLAC: {e:?}")))?;
    let mut bytes = sink.as_slice().to_vec();
    // flacenc anota como tamaño mínimo de bloque el del último bloque (más corto),
    // pero la especificación lo excluye; algunos lectores (symphonia) rechazan el
    // archivo. Con bloque fijo, mínimo = máximo. STREAMINFO empieza en el byte 8.
    if bytes.len() > 12 && &bytes[..4] == b"fLaC" && bytes[4] & 0x7f == 0 {
        let (min, max) = bytes[8..12].split_at_mut(2);
        min.copy_from_slice(max);
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

fn write_mp3(path: &Path, buf: &AudioBuffer, bitrate: mp3lame_encoder::Bitrate) -> Result<()> {
    use mp3lame_encoder::{Builder, FlushNoGap, InterleavedPcm, Quality};

    let err = |what: &str, e: &dyn std::fmt::Debug| CoreError::Format(format!("MP3 ({what}): {e:?}"));
    let mut encoder = Builder::new()
        .ok_or_else(|| CoreError::Format("no se pudo crear el codificador LAME".into()))?
        .with_num_channels(2)
        .map_err(|e| err("canales", &e))?
        .with_sample_rate(buf.sample_rate)
        .map_err(|e| err("frecuencia", &e))?
        .with_brate(bitrate)
        .map_err(|e| err("tasa de bits", &e))?
        .with_quality(Quality::NearBest)
        .map_err(|e| err("calidad", &e))?
        .build()
        .map_err(|e| err("inicialización", &e))?;

    let clamped: Vec<f32> = buf.data.iter().map(|s| s.clamp(-1.0, 1.0)).collect();
    let mut out = Vec::with_capacity(buf.data.len() / 4);
    // Bloques de 64 Ki frames para limitar el buffer intermedio.
    for chunk in clamped.chunks(65_536 * 2) {
        out.reserve(mp3lame_encoder::max_required_buffer_size(chunk.len() / 2));
        encoder.encode_to_vec(InterleavedPcm(chunk), &mut out).map_err(|e| err("codificación", &e))?;
    }
    out.reserve(7200);
    encoder.flush_to_vec::<FlushNoGap>(&mut out).map_err(|e| err("cierre", &e))?;
    std::fs::write(path, out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::decode_file;

    fn sine(rate: u32, secs: f64) -> AudioBuffer {
        let n = (rate as f64 * secs) as usize;
        let data = (0..n)
            .flat_map(|i| {
                let t = i as f64 / rate as f64;
                [(2.0 * std::f64::consts::PI * 440.0 * t).sin() as f32 * 0.5, (2.0 * std::f64::consts::PI * 660.0 * t).sin() as f32 * 0.4]
            })
            .collect();
        AudioBuffer { sample_rate: rate, data }
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mn_export_{name}"))
    }

    #[test]
    fn flac_is_lossless_after_quantization() {
        let buf = sine(44_100, 1.0);
        let path = tmp("t.flac");
        export_audio(&path, &buf, ExportFormat::Flac24).unwrap();
        let back = decode_file(&path).unwrap();
        assert_eq!(back.frames(), buf.frames());
        std::fs::remove_file(&path).ok();
        let max_err = back.data.iter().zip(&buf.data).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(max_err < 2.0 / 8_388_608.0, "error {max_err}");
    }

    #[test]
    fn mp3_keeps_duration_and_level() {
        let buf = sine(48_000, 2.0);
        let path = tmp("t.mp3");
        export_audio(&path, &buf, ExportFormat::Mp3_320).unwrap();
        let back = decode_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(back.sample_rate, 48_000);
        let dur = back.duration_secs();
        assert!((dur - 2.0).abs() < 0.1, "duración {dur}");
        let rms = |d: &[f32]| (d.iter().map(|s| s * s).sum::<f32>() / d.len() as f32).sqrt();
        let (a, b) = (rms(&buf.data), rms(&back.data[..buf.data.len().min(back.data.len())]));
        assert!((a - b).abs() / a < 0.05, "rms {a} vs {b}");
    }
}
