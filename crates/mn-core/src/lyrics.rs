//! Letras sincronizadas (`analysis/lyrics.json`) y su exportación a LRC, SRT y texto.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{CoreError, Result};

pub const LYRICS_FILE: &str = "analysis/lyrics.json";
pub const LYRICS_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LyricWord {
    pub start: f64,
    pub end: f64,
    pub text: String,
    #[serde(default = "one")]
    pub prob: f64,
}

fn one() -> f64 {
    1.0
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LyricLine {
    pub start: f64,
    pub end: f64,
    pub text: String,
    #[serde(default)]
    pub words: Vec<LyricWord>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Lyrics {
    pub version: u32,
    /// Código ISO 639-1 detectado o elegido ("es", "en"...).
    pub language: String,
    #[serde(default)]
    pub language_probability: f64,
    pub model_id: String,
    /// "vocals" si se transcribió el stem de voz, "mix" si la mezcla.
    pub source: String,
    pub duration: f64,
    pub lines: Vec<LyricLine>,
    #[serde(default)]
    pub seconds: f64,
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub warning: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LyricsFormat {
    /// LRC por línea (compatible con cualquier reproductor).
    Lrc,
    /// LRC mejorado con marcas por palabra (`<mm:ss.xx>`), para karaoke.
    LrcWords,
    Srt,
    Txt,
}

impl LyricsFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Lrc | Self::LrcWords => "lrc",
            Self::Srt => "srt",
            Self::Txt => "txt",
        }
    }
}

impl Lyrics {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(LYRICS_FILE)
    }

    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let path = Self::path(dir);
        if !path.is_file() {
            return Ok(None);
        }
        let l: Lyrics = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        l.validate()?;
        Ok(Some(l))
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        self.validate()?;
        let path = Self::path(dir);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string(self)?)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if self.version > LYRICS_VERSION {
            return Err(CoreError::Format(format!(
                "las letras usan la versión {} y esta aplicación solo admite hasta la {}",
                self.version, LYRICS_VERSION
            )));
        }
        for l in &self.lines {
            let times_ok = |a: f64, b: f64| a.is_finite() && b.is_finite() && a >= 0.0 && b >= a;
            if !times_ok(l.start, l.end) || l.words.iter().any(|w| !times_ok(w.start, w.end)) {
                return Err(CoreError::Format(format!("marcas de tiempo inválidas en «{}»", l.text)));
            }
        }
        Ok(())
    }

    /// Ordena las líneas y ajusta su inicio y fin a sus palabras.
    pub fn normalize(&mut self) {
        for l in &mut self.lines {
            l.text = l.text.split_whitespace().collect::<Vec<_>>().join(" ");
            l.words.sort_by(|a, b| a.start.total_cmp(&b.start));
            if let (Some(first), Some(last)) = (l.words.first(), l.words.last()) {
                l.start = first.start;
                l.end = last.end.max(first.start);
            }
        }
        self.lines.retain(|l| !l.text.is_empty());
        self.lines.sort_by(|a, b| a.start.total_cmp(&b.start));
    }

    pub fn export(&self, format: LyricsFormat, title: &str) -> String {
        match format {
            LyricsFormat::Lrc => self.to_lrc(title, false),
            LyricsFormat::LrcWords => self.to_lrc(title, true),
            LyricsFormat::Srt => self.to_srt(),
            LyricsFormat::Txt => self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n") + "\n",
        }
    }

    fn to_lrc(&self, title: &str, words: bool) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "[ti:{}]", title.replace(['[', ']'], ""));
        let _ = writeln!(out, "[length:{}]", &lrc_time(self.duration)[..5]);
        let _ = writeln!(out, "[la:{}]", self.language);
        let _ = writeln!(out, "[re:MusicNova]");
        for l in &self.lines {
            if words && !l.words.is_empty() {
                let body: Vec<String> = l.words.iter().map(|w| format!("<{}>{}", lrc_time(w.start), w.text)).collect();
                let _ = writeln!(out, "[{}]{} <{}>", lrc_time(l.start), body.join(" "), lrc_time(l.end));
            } else {
                let _ = writeln!(out, "[{}]{}", lrc_time(l.start), l.text);
            }
        }
        out
    }

    fn to_srt(&self) -> String {
        let mut out = String::new();
        for (i, l) in self.lines.iter().enumerate() {
            // Un subtítulo dura al menos 1 s sin pisar al siguiente.
            let next = self.lines.get(i + 1).map_or(f64::INFINITY, |n| n.start);
            let end = l.end.max(l.start + 1.0).min(next).max(l.start + 0.1);
            let _ = write!(out, "{}\n{} --> {}\n{}\n\n", i + 1, srt_time(l.start), srt_time(end), l.text);
        }
        out
    }
}

