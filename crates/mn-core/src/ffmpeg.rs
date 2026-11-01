//! Conversión con FFmpeg para vídeo y formatos que symphonia no decodifica.
//!
//! FFmpeg no se incluye con la aplicación: se instala bajo demanda desde el
//! gestor de modelos (`<datos>/models/ffmpeg/ffmpeg.exe`) o se usa el del PATH.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{CoreError, Result};

/// Extensiones que solo se pueden importar con FFmpeg.
pub const FFMPEG_ONLY_EXTENSIONS: &[&str] =
    &["mov", "avi", "wmv", "m4v", "flv", "3gp", "mpg", "mpeg", "ts", "opus", "wma", "ac3", "amr"];

/// Carpeta de datos de MusicNova (misma regla que el proceso de IA).
pub fn data_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("MUSICNOVA_DATA") {
        return Some(PathBuf::from(d));
    }
    if cfg!(windows) {
        std::env::var("LOCALAPPDATA").ok().map(|d| PathBuf::from(d).join("MusicNova"))
    } else {
        let base = std::env::var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .ok()?;
        Some(base.join("musicnova"))
    }
}

fn command(exe: &Path) -> Command {
    let mut cmd = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Busca FFmpeg: primero el instalado por MusicNova y después el del PATH.
pub fn locate() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Some(p) = data_dir().map(|d| d.join("models").join("ffmpeg").join(exe)) {
        if p.is_file() {
            return Some(p);
        }
    }
    let system = PathBuf::from(exe);
    let ok = command(&system)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    ok.then_some(system)
}

/// Extrae la primera pista de audio de `src` a WAV estéreo flotante a su frecuencia original.
pub fn convert_to_wav(ffmpeg: &Path, src: &Path, dest: &Path) -> Result<()> {
    let out = command(ffmpeg)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"])
        .arg(src)
        .args(["-vn", "-map", "0:a:0", "-ac", "2", "-c:a", "pcm_f32le"])
        .arg(dest)
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        let msg = msg.lines().last().unwrap_or("error desconocido");
        return Err(CoreError::Decode(format!("FFmpeg no pudo extraer el audio: {msg}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_wav_when_ffmpeg_is_available() {
        let Some(ffmpeg) = locate() else {
            eprintln!("FFmpeg no está instalado; se omite la prueba");
            return;
        };
        let dir = std::env::temp_dir();
        let src = dir.join("mn_ffmpeg_src.wav");
        let dest = dir.join("mn_ffmpeg_dest.wav");
        let buf = crate::audio::AudioBuffer { sample_rate: 22_050, data: vec![0.25; 22_050 * 2] };
        crate::audio::write_wav(&src, &buf, crate::audio::WavFormat::Pcm16).unwrap();
        convert_to_wav(&ffmpeg, &src, &dest).unwrap();
        let back = crate::audio::decode_file(&dest).unwrap();
        std::fs::remove_file(&src).ok();
        std::fs::remove_file(&dest).ok();
        assert_eq!(back.sample_rate, 22_050);
        assert_eq!(back.frames(), 22_050);
    }

    #[test]
    fn extracts_audio_from_video() {
        let Some(ffmpeg) = locate() else {
            eprintln!("FFmpeg no está instalado; se omite la prueba");
            return;
        };
        let dir = std::env::temp_dir();
        let video = dir.join("mn_ffmpeg_video.mp4");
        let dest = dir.join("mn_ffmpeg_video.wav");
        // Vídeo de 2 s con imagen de prueba y un tono de 440 Hz.
        let status = command(&ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=2"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2"])
            .args(["-c:v", "mpeg4", "-c:a", "aac", "-shortest"])
            .arg(&video)
            .status()
            .unwrap();
        assert!(status.success());
        convert_to_wav(&ffmpeg, &video, &dest).unwrap();
        let back = crate::audio::decode_file(&dest).unwrap();
        std::fs::remove_file(&video).ok();
        std::fs::remove_file(&dest).ok();
        assert_eq!(back.sample_rate, 48_000);
        assert!((back.duration_secs() - 2.0).abs() < 0.1, "duración {}", back.duration_secs());
        let left: Vec<f32> = back.data.chunks_exact(2).map(|f| f[0]).collect();
        let crossings = left.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as f64;
        let hz = crossings / 2.0 / back.duration_secs();
        assert!((hz - 440.0).abs() < 10.0, "frecuencia {hz}");
    }
}
