"""Audio a MIDI con voz real (Vocadito): notas frente al anotador 1 y precisión
de la curva nota+bend frente a la f0 anotada, según el origen de las curvas."""
import glob
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DATA  # noqa: E402
import json

import numpy as np

from musicnova_ai.jobs import Job
from musicnova_ai.models import ModelManager
from musicnova_ai.tasks.transcription import NotesTask

ROOT = os.path.join(DATA, "vocadito", "x")
job = Job("e", "e", lambda *a, **k: None)
from pathlib import Path
task = NotesTask(ModelManager(Path(tempfile.mkdtemp())))
sources = sys.argv[1:] or ["none", "contour", "crepe"]


def bend_at(n, t):
    b = n.get("bends")
    if not b:
        return 0.0
    dt = t - n["start"]
    xs = [p[0] for p in b]
    ys = [p[1] for p in b]
    return float(np.interp(dt, xs, ys))


for src in sources:
    tp = n_est = n_ref = 0
    errs = []
    covered = 0
    total_ref = 0
    for wav in sorted(glob.glob(os.path.join(ROOT, "Audio", "*.wav"))):
        name = os.path.basename(wav)[:-4]
        out = os.path.join(tempfile.gettempdir(), f"mn_eval_{name}.json")
        task.run({"input": wav, "output": out, "mode": "mono", "bends": src, "device": "cuda", "merge_gap": float(os.environ.get("GAP", "0.03")), "onset_threshold": float(os.environ.get("ON", "0.8")), "frame_threshold": float(os.environ.get("FR", "0.25"))}, job)
        notes = json.load(open(out, encoding="utf-8"))["notes"]
        ref = np.loadtxt(os.path.join(ROOT, "Annotations", "Notes", name + "_notesA1.csv"), delimiter=",", ndmin=2)
        ref_notes = [(s, 69 + 12 * np.log2(hz / 440)) for s, hz, d in ref]
        used = set()
        for n in notes:
            for k, (s, p) in enumerate(ref_notes):
                if k not in used and abs(n["start"] - s) <= 0.05 and abs(n["pitch"] - p) <= 0.5:
                    used.add(k)
                    tp += 1
                    break
        n_est += len(notes)
        n_ref += len(ref_notes)
        f0 = np.loadtxt(os.path.join(ROOT, "Annotations", "F0", name + "_f0.csv"), delimiter=",")
        for t, hz in f0:
            if hz <= 0:
                continue
            total_ref += 1
            active = [n for n in notes if n["start"] <= t < n["end"]]
            if not active:
                continue
            covered += 1
            n = active[0]
            est = n["pitch"] + bend_at(n, t) / 100
            errs.append(abs(100 * (est - (69 + 12 * np.log2(hz / 440)))))
    p, r = tp / max(1, n_est), tp / max(1, n_ref)
    errs = np.array(errs)
    with_bends = sum(1 for _ in [0])
    print(
        f"curvas={src:8s} notas: precisión {100*p:5.1f}%  recall {100*r:5.1f}%  F1 {200*p*r/max(1e-9,p+r):5.1f}%   "
        f"tono nota+curva frente a f0 real: mediana {np.median(errs):5.1f} c  p90 {np.percentile(errs,90):5.1f} c  "
        f"(< 50 c: {100*(errs<50).mean():5.1f}%, voz cubierta por notas {100*covered/total_ref:5.1f}%)"
    )