/// `mm:ss.xx` (centésimas), como usa LRC.
fn lrc_time(t: f64) -> String {
    let cs = (t.max(0.0) * 100.0).round() as u64;
    format!("{:02}:{:02}.{:02}", cs / 6000, (cs / 100) % 60, cs % 100)
}

/// `hh:mm:ss,mmm`, como usa SRT.
fn srt_time(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(start: f64, end: f64, text: &str) -> LyricWord {
        LyricWord { start, end, text: text.into(), prob: 0.9 }
    }

    fn sample() -> Lyrics {
        Lyrics {
            version: 1,
            language: "es".into(),
            language_probability: 0.98,
            model_id: "whisper_large_v3_turbo".into(),
            source: "vocals".into(),
            duration: 125.4,
            lines: vec![
                LyricLine {
                    start: 12.5,
                    end: 14.0,
                    text: "Hola mundo".into(),
                    words: vec![word(12.5, 13.0, "Hola"), word(13.2, 14.0, "mundo")],
                },
                LyricLine { start: 14.5, end: 14.9, text: "otra".into(), words: vec![word(14.5, 14.9, "otra")] },
            ],
            seconds: 3.0,
            device: "cuda".into(),
            warning: None,
        }
    }

    #[test]
    fn lrc_line_and_word_level() {
        let l = sample();
        let lrc = l.export(LyricsFormat::Lrc, "Mi [canción]");
        assert!(lrc.starts_with("[ti:Mi canción]\n[length:02:05]\n[la:es]\n[re:MusicNova]\n"));
        assert!(lrc.contains("[00:12.50]Hola mundo\n[00:14.50]otra\n"));
        let karaoke = l.export(LyricsFormat::LrcWords, "x");
        assert!(karaoke.contains("[00:12.50]<00:12.50>Hola <00:13.20>mundo <00:14.00>\n"), "{karaoke}");
    }

    #[test]
    fn srt_has_min_duration_without_overlap() {
        let srt = sample().export(LyricsFormat::Srt, "x");
        assert_eq!(
            srt,
            "1\n00:00:12,500 --> 00:00:14,000\nHola mundo\n\n2\n00:00:14,500 --> 00:00:15,500\notra\n\n"
        );
        let mut l = sample();
        l.lines[0].end = 12.6; // < 1 s, pero el siguiente empieza en 14,5
        assert!(l.export(LyricsFormat::Srt, "x").contains("00:00:12,500 --> 00:00:13,500"));
    }

    #[test]
    fn long_times_format() {
        assert_eq!(lrc_time(3725.456), "62:05.46");
        assert_eq!(srt_time(3725.4567), "01:02:05,457");
    }

    #[test]
    fn normalize_sorts_and_fits_words() {
        let mut l = sample();
        l.lines.swap(0, 1);
        l.lines[1].start = 0.0;
        l.lines[1].text = "  Hola   mundo ".into();
        l.lines.push(LyricLine { start: 1.0, end: 1.0, text: "  ".into(), words: vec![] });
        l.normalize();
        assert_eq!(l.lines.len(), 2);
        assert_eq!((l.lines[0].start, l.lines[0].text.as_str()), (12.5, "Hola mundo"));
    }

    #[test]
    fn roundtrip_and_python_format() {
        let dir = std::env::temp_dir().join(format!("mn_ly_{}", uuid::Uuid::new_v4()));
        let l = sample();
        l.save(&dir).unwrap();
        assert_eq!(Lyrics::load(&dir).unwrap(), Some(l));
        std::fs::remove_dir_all(&dir).ok();
        let json = r#"{"version":1,"language":"en","language_probability":0.94,"model_id":"whisper_small",
            "source":"mix","duration":10.0,"lines":[{"start":1.0,"end":2.0,"text":"Hi there",
            "words":[{"start":1.0,"end":1.4,"text":"Hi","prob":0.9},{"start":1.5,"end":2.0,"text":"there","prob":0.8}]}],
            "seconds":1.2,"device":"cpu","warning":null}"#;
        let l: Lyrics = serde_json::from_str(json).unwrap();
        l.validate().unwrap();
        assert_eq!(l.lines[0].words.len(), 2);
    }

    #[test]
    fn rejects_bad_times() {
        let mut l = sample();
        l.lines[0].words[0].end = 1.0;
        assert!(l.validate().is_err());
    }
}
