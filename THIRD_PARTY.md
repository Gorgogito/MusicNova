# Componentes de terceros

MusicNova se distribuye bajo GPL-3.0-or-later. Todos los componentes siguientes tienen licencias
compatibles.

## Código incluido en el repositorio

| Componente | Ubicación | Licencia | Origen |
|---|---|---|---|
| Mel-Band RoFormer (`mel_band_roformer.py`, `attend.py`) | `ai-worker/musicnova_ai/vendor/msst/` | MIT | ZFTurbo/Music-Source-Separation-Training, commit `9cc76c03` |
| Beat This! (`beat_tracker.py`, `roformer.py`) | `ai-worker/musicnova_ai/vendor/beat_this/` | MIT | CPJKU/beat_this, commit `b95c8ab0` |
| Basic Pitch (inferencia y creación de notas, adaptadas) y su modelo ONNX ICASSP 2022 (`nmp.onnx`) | `ai-worker/musicnova_ai/vendor/basic_pitch/` | Apache-2.0 | spotify/basic-pitch, commit `fa5997af` (cambios descritos en el archivo) |
| Arquitectura Cnn10/Cnn14 de PANNs (reescrita con log-mel en torch) | `ai-worker/musicnova_ai/vendor/panns/` | MIT | qiuqiangkong/audioset_tagging_cnn |
| Lista de clases de AudioSet (`class_labels_indices.csv`) | `ai-worker/musicnova_ai/vendor/panns/` | CC BY 4.0 | Google AudioSet, vía audioset_tagging_cnn |
| DeepFilterNet3: procesado de señal portado a numpy (STFT Vorbis, bandas ERB, normalización, máscara y filtrado profundo) | `ai-worker/musicnova_ai/vendor/deepfilternet/` | MIT o Apache-2.0 | Rikorose/DeepFilterNet (libDF, `deepfilternet3.py`, `multiframe.py`), commit `84d57ec2` |
| Signalsmith Stretch y Signalsmith Linear (cabeceras C++) | `crates/mn-stretch/vendor/` | MIT | Signalsmith Audio, vía signalsmith-stretch-rs 0.1.3 |
| Envoltura C de Signalsmith Stretch (con `set_formant_base` corregido) | `crates/mn-stretch/vendor/wrapper/` | MIT | Colin Marc (signalsmith-stretch-rs) |
| Plugin VST3 de ganancia de ejemplo (solo pruebas, adaptado con estado) | `crates/mn-testplugin/src/vst3.rs` | MIT OR Apache-2.0 | Micah Johnston, vst3-rs 0.3.0 `examples/gain.rs` |

Algoritmos implementados desde cero en `crates/mn-fx`: filtros del «Audio EQ Cookbook» (R. Bristow-Johnson), reverb según Freeverb (Jezar at Dreampoint, dominio público) y medición de sonoridad según ITU-R BS.1770-4 / EBU R128 (derivación de la ponderación K como en libebur128).

## Bibliotecas enlazadas o instaladas

