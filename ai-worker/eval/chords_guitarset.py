"""Acordes con GuitarSet (guitarra real; CC BY 4.0): acompañamientos con la
progresión de la partitura anotada (mayor, menor, 7 y m7♭5).

Por tramas de 10 ms: raíz, mayor/menor (las cuatriadas cuentan por su tríada) y
tipo exacto (mayor, menor, 7, m7♭5). Compara BTC con las plantillas + HMM.
uso: chords_guitarset.py [n_archivos]"""
import glob
import json
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DATA  # noqa: E402

import numpy as np  # noqa: E402

from musicnova_ai.jobs import Job  # noqa: E402
from musicnova_ai.models import ModelManager  # noqa: E402
from musicnova_ai.tasks.analysis import AnalyzeTask  # noqa: E402

ROOT = os.path.join(DATA, "guitarset")
NOTES = {"C": 0, "C#": 1, "Db": 1, "D": 2, "D#": 3, "Eb": 3, "E": 4, "F": 5, "F#": 6, "Gb": 6, "G": 7, "G#": 8, "Ab": 8, "A": 9, "A#": 10, "Bb": 10, "B": 11}
HARTE = {"maj": "", "min": "m", "7": "7", "hdim7": "m7b5", "maj7": "maj7", "min7": "m7", "dim": "dim", "sus4": "sus4"}
TRIAD = {"": "maj", "7": "maj", "maj7": "maj", "6": "maj", "m": "min", "m7": "min", "m6": "min", "mMaj7": "min"}


def parse(label):
    if label in ("N", "X"):
        return None, None
    root, _, q = label.partition(":")
    q = q.split("/")[0].split("(")[0] or "maj"
    return NOTES[root], HARTE.get(q, q)


def reference(jams):
    j = json.load(open(jams, encoding="utf-8"))
    ann = [a for a in j["annotations"] if a["namespace"] == "chord"][0]  # la de partitura
    return [(d["time"], d["time"] + d["duration"], *parse(d["value"])) for d in ann["data"]]


def at(segs, t, key_start="start"):
    for s in segs:
        if s[0] <= t < s[1]:
            return s
    return None


n_files = int(sys.argv[1]) if len(sys.argv) > 1 else 999
wavs = sorted(glob.glob(os.path.join(ROOT, "audio", "*_comp_mic.wav")))[:n_files]
task = AnalyzeTask(ModelManager())
tmp = tempfile.mkdtemp()
totals = {}
for engine in ("hmm", "btc"):
    hit = {"raíz": 0, "mayor/menor": 0, "tipo": 0}
    n = 0
    for w in wavs:
        name = os.path.basename(w).replace("_mic.wav", "")
        ref = reference(os.path.join(ROOT, "ann", name + ".jams"))
        r = task.run({"input": w, "chord_engine": engine}, Job("a", "a", lambda *a: None))
        est = [(c["start"], c["end"], c["root"], c["quality"]) for c in r["chords"]]
        end = min(ref[-1][1], r["duration"])
        for t in np.arange(0, end, 0.01):
            rs, es = at(ref, t), at(est, t)
            if rs is None or rs[2] is None:
                continue
            n += 1
            if es is None or es[2] is None:
                continue
            same_root = es[2] == rs[2]
            hit["raíz"] += same_root
            hit["mayor/menor"] += same_root and TRIAD.get(es[3]) == TRIAD.get(rs[3])
            hit["tipo"] += same_root and es[3] == rs[3]
    totals[engine] = {k: v / n for k, v in hit.items()}
    print(f"{engine:4s} ({len(wavs)} acompañamientos): " + "  ".join(f"{k} {100 * v:.1f}%" for k, v in totals[engine].items()), flush=True)
