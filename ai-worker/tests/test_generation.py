"""Fase 7: generación con ACE-Step (entorno aparte) y manifiestos con subcarpetas.

El protocolo con el runner se prueba con un runner falso; la generación real
necesita el entorno de ACE-Step y su modelo (se salta si no están).
"""

from __future__ import annotations

import json
import sys
import threading
import time
from pathlib import Path

import numpy as np
import pytest
import soundfile as sf

from musicnova_ai import acestep_env
from musicnova_ai.jobs import Job, JobCancelled
from musicnova_ai.models import ModelManager, ModelManifest
from musicnova_ai.tasks import generation
from musicnova_ai.tasks.generation import GenerateStemTask, parse_line, run_runner


def test_parse_runner_lines():
    assert parse_line("MN-PROGRESS 0.25 generando paso 4\n") == ("progress", (0.25, "generando paso 4"))
    assert parse_line('MN-RESULT {"output": "x.wav"}') == ("result", {"output": "x.wav"})
    assert parse_line('MN-ERROR "falta memoria"') == ("error", "falta memoria")
    assert parse_line("2026-11-02 | INFO | cargando") is None
    assert parse_line("MN-PROGRESS nada") is None


FAKE = """
import json, sys, time
p = json.load(open(sys.argv[1], encoding="utf-8"))
print("ruido de registro de ACE-Step", flush=True)
for i in range(3):
    print(f"MN-PROGRESS {i / 3:.2f} paso {i}", flush=True)
    time.sleep(p.get("sleep", 0))
if p.get("fail"):
    print('MN-ERROR "sin memoria en la GPU"', flush=True)
    sys.exit(1)
open(p["output"], "w").write("ok")
print("MN-RESULT " + json.dumps({"output": p["output"], "seconds": 0.1}), flush=True)
"""


def fake_runner(tmp_path, monkeypatch):
    script = tmp_path / "fake_runner.py"
    script.write_text(FAKE, encoding="utf-8")
    monkeypatch.setattr(generation, "RUNNER", script)


def test_runner_protocol_progress_result_and_errors(tmp_path, monkeypatch):
    fake_runner(tmp_path, monkeypatch)
    seen = []
    job = Job("j", "generate_stem", lambda ev, data: seen.append((ev, data)))
    out = tmp_path / "out.wav"
    r = run_runner({"output": str(out)}, job, Path(sys.executable), tmp_path)
    assert r["output"] == str(out) and out.read_text() == "ok"
    # Job.report limita la frecuencia de los avisos: al menos llega el primero.
    progress = [d["progress"] for e, d in seen if e == "job.progress"]
    assert progress and all(0 <= v <= 1 for v in progress)
    with pytest.raises(RuntimeError, match="sin memoria"):
        run_runner({"output": str(out), "fail": True}, Job("k", "g", lambda *a: None), Path(sys.executable), tmp_path)


def test_runner_is_killed_on_cancel(tmp_path, monkeypatch):
    # Con el python.exe del venv (un lanzador con un proceso hijo en Windows):
    # hay que matar el árbol entero, no solo el lanzador.
    fake_runner(tmp_path, monkeypatch)
    out = tmp_path / "o.wav"
    job = Job("j", "generate_stem", lambda *a: None)
    threading.Timer(0.5, job.cancel).start()
    started = time.monotonic()
    with pytest.raises(JobCancelled):
        run_runner({"output": str(out), "sleep": 3}, job, Path(sys.executable), tmp_path)
    assert time.monotonic() - started < 3
    time.sleep(4)
    assert not out.exists(), "el runner siguió vivo tras cancelar"


def test_manifest_accepts_safe_subfolders_only(tmp_path):
    def write(name):
        p = tmp_path / "m.toml"
        p.write_text(
            'id = "x"\nname = "x"\ntask = "generation"\nfamily = "x"\ndescription = ""\nlicense = "MIT"\nlicense_url = ""\n'
            f'commercial_use = true\n[[files]]\nname = "{name}"\nurl = "https://e/x"\nsha256 = "0123456789abcdef"\n',
            encoding="utf-8",
        )
        return ModelManifest.from_toml(p)

    assert write("vae/config.json").files[0].name == "vae/config.json"
    for bad in ["../evil.dll", "vae/../../x", ".hidden/x", "a//b", "C:/x"]:
        with pytest.raises(ValueError):
            write(bad)


def test_acestep_manifest_is_complete():
    m = ModelManager().get("acestep_v15_base")
    names = {f.name for f in m.files}
    for needed in ["acestep-v15-base/model.safetensors", "vae/diffusion_pytorch_model.safetensors", "Qwen3-Embedding-0.6B/model.safetensors"]:
        assert needed in names
    assert m.license == "MIT" and all(len(f.sha256) == 64 for f in m.files)


@pytest.mark.skipif(not acestep_env.is_ready(), reason="entorno de ACE-Step no instalado")
def test_generate_drums_with_acestep(tmp_path):
    models = ModelManager()
    if not models.is_installed("acestep_v15_base"):
        pytest.skip("modelo ACE-Step no instalado")
    # Contexto: 12 s de acordes de piano sintéticos a 100 BPM en Do mayor.
    sr = 48_000
    t = np.arange(sr * 12) / sr
    chords = [[60, 64, 67], [57, 60, 64], [65, 69, 72], [67, 71, 74]]
    y = np.zeros_like(t)
    for k, ch in enumerate(chords * 2):
        seg = (t >= k * 1.2) & (t < k * 1.2 + 1.2)
        for m in ch:
            y[seg] += 0.1 * np.sin(2 * np.pi * 440 * 2 ** ((m - 69) / 12) * t[seg]) * np.exp(-(t[seg] - k * 1.2) * 1.5)
    src = tmp_path / "contexto.wav"
    sf.write(src, np.stack([y, y], 1).astype(np.float32), sr)
    out = tmp_path / "bateria.wav"
    r = GenerateStemTask(models).run(
        {"input": str(src), "output": str(out), "track": "drums", "caption": "simple pop drum beat", "bpm": 100, "keyscale": "C major", "steps": 16, "seed": 7},
        Job("g", "generate_stem", lambda *a: None),
    )
    audio, out_sr = sf.read(out, always_2d=True)
    assert out_sr == 48_000 and abs(len(audio) / out_sr - 12) < 1.0, (len(audio), out_sr)
    assert np.sqrt(np.mean(audio**2)) > 1e-3, "la pista generada no debería ser silencio"
    print(json.dumps(r))
