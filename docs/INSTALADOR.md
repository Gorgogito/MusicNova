# Instalador de Windows

`MusicNova_<versión>_x64-setup.exe` (NSIS, unos 17 MB). Se instala por usuario, sin permisos de
administrador, en `%LOCALAPPDATA%\Programs\MusicNova`. El asistente está en español e inglés. Sin
certificado de firma de código por ahora: Windows SmartScreen avisa al ejecutarlo («Más
información» → «Ejecutar de todas formas»).

## Qué lleva y qué se descarga después

| Parte | Dónde | Cuándo |
|---|---|---|
| App (Rust + interfaz) | `Programs\MusicNova\musicnova.exe` | En el instalador |
| Python 3.12 (python-build-standalone, con pip y venv) | `Programs\MusicNova\python` | En el instalador |
| Código del proceso de IA | `Programs\MusicNova\ai-worker` | En el instalador |
| Entorno de Python del proceso de IA | `%LOCALAPPDATA%\MusicNova\envs\worker` | Primer arranque (unos 6 s, sin red) |
| Paquetes de cada función (numpy, librosa, PyTorch…) | En ese entorno | La primera vez que se usa la función o se descarga su modelo |
| Modelos de IA | `%LOCALAPPDATA%\MusicNova\models` | Desde «Modelos IA», como siempre |

Los paquetes tienen las versiones probadas en desarrollo (`ai-worker/musicnova_ai/deps.py`).
PyTorch se elige según la GPU: con una NVIDIA, la variante CUDA más nueva que admite su driver
(cu130 con driver 580 o más, cu128 con 570, cu126 con 560); si no, la de CPU. Una restricción de pip
impide que otro paquete la sustituya por la de PyPI. `MUSICNOVA_TORCH=cpu` fuerza la de CPU.

El programa y los datos van en carpetas distintas: actualizar o desinstalar no toca los modelos ni
los entornos. Al desinstalar se quita toda la carpeta del programa (también las cachés que Python
haya escrito); los datos se conservan.

## Compilar el instalador

```
cd app
npx tauri build
```

`beforeBuildCommand` compila la interfaz y ejecuta `scripts/prepare-bundle.mjs`. Ese script
descarga el Python incluido (`scripts/fetch-python.mjs`, versión fijada y verificada con el
SHA-256 de la release) en `src-tauri/resources/python` (no está en git) y limpia las cachés de
bytecode. La primera vez, Tauri descarga NSIS. El resultado queda en
`target/release/bundle/nsis/`.

Ganchos del instalador en `app/src-tauri/nsis/hooks.nsh`:
- antes de instalar, la carpeta por defecto de Tauri (`%LOCALAPPDATA%\MusicNova`, que es la de
  datos) se cambia por `%LOCALAPPDATA%\Programs\MusicNova`;
- después de desinstalar se borran enteras `python\` y `ai-worker\`.

## Desarrollo

En `tauri dev` (compilación de depuración) el proceso de IA usa `ai-worker/.venv` y no instala
nada. `MUSICNOVA_FORCE_MANAGED=1` prueba en desarrollo el entorno gestionado de la app instalada.
`MUSICNOVA_WORKER_DIR` y `MUSICNOVA_PYTHON` siguen permitiendo usar otro código u otro Python.

## Verificación (noviembre de 2026)

- Instalación silenciosa (`/S`) en 4 s; entrada en «Aplicaciones instaladas»; 73 MB en disco.
- Primer arranque: entorno creado y proceso de IA listo con el Python incluido.
- Primer uso de la transcripción de batería: instaló numpy, scipy, soundfile y librosa en 39 s y
  transcribió. Con el entorno ya preparado, la siguiente vez empieza directamente.
- PyTorch y demucs en un entorno nuevo: PyTorch de su índice y demucs de PyPI, sin que la
  restricción deje cambiar PyTorch (`2.11.0+cpu` tras instalar demucs), en 86 s con la variante de CPU.
- Desinstalación silenciosa: carpeta del programa y entrada del registro eliminadas; modelos
  (9,4 GB) y entornos intactos.