| Componente | Uso | Licencia |
|---|---|---|
| Tauri, symphonia, rubato, cpal, rtrb | Aplicación y motor de audio | MIT / Apache-2.0 / MPL-2.0 |
| flacenc | Exportación FLAC | Apache-2.0 |
| realfft (rustfft) | Reducción de ruido constante | MIT / Apache-2.0 |
| clap-sys | Alojamiento de plugins CLAP | MIT / Apache-2.0 |
| vst3 (vst3-rs, enlaces del VST 3 SDK 3.8), com-scrape-types | Alojamiento de plugins VST3 | MIT / Apache-2.0 (el SDK de Steinberg es MIT desde la versión 3.8) |
| libloading, base64, windows-sys | Carga de plugins, estado y ventanas de sus interfaces | ISC / MIT / MIT o Apache-2.0 |
| LAME (vía mp3lame-encoder) | Exportación MP3 | LGPL-3.0 |
| PyTorch, torchaudio | Inferencia de IA | BSD-3-Clause |
| Demucs, openunmix, dora-search | Separación de stems | MIT |
| librosa, beartype, rotary-embedding-torch, einops, julius | Separación RoFormer | ISC / MIT |
| soundfile (libsndfile) | Lectura/escritura de audio en el worker | BSD-3-Clause / LGPL-2.1 |
| SciPy, soxr, numba | Análisis musical (acordes, secciones, remuestreo) | BSD-3-Clause / LGPL-2.1 / BSD-2-Clause |
| faster-whisper (incluye el VAD Silero), CTranslate2, tokenizers | Transcripción de letras | MIT / MIT / Apache-2.0 |
| ONNX Runtime | VAD de faster-whisper y modelo de Basic Pitch | MIT |
| ACE-Step 1.5 (código, en un entorno de Python aparte con PyTorch 2.7.1, transformers, diffusers y accelerate) | Generación de acompañamientos; se descarga desde la aplicación en un commit fijo | MIT (ACE-Step); Apache-2.0 / BSD (dependencias) |
| torchcrepe (Max Morrison) con los pesos de CREPE «full» y «tiny» incluidos en el paquete (Kim et al., 2018) | Tono de la voz y curvas de tono de las notas | MIT (código y pesos) |
| Matchering 2.0 (sergree) | Master por referencia | GPL-3.0 |
| statsmodels, pandas, resampy | Dependencias de Matchering | BSD-3-Clause / BSD-3-Clause / ISC |
| NVIDIA cuBLAS para CUDA 12 (`nvidia-cublas-cu12`) | Whisper en GPU (solo se instala con pip; no se incluye en el repositorio) | Licencia NVIDIA de redistribución de CUDA |

## Descargas bajo demanda (no incluidas en el instalador)

Cada una se verifica con SHA-256 y la app muestra su licencia antes de descargarla.

| Componente | Licencia | Fuente |
|---|---|---|
| HTDemucs, HTDemucs 6s, HTDemucs FT | MIT | dl.fbaipublicfiles.com/demucs |
| Mel-Band RoFormer · Voz (Kimberley Jensen) | MIT | huggingface.co/KimberleyJSN/melbandroformer |
| FFmpeg 9.0 (compilación LGPL) | LGPL-2.1-or-later | github.com/BtbN/FFmpeg-Builds |
| Beat This! final0 y small0 (CPJKU, JKU Linz) | MIT (código y pesos) | cloud.cp.jku.at |
| Whisper large-v3, large-v3 turbo y small (OpenAI) en formato CTranslate2 | MIT | huggingface.co/Systran, huggingface.co/deepdml |
| PANNs Cnn14 y Cnn10 (Kong et al., 2020) | CC BY 4.0 (pesos) | zenodo.org/records/3987831 |
| DeepFilterNet3 (grafos ONNX) | MIT o Apache-2.0 | github.com/Rikorose/DeepFilterNet (commit `84d57ec2`) |
| ACE-Step 1.5 base (DiT), VAE | MIT | huggingface.co/ACE-Step/acestep-v15-base, huggingface.co/ACE-Step/Ace-Step1.5 |
| Qwen3-Embedding-0.6B (codificador de texto de ACE-Step) | Apache-2.0 | huggingface.co/ACE-Step/Ace-Step1.5 (Alibaba Qwen) |

## Datos usados solo para medir (no se incluyen ni se distribuyen)

Los descarga `ai-worker/eval/fetch.py` en una carpeta local para reproducir las
medidas de `docs/TRANSCRIPCION.md`.

| Conjunto | Licencia | Fuente |
|---|---|---|
| Vocadito (voces solistas con f0 y notas anotadas) | CC BY 4.0 | zenodo.org/records/5578807 |
| MDB Drums (batería de 23 canciones de MedleyDB, golpes anotados) | CC BY-NC-SA 4.0 | github.com/CarlSouthall/MDBDrums |
| IRMAS, partición de prueba (instrumentos predominantes anotados) | CC BY-NC-SA 3.0 | zenodo.org/records/1290750 |
