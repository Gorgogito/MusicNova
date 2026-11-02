"""Detección de instrumentos con IRMAS (fragmentos de 5–20 s de música real con
los instrumentos predominantes anotados).

Recall: de los instrumentos anotados, cuántos aparecen en el informe.
Precisión: de los grupos que informamos y que IRMAS podría anotar (los 11
instrumentos melódicos que cubre), cuántos están anotados. IRMAS solo anota los
predominantes, así que un acompañamiento real cuenta como fallo: la precisión
es una cota inferior.

uso: instruments_irmas.py [cnn14|cnn10] [ACTIVE_THRESHOLD] [MIN_COVERAGE]"""
import collections
import os
import pickle
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DATA  # noqa: E402
import numpy as np

import musicnova_ai.tasks.transcription as T

ROOT = os.path.join(DATA, "irmas")
arch = sys.argv[1] if len(sys.argv) > 1 else "cnn14"
if len(sys.argv) > 2:
    T.ACTIVE_THRESHOLD = float(sys.argv[2])
if len(sys.argv) > 3:
    T.MIN_COVERAGE = float(sys.argv[3])

# Etiqueta de IRMAS → grupos que la cuentan como acierto.
MAP = {
    "cel": {"strings"}, "cla": {"woodwind"}, "flu": {"woodwind"}, "gac": {"acoustic_guitar", "guitar"},
    "gel": {"electric_guitar", "guitar"}, "org": {"keyboard"}, "pia": {"piano"}, "sax": {"saxophone"},
    "tru": {"brass"}, "vio": {"violin", "strings"}, "voi": {"vocals"},
}
JUDGED = set().union(*MAP.values())

cache = os.path.join(ROOT, f"probs_{arch}.pkl")
if os.path.exists(cache):
    data = pickle.load(open(cache, "rb"))
else:
    import torch

    from musicnova_ai.vendor import panns

    model_dir = os.path.join(os.environ["LOCALAPPDATA"], "MusicNova", "models", f"panns_{arch}")
    ckpt = [f for f in os.listdir(model_dir) if f.endswith(".pth")][0]
    model = panns.load("Cnn14" if arch == "cnn14" else "Cnn10", os.path.join(model_dir, ckpt), "cuda")
    data = []
    for txt in open(os.path.join(ROOT, "sample.txt"), encoding="utf-8").read().split("\n"):
        wav = os.path.join(ROOT, txt[:-4] + ".wav")
        if not os.path.exists(wav):
            continue
        labels = [l.strip() for l in open(os.path.join(ROOT, txt), encoding="utf-8", errors="replace") if l.strip()]
        y = T._load_mono(wav, T.PANNS_SR)
        win, hop = int(T.INSTRUMENT_WINDOW * T.PANNS_SR), int(T.INSTRUMENT_HOP * T.PANNS_SR)
        starts = list(range(0, max(1, len(y) - win + hop), hop))
        chunk = [np.pad(y[s : s + win], (0, max(0, win - len(y[s : s + win])))) for s in starts]
        with torch.inference_mode():
            p = model(torch.from_numpy(np.stack(chunk)).cuda()).float().cpu().numpy()
        data.append((txt, labels, len(y) / T.PANNS_SR, p))
    pickle.dump(data, open(cache, "wb"))

names = T._labels()
hit = collections.Counter()
total = collections.Counter()
fp = collections.Counter()
reported = collections.Counter()
for txt, labels, dur, p in data:
    found = {i["id"] for i in T.summarize_instruments(p, names, dur)}
    for l in labels:
        total[l] += 1
        hit[l] += bool(found & MAP[l])
    ok = set().union(*(MAP[l] for l in labels))
    for g in found & JUDGED:
        reported[g] += 1
        fp[g] += g not in ok
rec = sum(hit.values()) / sum(total.values())
prec = 1 - sum(fp.values()) / max(1, sum(reported.values()))
print(
    f"{arch} umbral {T.ACTIVE_THRESHOLD} cobertura {T.MIN_COVERAGE}: recall {100*rec:.1f}%  precisión {100*prec:.1f}%  "
    f"F1 {200*rec*prec/(rec+prec):.1f}%  ({len(data)} fragmentos)"
)
print("  recall por instrumento: " + "  ".join(f"{l} {100*hit[l]/total[l]:.0f}%" for l in sorted(total)))
print("  falsos por grupo: " + "  ".join(f"{g} {fp[g]}/{reported[g]}" for g in sorted(reported)))
