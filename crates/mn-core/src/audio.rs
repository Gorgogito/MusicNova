//! Decodificación (symphonia), remuestreo (rubato) y escritura WAV.
//!
//! Todo el audio interno se maneja como estéreo entrelazado `f32`.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use rubato::{FftFixedIn, Resampler};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::{CoreError, Result};

/// Audio estéreo entrelazado (L, R, L, R, ...).
#[derive(Clone, Debug)]
pub struct AudioBuffer {
    pub sample_rate: u32,
    pub data: Vec<f32>,
}

impl AudioBuffer {
    pub fn frames(&self) -> usize {
        self.data.len() / 2
    }

    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / self.sample_rate as f64
    }
}

/// Extensiones que el decodificador acepta.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    "wav", "wave", "flac", "mp3", "ogg", "oga", "m4a", "mp4", "aac", "aif", "aiff", "caf", "mkv",
    "webm",
];

/// Decodifica un archivo completo a estéreo `f32` en su frecuencia original.
pub fn decode_file(path: &Path) -> Result<AudioBuffer> {
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| CoreError::Decode(e.to_string()))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| CoreError::Decode("el archivo no contiene pistas de audio".into()))?;
    let track_id = track.id;
    let mut sample_rate = track.codec_params.sample_rate.unwrap_or(0);
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| CoreError::Decode(e.to_string()))?;

    let mut data = Vec::new();
    if let Some(n) = track.codec_params.n_frames {
        data.reserve(n as usize * 2);
    }
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(CoreError::Decode(e.to_string())),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // Un paquete corrupto no debe abortar toda la canción.
            Err(SymError::DecodeError(_)) => continue,
            // Algunos lectores señalan el final del flujo desde el decodificador.
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof && !data.is_empty() => break,
            Err(e) => return Err(CoreError::Decode(e.to_string())),
        };
        let spec = *decoded.spec();
        sample_rate = spec.rate;
        let needed = decoded.capacity() * spec.channels.count();
        if sample_buf.as_ref().map_or(true, |b| b.capacity() < needed) {
            sample_buf = Some(SampleBuffer::new(decoded.capacity() as u64, spec));
        }
        let buf = sample_buf.as_mut().unwrap();
        buf.copy_interleaved_ref(decoded);
        push_as_stereo(&mut data, buf.samples(), spec.channels.count());
    }

    if sample_rate == 0 || data.is_empty() {
        return Err(CoreError::Decode("no se obtuvieron muestras de audio".into()));
    }
    Ok(AudioBuffer { sample_rate, data })
}

fn push_as_stereo(out: &mut Vec<f32>, samples: &[f32], channels: usize) {
    match channels {
        0 => {}
        1 => {
            for &s in samples {
                out.push(s);
                out.push(s);
            }
        }
        2 => out.extend_from_slice(samples),
        n => {
            // Multicanal: se conservan los dos primeros canales (frontal izq./der.).
            for frame in samples.chunks_exact(n) {
                out.push(frame[0]);
                out.push(frame[1]);
            }
        }
    }
}

/// Remuestrea a `target_rate` con un remuestreador FFT de alta calidad.
pub fn resample(buf: &AudioBuffer, target_rate: u32) -> Result<AudioBuffer> {
    if buf.sample_rate == target_rate {
        return Ok(buf.clone());
    }
    const CHUNK: usize = 8192;
    let err = |e: &dyn std::fmt::Display| CoreError::Resample(e.to_string());

    let frames = buf.frames();
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for f in buf.data.chunks_exact(2) {
        left.push(f[0]);
        right.push(f[1]);
    }

    let mut rs = FftFixedIn::<f32>::new(buf.sample_rate as usize, target_rate as usize, CHUNK, 2, 2)
        .map_err(|e| err(&e))?;
    let ratio = target_rate as f64 / buf.sample_rate as f64;
    let expected = (frames as f64 * ratio).round() as usize;
    let delay = rs.output_delay();
    let mut out_l = Vec::with_capacity(expected + delay + CHUNK);
    let mut out_r = Vec::with_capacity(expected + delay + CHUNK);

    let mut pos = 0;
    while pos + CHUNK <= frames {
        let res = rs
            .process(&[&left[pos..pos + CHUNK], &right[pos..pos + CHUNK]], None)
            .map_err(|e| err(&e))?;
        out_l.extend_from_slice(&res[0]);
        out_r.extend_from_slice(&res[1]);
        pos += CHUNK;
    }
    if pos < frames {
        let res = rs
            .process_partial(Some(&[&left[pos..], &right[pos..]]), None)
            .map_err(|e| err(&e))?;
        out_l.extend_from_slice(&res[0]);
        out_r.extend_from_slice(&res[1]);
    }
    // Vacía el retardo interno del filtro.
    while out_l.len() < expected + delay {
        let res = rs.process_partial::<&[f32]>(None, None).map_err(|e| err(&e))?;
        if res[0].is_empty() {
            break;
        }
        out_l.extend_from_slice(&res[0]);
        out_r.extend_from_slice(&res[1]);
    }

    let end = (delay + expected).min(out_l.len());
    let mut data = Vec::with_capacity(end.saturating_sub(delay) * 2);
    for i in delay..end {
        data.push(out_l[i]);
        data.push(out_r[i]);
    }
    Ok(AudioBuffer { sample_rate: target_rate, data })
}

