"""Entorno aparte para ACE-Step 1.5 (generación de acompañamientos).

ACE-Step pide torch 2.7.1 en Windows y el worker está fijado en torch 2.11, así
que vive en su propio venv (`<datos>/envs/acestep`) con las versiones exactas
de su `uv.lock`, y el código de ACE-Step (MIT) se descarga en un commit fijo y
se verifica con SHA-256. El worker lo lanza como subproceso con
`acestep_runner.py`.
"""

from __future__ import annotations

import hashlib
import io
import os
import shutil
import subprocess
import sys
import urllib.request
import zipfile
from pathlib import Path

from .jobs import Job
from .models import USER_AGENT, data_dir

ACESTEP_COMMIT = "ca1e85fe9430179831e6bc6be790c332190a3866"
ACESTEP_ZIP = f"https://github.com/ACE-Step/ACE-Step-1.5/archive/{ACESTEP_COMMIT}.zip"
ACESTEP_ZIP_SHA256 = "108e45dffc7eb62af98d781c4159aa63a2b7a241186734e3e065279da7fc198d"

TORCH_INDEX = "https://download.pytorch.org/whl/cu128"
TORCH = ["torch==2.7.1+cu128", "torchaudio==2.7.1+cu128"]
# Solo lo que usa la inferencia (sin interfaz web, servidor ni entrenamiento),
# con las versiones que resolvió el uv.lock de ese commit.
DEPS = [
    "numpy==2.3.5",
    "transformers==4.57.6",
    "tokenizers==0.22.2",
    "diffusers==0.37.1",
    "accelerate==1.12.0",
    "huggingface-hub==0.36.0",
    "safetensors==0.7.0",
    "peft==0.18.1",
    "loguru==0.7.3",
    "einops==0.8.2",
    "numba==0.63.1",
    "scipy==1.17.0",
    "soundfile==0.13.1",
    "vector-quantize-pytorch==1.27.20",
    "pytorch-wavelets==1.3.0",
    "pywavelets==1.9.0",
    "toml==0.10.2",
    "tqdm==4.67.1",
    "setuptools==71.1.0",
]


def env_dir() -> Path:
    return data_dir() / "envs" / "acestep"


def python_exe() -> Path:
    d = env_dir()
    return d / "Scripts" / "python.exe" if os.name == "nt" else d / "bin" / "python"


def source_dir() -> Path:
    """Carpeta que contiene el paquete `acestep` (se añade a PYTHONPATH)."""
    return env_dir() / "src"


def is_ready() -> bool:
    return python_exe().is_file() and (source_dir() / "acestep" / "inference.py").is_file() and (env_dir() / "READY").is_file()


def _base_python() -> str:
    """Intérprete con el que crear el venv: el base del worker (no el de su venv)."""
    return getattr(sys, "_base_executable", None) or sys.executable


def _pip(args: list[str], job: Job, label: str, start: float, end: float) -> None:
    cmd = [str(python_exe()), "-m", "pip", "install", "--disable-pip-version-check", "--no-input", *args]
    job.report(start, label, force=True)
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace")
    tail: list[str] = []
    assert proc.stdout is not None
    for line in proc.stdout:
        tail = (tail + [line.rstrip()])[-20:]
        try:
            job.check()
        except Exception:
            proc.kill()
            raise
        if line.startswith(("Collecting", "Downloading", "Installing")):
            job.report(start + (end - start) * 0.5, f"{label}: {line.strip()[:80]}")
    if proc.wait() != 0:
        raise RuntimeError(f"pip falló ({label}):\n" + "\n".join(tail))
    job.report(end, label, force=True)


def install(job: Job) -> dict[str, str]:
    """Crea (o completa) el entorno. Descarga unos 3–4 GB la primera vez."""
    d = env_dir()
    marker = d / "READY"
    if marker.exists():
        marker.unlink()
    if not python_exe().is_file():
        job.report(0.0, "creando el entorno de Python", force=True)
        d.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run([_base_python(), "-m", "venv", str(d)], check=True)
    _pip(["--index-url", TORCH_INDEX, *TORCH], job, "instalando PyTorch 2.7.1 (CUDA 12.8)", 0.02, 0.6)
    _pip(DEPS, job, "instalando dependencias de ACE-Step", 0.6, 0.85)

    job.report(0.86, "descargando ACE-Step 1.5", force=True)
    req = urllib.request.Request(ACESTEP_ZIP, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=120) as r:
        data = r.read()
    digest = hashlib.sha256(data).hexdigest()
    if digest != ACESTEP_ZIP_SHA256:
        raise ValueError(f"la suma SHA-256 del código de ACE-Step no coincide ({digest})")
    src = source_dir()
    if src.exists():
        shutil.rmtree(src)
    src.mkdir(parents=True)
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        prefix = z.namelist()[0]
        for name in z.namelist():
            rel = name[len(prefix) :]
            # Solo el paquete y la licencia; nada fuera de la carpeta.
            if not (rel.startswith("acestep/") or rel == "LICENSE") or ".." in rel.split("/"):
                continue
            target = src / rel
            if name.endswith("/"):
                target.mkdir(parents=True, exist_ok=True)
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            with z.open(name) as fsrc, open(target, "wb") as fdst:
                shutil.copyfileobj(fsrc, fdst)
    job.report(0.95, "comprobando el entorno", force=True)
    check = subprocess.run(
        [str(python_exe()), "-c", "import torch, transformers, diffusers; print(torch.__version__, torch.cuda.is_available())"],
        capture_output=True,
        text=True,
        env={**os.environ, "PYTHONPATH": str(src)},
    )
    if check.returncode != 0:
        raise RuntimeError("el entorno de ACE-Step no arranca:\n" + check.stderr[-2000:])
    marker.write_text(ACESTEP_COMMIT, encoding="utf-8")
    job.report(1.0, "listo", force=True)
    return {"python": str(python_exe()), "torch": check.stdout.strip()}
