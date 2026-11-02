"""Acompañamientos generados con ACE-Step frente a la batería real (MDB Drums).

Para cada canción: contexto = mezcla − batería original; ACE-Step genera una
batería nueva («lego»). Se mide:
- encaje rítmico: F de los golpes generados (detectados con la transcripción de
  batería de MusicNova) frente a los golpes reales anotados, a ±50 ms y a ±100 ms;
- que sea batería: lo que reconoce PANNs en la pista generada;
- que sea solo la pista: correlación de la salida con el contexto.

uso: generation_mdb.py [n_canciones] [pasos]"""
import glob
import json
import os
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DATA  # noqa: E402

import numpy as np  # noqa: E402
import soundfile as sf  # noqa: E402

import musicnova_ai.tasks.drums as D  # noqa: E402
from musicnova_ai.jobs import Job  # noqa: E402
from musicnova_ai.models import ModelManager  # noqa: E402
from musicnova_ai.tasks.generation import GenerateStemTask  # noqa: E402
from musicnova_ai.tasks.transcription import InstrumentsTask  # noqa: E402

ROOT = os.path.join(DATA, "mdb")
n_songs = int(sys.argv[1]) if len(sys.argv) > 1 else 4
steps = int(sys.argv[2]) if len(sys.argv) > 2 else 32
names = sorted(os.path.basename(f)[len("annotations__class__") : -len("_class.txt")] for f in glob.glob(os.path.join(ROOT, "annotations__class__*_class.txt")))[1::2][:n_songs]
models = ModelManager()
gen = GenerateStemTask(models)
instr = InstrumentsTask(models)
tmp = tempfile.mkdtemp()


def fmeasure(est, ref, tol):
    ref = np.sort(ref)
    used = np.zeros(len(ref), bool)
    tp = 0
    for t in np.sort(est):
        if len(ref) == 0:
            break
        d = np.abs(ref - t)
        d[used] = 9
        j = int(np.argmin(d))
        if d[j] <= tol:
            used[j] = True
            tp += 1
    p, r = tp / max(1, len(est)), tp / max(1, len(ref))
    return 2 * p * r / max(1e-9, p + r)


rows = []
for name in names:
    mix, sr = sf.read(os.path.join(ROOT, f"audio__full_mix__{name}_MIX.wav"), dtype="float32", always_2d=True)
    drums, _ = sf.read(os.path.join(ROOT, f"audio__drum_only__{name}_Drum.wav"), dtype="float32", always_2d=True)
    n = min(len(mix), len(drums))
    # Las mezclas de MedleyDB son la suma de los stems con ganancia: se ajusta por mínimos cuadrados.
    g = float(np.sum(mix[:n] * drums[:n]) / np.sum(drums[:n] ** 2))
    context = mix[:n] - g * drums[:n]
    ctx_path = os.path.join(tmp, f"{name}_ctx.wav")
    sf.write(ctx_path, context, sr, subtype="FLOAT")
    out = os.path.join(tmp, f"{name}_gen.wav")
    started = time.monotonic()
    gen.run({"input": ctx_path, "output": out, "track": "drums", "caption": "drum kit", "steps": steps, "seed": 1}, Job("g", "g", lambda *a: None))
    secs = time.monotonic() - started
    y, gsr = sf.read(out, dtype="float32", always_2d=True)
    hits = D.transcribe_drums(D._load_mono(out, D.DRUMS_SR))
    est = np.array([h["start"] for h in hits])
    ref = np.array([float(l.split()[0]) for l in open(os.path.join(ROOT, f"annotations__class__{name}_class.txt"))])
    found = instr.run({"input": out}, Job("i", "i", lambda *a: None))["instruments"]
    import librosa

    ctx_mono = librosa.resample(context.mean(1), orig_sr=sr, target_sr=gsr)[: len(y)]
    gen_mono = y.mean(1)[: len(ctx_mono)]
    corr = float(np.corrcoef(ctx_mono, gen_mono)[0, 1])
    rows.append(
        {
            "canción": name.replace("MusicDelta_", ""),
            "duración": round(n / sr, 1),
            "tiempo": round(secs, 1),
            "F50": round(fmeasure(est, ref, 0.05), 3),
            "F100": round(fmeasure(est, ref, 0.10), 3),
            # Azar con la misma densidad: los golpes generados desplazados 0,1–0,4 s.
            "F50_azar": round(float(np.mean([fmeasure(est + d, ref, 0.05) for d in np.linspace(0.1, 0.4, 7)])), 3),
            "golpes": f"{len(est)}/{len(ref)}",
            "PANNs": [i["label"] for i in found[:3]],
            "corr_contexto": round(corr, 3),
        }
    )
    print(json.dumps(rows[-1], ensure_ascii=False), flush=True)

print("medias: F50", round(np.mean([r["F50"] for r in rows]), 3), "F50 azar", round(np.mean([r["F50_azar"] for r in rows]), 3), "F100", round(np.mean([r["F100"] for r in rows]), 3), "tiempo/duración", round(np.mean([r["tiempo"] / r["duración"] for r in rows]), 2))
