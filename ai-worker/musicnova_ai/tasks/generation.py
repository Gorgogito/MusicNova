"""IA generativa: pistas de acompañamiento con ACE-Step 1.5 (tarea «lego»).

ACE-Step corre en su propio entorno (ver `acestep_env`) y se lanza como
subproceso con `acestep_runner.py`, que informa del avance por stdout.
"""

from __future__ import annotations

import json
import os
import queue
import subprocess
import tempfile
import threading
import time
from pathlib import Path
from typing import Any

from .. import acestep_env
from ..jobs import Job
from ..system import best_device
from . import Task, register

MODEL_ID = "acestep_v15_base"
TRACKS = ("drums", "bass", "guitar", "keyboard", "strings", "synth", "percussion", "brass", "woodwinds", "backing_vocals", "vocals", "fx")
# Pasos de difusión del modelo base: la documentación recomienda 32–64.
DEFAULT_STEPS = 32
RUNNER = Path(__file__).resolve().parent.parent / "acestep_runner.py"


def parse_line(line: str) -> tuple[str, Any] | None:
    """Evento de una línea del runner: ("progress", (p, msg)), ("result", dict) o ("error", str)."""
    line = line.rstrip("\r\n")
    if line.startswith("MN-PROGRESS "):
        rest = line[len("MN-PROGRESS ") :]
        value, _, msg = rest.partition(" ")
        try:
            return "progress", (float(value), msg)
        except ValueError:
            return None
    if line.startswith("MN-RESULT "):
        return "result", json.loads(line[len("MN-RESULT ") :])
    if line.startswith("MN-ERROR "):
        return "error", json.loads(line[len("MN-ERROR ") :])
    return None


def kill_tree(proc: subprocess.Popen) -> None:
    """Mata el proceso y sus hijos: en Windows el python.exe de un venv es un
    lanzador que arranca el intérprete real como proceso hijo."""
    if proc.poll() is not None:
        return
    if os.name == "nt":
        subprocess.run(
            ["taskkill", "/F", "/T", "/PID", str(proc.pid)],
            capture_output=True,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
    else:
        proc.kill()


def run_runner(params: dict[str, Any], job: Job, python: Path, source: Path) -> dict[str, Any]:
    """Lanza el runner y sigue su salida; lo mata si se cancela el trabajo."""
    with tempfile.TemporaryDirectory(prefix="mn_acestep_", ignore_cleanup_errors=True) as tmp:
        pfile = Path(tmp) / "params.json"
        pfile.write_text(json.dumps(params, ensure_ascii=False), encoding="utf-8")
        env = {**os.environ, "PYTHONPATH": str(source), "PYTHONIOENCODING": "utf-8", "PYTHONUTF8": "1"}
        proc = subprocess.Popen(
            [str(python), "-u", str(RUNNER), str(pfile)],
            cwd=tmp,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            env=env,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        lines: queue.Queue[str | None] = queue.Queue()

        def reader() -> None:
            assert proc.stdout is not None
            for line in proc.stdout:
                lines.put(line)
            lines.put(None)

        threading.Thread(target=reader, daemon=True).start()
        result = None
        error = None
        tail: list[str] = []
        try:
            while True:
                try:
                    job.check()
                except Exception:
                    kill_tree(proc)
                    raise
                try:
                    line = lines.get(timeout=0.5)
                except queue.Empty:
                    continue
                if line is None:
                    break
                ev = parse_line(line)
                if ev is None:
                    tail = (tail + [line.rstrip()])[-15:]
                elif ev[0] == "progress":
                    job.report(ev[1][0], ev[1][1])
                elif ev[0] == "result":
                    result = ev[1]
                else:
                    error = ev[1]
        finally:
            kill_tree(proc)
            proc.wait()
        if result is None:
            detail = error or "\n".join(tail[-8:]) or f"código {proc.returncode}"
            raise RuntimeError(f"ACE-Step falló: {detail}")
        return result


@register
class InstallAceStepTask(Task):
    id = "install_acestep"
    name = "Entorno de ACE-Step"
    description = "Instala el entorno de Python aparte que necesita ACE-Step (unos 3–4 GB)."

    def available(self) -> tuple[bool, str]:
        return True, ""

    def run(self, params: dict[str, Any], job: Job) -> dict[str, Any]:
        return acestep_env.install(job)


@register
class GenerateStemTask(Task):
    id = "generate_stem"
    name = "Generar acompañamiento"
    description = "Genera una pista nueva (batería, bajo, guitarra…) que encaja con la canción (ACE-Step 1.5)."
    model_task = "generation"

    def available(self) -> tuple[bool, str]:
        if not acestep_env.is_ready():
            return False, "falta instalar el entorno de ACE-Step"
        return True, ""

    def run(self, params: dict[str, Any], job: Job) -> dict[str, Any]:
        if not acestep_env.is_ready():
            raise RuntimeError("instala primero el entorno de ACE-Step")
        if MODEL_ID not in self.models.manifests or not self.models.is_installed(MODEL_ID):
            raise RuntimeError("descarga el modelo ACE-Step 1.5 base en «Modelos»")
        track = params.get("track")
        if track not in TRACKS:
            raise ValueError(f"pista desconocida: {track}")
        device = params.get("device") or "auto"
        if device == "auto":
            device = best_device()
        started = time.monotonic()
        job.report(0.0, "preparando ACE-Step", force=True)
        runner_params = {
            "checkpoints": str(self.models.model_dir(MODEL_ID)),
            "src_audio": str(params["input"]),
            "output": str(params["output"]),
            "track": track,
            "caption": str(params.get("caption") or "")[:512],
            "bpm": int(round(params["bpm"])) if params.get("bpm") else None,
            "keyscale": params.get("keyscale") or "",
            "timesignature": str(params.get("timesignature") or ""),
            "steps": int(params.get("steps") or DEFAULT_STEPS),
            "guidance": float(params.get("guidance") or 7.0),
            "seed": int(params.get("seed", -1)),
            "device": device,
            # En GPU de menos de 20 GB ACE-Step recomienda descargar a CPU.
            "offload": bool(params.get("offload", True)),
        }
        result = run_runner(runner_params, job, acestep_env.python_exe(), acestep_env.source_dir())
        job.report(1.0, "listo", force=True)
        return {**result, "track": track, "total_seconds": round(time.monotonic() - started, 1)}
