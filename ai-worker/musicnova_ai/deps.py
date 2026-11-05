"""Dependencias de Python bajo demanda.

En la aplicación instalada, el proceso de IA corre en un entorno propio
(`%LOCALAPPDATA%\\MusicNova\\envs\\worker`) que empieza vacío: cada función
instala lo que necesita la primera vez que se usa (o al descargar su modelo).
Las versiones están fijadas a las probadas en desarrollo. PyTorch se elige según
la GPU: con una NVIDIA y su driver, la variante CUDA más nueva que admite; si
no, la de CPU.

Fuera de ese entorno (desarrollo) no se instala nada: si falta algo, la tarea
lo dice, como siempre.
"""

from __future__ import annotations

import importlib
import importlib.util
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Callable, Iterable

from . import system

Report = Callable[[float, str], None]

TORCH_VERSION = "2.11.0"
TORCH_INDEX = "https://download.pytorch.org/whl/{variant}"

#: Módulo importable → requisito de pip (versiones probadas en desarrollo).
REQUIREMENTS: dict[str, str] = {
    "numpy": "numpy==2.5.4",
    "scipy": "scipy==1.18.1",
    "soundfile": "soundfile==0.14.0",
    "librosa": "librosa==1.0.0",
    "soxr": "soxr==1.1.0",
    "einops": "einops==0.8.2",
    "rotary_embedding_torch": "rotary-embedding-torch==0.9.1",
    "demucs": "demucs==4.0.1",
    "julius": "julius==0.2.8",
    "lameenc": "lameenc==1.9.0",
    "beartype": "beartype==0.23.1",
    "onnxruntime": "onnxruntime==1.31.1",
    "faster_whisper": "faster-whisper==1.2.1",
    "ctranslate2": "ctranslate2==4.8.4",
    "matchering": "matchering==2.0.6",
    "torchcrepe": "torchcrepe==0.0.24",
}
TORCH_MODULES = ("torch", "torchaudio")


def managed() -> bool:
    """Si este proceso corre en el entorno que gestiona la aplicación."""
    return os.environ.get("MUSICNOVA_MANAGED_ENV") == "1"


def missing(modules: Iterable[str]) -> list[str]:
    importlib.invalidate_caches()
    return [m for m in modules if importlib.util.find_spec(m) is None]


def torch_variant() -> str:
    """cu130 / cu128 / cu126 según el driver NVIDIA; cpu sin GPU NVIDIA."""
    forced = os.environ.get("MUSICNOVA_TORCH")
    if forced:
        return forced
    best = 0
    for g in system.info().get("gpus", []):
        try:
            best = max(best, int(str(g.get("driver", "0")).split(".")[0]))
        except ValueError:
            pass
    # Driver mínimo de cada CUDA en Windows.
    if best >= 580:
        return "cu130"
    if best >= 570:
        return "cu128"
    if best >= 560:
        return "cu126"
    return "cpu"


def _pip(args: list[str], report: Report, label: str, start: float, end: float) -> None:
    cmd = [sys.executable, "-m", "pip", "install", "--disable-pip-version-check", "--no-input", "--progress-bar", "off", *args]
    report(start, label)
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    tail: list[str] = []
    seen = 0
    assert proc.stdout is not None
    for line in proc.stdout:
        line = line.rstrip()
        if not line:
            continue
        tail = (tail + [line])[-25:]
        # Avance aproximado: cada paquete que se descarga o instala mueve la barra.
        if line.startswith(("Collecting", "Downloading", "Installing")):
            seen += 1
            report(start + (end - start) * min(0.95, seen / (seen + 8)), label)
    if proc.wait() != 0:
        raise RuntimeError(f"pip falló ({label}):\n" + "\n".join(tail))
    report(end, label)


def ensure(modules: Iterable[str], report: Report, start: float = 0.0, end: float = 0.3) -> bool:
    """Instala lo que falte de `modules` (solo en el entorno gestionado).
    Devuelve si instaló algo."""
    need = missing(modules)
    if not need or not managed():
        return False
    unknown = [m for m in need if m not in REQUIREMENTS and m not in TORCH_MODULES]
    if unknown:
        raise RuntimeError("faltan dependencias de Python: " + ", ".join(unknown))
    variant = torch_variant()
    pins = [f"{m}=={TORCH_VERSION}+{variant}" for m in TORCH_MODULES]
    with tempfile.TemporaryDirectory() as tmp:
        # Restricción: ningún paquete puede cambiar el PyTorch elegido por el de PyPI.
        constraints = Path(tmp) / "torch-constraints.txt"
        constraints.write_text("\n".join(pins) + "\n", encoding="utf-8")
        mid = start + (end - start) * 0.6
        if any(m in TORCH_MODULES for m in need) or (missing(TORCH_MODULES) and _needs_torch(need)):
            label = "instalando PyTorch (CUDA)" if variant != "cpu" else "instalando PyTorch (CPU)"
            _pip(["--index-url", TORCH_INDEX.format(variant=variant), *pins], report, label, start, mid)
        else:
            mid = start
        reqs = sorted({REQUIREMENTS[m] for m in need if m in REQUIREMENTS})
        if "faster_whisper" in need and variant != "cpu":
            reqs.append("nvidia-cublas-cu12==12.9.2.10")
        if reqs:
            _pip(["--constraint", str(constraints), "--extra-index-url", TORCH_INDEX.format(variant=variant), *reqs], report, "instalando dependencias", mid, end)
    importlib.invalidate_caches()
    still = missing(modules)
    if still:
        raise RuntimeError("faltan dependencias de Python: " + ", ".join(still))
    return True


def _needs_torch(modules: Iterable[str]) -> bool:
    """Paquetes que arrastran PyTorch: se instala antes la variante elegida."""
    return any(m in {"demucs", "torchcrepe", "rotary_embedding_torch", "julius"} for m in modules)
