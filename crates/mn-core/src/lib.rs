//! Núcleo de MusicNova: decodificación de audio, remuestreo, formas de onda y
//! formato de proyecto. No depende de la interfaz ni del dispositivo de audio.

pub mod analysis;
pub mod automation;
pub mod audio;
pub mod export;
pub mod ffmpeg;
pub mod lyrics;
pub mod master;
pub mod mix;
pub mod peaks;
pub mod project;
pub mod transcription;
pub mod tuning;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("no se pudo decodificar el audio: {0}")]
    Decode(String),
    #[error("error de remuestreo: {0}")]
    Resample(String),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Format(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
