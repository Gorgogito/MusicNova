# MusicNova

Estudio musical de escritorio con IA **local, gratuita y de código abierto**: separación de stems,
acordes, tonalidad, tempo, letras, herramientas de práctica, edición multipista y generación musical.
Sin suscripciones ni nube: todo se procesa en tu equipo.

Licencia: **GPL-3.0-or-later**. Cada modelo de IA muestra su propia licencia antes de descargarse.

## Arquitectura

```
app/            Interfaz (React + TypeScript) y shell de escritorio (Tauri 2, app/src-tauri)
crates/mn-core    Decodificación, remuestreo, formas de onda y formato de proyecto
crates/mn-engine  Motor de audio en tiempo real (cpal, mezcla sin bloqueos, bucle, metrónomo)
crates/mn-stretch Tempo y tono en tiempo real (Signalsmith Stretch, binding sin bindgen)
ai-worker/      Proceso de IA en Python (JSON-RPC por stdin/stdout, trabajos cancelables,
                gestor de modelos con manifiestos TOML y verificación SHA-256)
docs/PLAN.md    Análisis, decisiones y plan por fases
```

## Desarrollo (Windows)

Requisitos: Rust estable con MSVC (Visual Studio Build Tools), Node.js 20+, Python 3.11+ y WebView2.

```powershell
# 1. Proceso de IA
cd ai-worker
python -m venv .venv
.venv\Scripts\python -m pip install -e ".[dev]"
# PyTorch (CUDA 13; usa /whl/cpu si no tienes GPU NVIDIA) y dependencias de separación
.venv\Scripts\python -m pip install torch==2.11.0 torchaudio==2.11.0 --index-url https://download.pytorch.org/whl/cu130
.venv\Scripts\python -m pip install -e ".[dev,separation,analysis]"
.venv\Scripts\python -m pytest

# 2. Interfaz y app
cd ..\app
npm install
npm run tauri dev
```

Pruebas del núcleo y el motor: `cargo test --workspace`.
Probar la salida de audio: `cargo run -p mn-engine --example play [archivo]`.

Variables opcionales:

| Variable | Uso |
|---|---|
| `MUSICNOVA_PYTHON` | Intérprete de Python para el worker (por defecto `ai-worker/.venv`) |
| `MUSICNOVA_WORKER_DIR` | Carpeta del worker |
| `MUSICNOVA_DATA` | Carpeta de datos y modelos (por defecto `%LOCALAPPDATA%\MusicNova`) |

## Atajos de teclado (Estudio)

| Tecla | Acción |
|---|---|
| Espacio | Reproducir / pausar (con cuenta previa si está activada) |
| Inicio | Ir al inicio (o al inicio del bucle) |
| ← / → | Retroceder / avanzar 5 s (Mayús: 1 s) |
| [ / ] | Inicio / fin del bucle en la posición actual |
| L | Activar o desactivar el bucle |
| M | Añadir marcador |
| − / = | Velocidad −5 % / +5 % |
| , / . | Tono −1 / +1 semitono |
| K | Metrónomo |

## Añadir un modelo

Crea un manifiesto TOML en `ai-worker/musicnova_ai/manifests/` (o en `%LOCALAPPDATA%\MusicNova\manifests`):

```toml
id = "mi_modelo"
name = "Mi modelo"
task = "separation"          # separation | chords | beats | key | lyrics | transcription | generation
family = "demucs"
description = "..."
license = "MIT"
license_url = "https://..."
commercial_use = true

[[files]]
name = "pesos.th"
url = "https://..."
sha256 = "<hash completo o prefijo de al menos 8 hex>"
size = 123456
```