/// Escribe un WAV estéreo de 32 bits en coma flotante (sin pérdida respecto al buffer).
pub fn write_wav_f32(path: &Path, buf: &AudioBuffer) -> Result<()> {
    let mut w = BufWriter::new(File::create(path)?);
    let data_bytes = (buf.data.len() * 4) as u32;
    let channels: u16 = 2;
    let block_align = channels * 4;
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_bytes).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&3u16.to_le_bytes())?; // WAVE_FORMAT_IEEE_FLOAT
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&buf.sample_rate.to_le_bytes())?;
    w.write_all(&(buf.sample_rate * block_align as u32).to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&32u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_bytes.to_le_bytes())?;
    for s in &buf.data {
        w.write_all(&s.to_le_bytes())?;
    }
    w.flush()?;
    Ok(())
}

/// Escritor incremental de WAV estéreo `f32`: escribe la cabecera al crear el
/// archivo y la corrige al terminar, de modo que una grabación larga nunca
/// tiene que caber entera en memoria.
pub struct WavF32Writer {
    w: BufWriter<File>,
    frames: u64,
}

impl WavF32Writer {
    pub fn create(path: &Path, sample_rate: u32) -> Result<Self> {
        let mut w = BufWriter::new(File::create(path)?);
        let block_align: u16 = 2 * 4;
        w.write_all(b"RIFF")?;
        w.write_all(&36u32.to_le_bytes())?;
        w.write_all(b"WAVEfmt ")?;
        w.write_all(&16u32.to_le_bytes())?;
        w.write_all(&3u16.to_le_bytes())?; // WAVE_FORMAT_IEEE_FLOAT
        w.write_all(&2u16.to_le_bytes())?;
        w.write_all(&sample_rate.to_le_bytes())?;
        w.write_all(&(sample_rate * block_align as u32).to_le_bytes())?;
        w.write_all(&block_align.to_le_bytes())?;
        w.write_all(&32u16.to_le_bytes())?;
        w.write_all(b"data")?;
        w.write_all(&0u32.to_le_bytes())?;
        Ok(Self { w, frames: 0 })
    }

    /// Añade frames estéreo entrelazados.
    pub fn write(&mut self, interleaved: &[f32]) -> Result<()> {
        for s in interleaved {
            self.w.write_all(&s.to_le_bytes())?;
        }
        self.frames += (interleaved.len() / 2) as u64;
        Ok(())
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Corrige los tamaños de la cabecera y cierra el archivo.
    pub fn finish(mut self) -> Result<u64> {
        use std::io::{Seek, SeekFrom};
        let data_bytes = u32::try_from(self.frames * 8)
            .map_err(|_| CoreError::Format("la grabación supera el tamaño máximo de un WAV (4 GB)".into()))?;
        self.w.flush()?;
        let f = self.w.get_mut();
        f.seek(SeekFrom::Start(4))?;
        f.write_all(&(36 + data_bytes).to_le_bytes())?;
        f.seek(SeekFrom::Start(40))?;
        f.write_all(&data_bytes.to_le_bytes())?;
        f.flush()?;
        Ok(self.frames)
    }
}

/// Formato de muestra para exportar WAV.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WavFormat {
    Pcm16,
    Pcm24,
    Float32,
}

