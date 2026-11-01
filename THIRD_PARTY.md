# Componentes de terceros

MusicNova se distribuye bajo GPL-3.0-or-later. Todos los componentes siguientes tienen licencias
compatibles.

## Código incluido en el repositorio

| Componente | Ubicación | Licencia | Origen |
|---|---|---|---|
| Mel-Band RoFormer (`mel_band_roformer.py`, `attend.py`) | `ai-worker/musicnova_ai/vendor/msst/` | MIT | ZFTurbo/Music-Source-Separation-Training, commit `9cc76c03` |
| Beat This! (`beat_tracker.py`, `roformer.py`) | `ai-worker/musicnova_ai/vendor/beat_this/` | MIT | CPJKU/beat_this, commit `b95c8ab0` |
| Signalsmith Stretch y Signalsmith Linear (cabeceras C++) | `crates/mn-stretch/vendor/` | MIT | Signalsmith Audio, vía signalsmith-stretch-rs 0.1.3 |
| Envoltura C de Signalsmith Stretch | `crates/mn-stretch/vendor/wrapper/` | MIT | Colin Marc (signalsmith-stretch-rs) |

## Bibliotecas enlazadas o instaladas

| Componente | Uso | Licencia |
|---|---|---|
| Tauri, symphonia, rubato, cpal, rtrb | Aplicación y motor de audio | MIT / Apache-2.0 / MPL-2.0 |
| flacenc | Exportación FLAC | Apache-2.0 |
| LAME (vía mp3lame-encoder) | Exportación MP3 | LGPL-3.0 |
| PyTorch, torchaudio | Inferencia de IA | BSD-3-Clause |
| Demucs, openunmix, dora-search | Separación de stems | MIT |
| librosa, beartype, rotary-embedding-torch, einops, julius | Separación RoFormer | ISC / MIT |
| soundfile (libsndfile) | Lectura/escritura de audio en el worker | BSD-3-Clause / LGPL-2.1 |
| SciPy, soxr, numba | Análisis musical (acordes, secciones, remuestreo) | BSD-3-Clause / LGPL-2.1 / BSD-2-Clause |

## Descargas bajo demanda (no incluidas en el instalador)

Cada una se verifica con SHA-256 y la app muestra su licencia antes de descargarla.

| Componente | Licencia | Fuente |
|---|---|---|
| HTDemucs, HTDemucs 6s, HTDemucs FT | MIT | dl.fbaipublicfiles.com/demucs |
| Mel-Band RoFormer · Voz (Kimberley Jensen) | MIT | huggingface.co/KimberleyJSN/melbandroformer |
| FFmpeg 9.0 (compilación LGPL) | LGPL-2.1-or-later | github.com/BtbN/FFmpeg-Builds |
| Beat This! final0 y small0 (CPJKU, JKU Linz) | MIT (código y pesos) | cloud.cp.jku.at |
