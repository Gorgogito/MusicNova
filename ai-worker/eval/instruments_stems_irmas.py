import os, pickle, sys, collections
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DATA  # noqa: E402
import numpy as np
import musicnova_ai.tasks.transcription as T
ROOT = os.path.join(DATA, "irmas")
arch = sys.argv[1] if len(sys.argv) > 1 else "cnn14"
data = pickle.load(open(os.path.join(ROOT, f"stems_{arch}.pkl"), "rb"))
MAP = {"cel": {"strings"}, "cla": {"woodwind"}, "flu": {"woodwind"}, "gac": {"acoustic_guitar", "guitar"},
       "gel": {"electric_guitar", "guitar"}, "org": {"keyboard"}, "pia": {"piano"}, "sax": {"saxophone"},
       "tru": {"brass"}, "vio": {"violin", "strings"}, "voi": {"vocals"}}
JUDGED = set().union(*MAP.values())
names = T._labels(); idx = {n: i for i, n in enumerate(names)}
STEM_OF = {"vocals": "vocals", "speech": "vocals", "drums": "drums", "percussion": "drums", "bass": "bass"}
def combine(mix, stems, how):
    out = stems["other"].copy()
    for g, _, classes in T.INSTRUMENTS:
        st = STEM_OF.get(g, "other")
        for c in classes:
            if c in idx:
                out[:, idx[c]] = stems[st][:, idx[c]]
    return np.maximum(out, mix) if how == "max" else out
def run(how):
    hit = collections.Counter(); tot = collections.Counter(); rep = fp = 0
    for txt, labels, dur, mix, stems in data:
        p = mix if how == "mezcla" else combine(mix, stems, how)
        found = {i["id"] for i in T.summarize_instruments(p, names, dur)}
        for l in labels: tot[l] += 1; hit[l] += bool(found & MAP[l])
        ok = set().union(*(MAP[l] for l in labels))
        for g in found & JUDGED: rep += 1; fp += g not in ok
    r = sum(hit.values()) / sum(tot.values()); pr = 1 - fp / max(1, rep)
    print(f"{arch} {how:7s} umbral {T.ACTIVE_THRESHOLD}: recall {100*r:5.1f}%  precisión {100*pr:5.1f}%  F1 {200*r*pr/(r+pr):5.1f}%   " + " ".join(f"{l} {100*hit[l]/tot[l]:.0f}" for l in sorted(tot)))
for th in [0.2, 0.15, 0.1]:
    T.ACTIVE_THRESHOLD = th
    for how in ["mezcla", "stems", "max"]:
        run(how)