/// Escribe un WAV estéreo en el formato indicado. A 16 bits aplica dither TPDF.
pub fn write_wav(path: &Path, buf: &AudioBuffer, format: WavFormat) -> Result<()> {
    if format == WavFormat::Float32 {
        return write_wav_f32(path, buf);
    }
    let bytes_per_sample: u16 = if format == WavFormat::Pcm16 { 2 } else { 3 };
    let channels: u16 = 2;
    let block_align = channels * bytes_per_sample;
    let data_bytes = (buf.frames() as u64 * block_align as u64) as u32;
    let mut w = BufWriter::new(File::create(path)?);
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_bytes).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?; // WAVE_FORMAT_PCM
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&buf.sample_rate.to_le_bytes())?;
    w.write_all(&(buf.sample_rate * block_align as u32).to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&(bytes_per_sample * 8).to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_bytes.to_le_bytes())?;

    // Generador xorshift para el dither: determinista y sin dependencias.
    let mut seed: u32 = 0x9E37_79B9;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed as f32 / u32::MAX as f32
    };
    for &s in &buf.data {
        if format == WavFormat::Pcm16 {
            let dither = rand() - rand();
            let v = (s * 32767.0 + dither).round().clamp(-32768.0, 32767.0) as i16;
            w.write_all(&v.to_le_bytes())?;
        } else {
            let v = (s as f64 * 8_388_607.0).round().clamp(-8_388_608.0, 8_388_607.0) as i32;
            w.write_all(&v.to_le_bytes()[..3])?;
        }
    }
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_wav_roundtrip() {
        let buf = sine(44_100, 0.2, 330.0);
        let path = std::env::temp_dir().join(format!("mn_core_stream_{}.wav", std::process::id()));
        let mut w = WavF32Writer::create(&path, 44_100).unwrap();
        for chunk in buf.data.chunks(1000) {
            w.write(chunk).unwrap();
        }
        assert_eq!(w.finish().unwrap() as usize, buf.frames());
        let back = decode_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(back.sample_rate, 44_100);
        assert_eq!(back.data, buf.data);
    }

    #[test]
    fn pcm_roundtrip() {
        let buf = sine(48_000, 0.25, 220.0);
        for (fmt, tol) in [(WavFormat::Pcm16, 2.0 / 32768.0), (WavFormat::Pcm24, 1.0 / 8_000_000.0)] {
            let path = std::env::temp_dir().join(format!("mn_core_{fmt:?}.wav"));
            write_wav(&path, &buf, fmt).unwrap();
            let back = decode_file(&path).unwrap();
            std::fs::remove_file(&path).ok();
            assert_eq!(back.frames(), buf.frames());
            let max_err = back.data.iter().zip(&buf.data).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert!(max_err <= tol, "{fmt:?}: error {max_err}");
        }
    }

    fn sine(rate: u32, secs: f64, hz: f64) -> AudioBuffer {
        let n = (rate as f64 * secs) as usize;
        let mut data = Vec::with_capacity(n * 2);
        for i in 0..n {
            let s = (2.0 * std::f64::consts::PI * hz * i as f64 / rate as f64).sin() as f32 * 0.5;
            data.push(s);
            data.push(s);
        }
        AudioBuffer { sample_rate: rate, data }
    }

    #[test]
    fn wav_roundtrip() {
        let buf = sine(44_100, 0.5, 440.0);
        let path = std::env::temp_dir().join("mn_core_roundtrip.wav");
        write_wav_f32(&path, &buf).unwrap();
        let back = decode_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(back.sample_rate, 44_100);
        assert_eq!(back.frames(), buf.frames());
        let max_err = back.data.iter().zip(&buf.data).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(max_err < 1e-6, "error {max_err}");
    }

    #[test]
    fn resample_keeps_duration_and_pitch() {
        let buf = sine(44_100, 1.0, 1000.0);
        let out = resample(&buf, 48_000).unwrap();
        assert_eq!(out.frames(), 48_000);
        // El nivel RMS de un seno de amplitud 0.5 es ~0.3536.
        let mid = &out.data[20_000..76_000];
        let rms = (mid.iter().map(|s| s * s).sum::<f32>() / mid.len() as f32).sqrt();
        assert!((rms - 0.3536).abs() < 0.01, "rms {rms}");
        // Cruces por cero ≈ 2 * 1000 por segundo.
        let left: Vec<f32> = out.data.chunks_exact(2).map(|f| f[0]).collect();
        let crossings = left.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        assert!((crossings as i64 - 2000).abs() < 10, "cruces {crossings}");
    }
}
