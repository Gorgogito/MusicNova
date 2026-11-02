"""Batería a MIDI con MDB Drums (23 canciones reales). F por instrumento con
tolerancia de 50 ms. Canciones pares = ajuste, impares = prueba.
uso: drums_mdb.py [drum_only|full_mix|sep] [ajuste|prueba|todo] [clave=valor ...]"""
import glob, os, sys, pickle
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DATA  # noqa: E402
import numpy as np
import musicnova_ai.tasks.drums as D

ROOT = os.path.join(DATA, "mdb")
kind = sys.argv[1] if len(sys.argv) > 1 else "drum_only"
split = sys.argv[2] if len(sys.argv) > 2 else "todo"
for kv in sys.argv[3:]:
    k, v = kv.split("=")
    if k.startswith("th_"):
        D.PEAK_THRESHOLD[k[3:]] = float(v)
    else:
        setattr(D, k, type(getattr(D, k))(float(v)))
CLS = {"KD": 36, "SD": 38, "HH": 42}
names = sorted(os.path.basename(f)[len("annotations__class__"):-len("_class.txt")] for f in glob.glob(os.path.join(ROOT, "annotations__class__*_class.txt")))
if split == "ajuste": names = names[0::2]
elif split == "prueba": names = names[1::2]
stats = {p: [0, 0, 0] for p in CLS.values()}
for name in names:
    if kind == "sep":
        wav = os.path.join(ROOT, "sep", name + "_drums.wav")
    else:
        wav = os.path.join(ROOT, f"audio__{kind}__{name}_{'Drum' if kind == 'drum_only' else 'MIX'}.wav")
    y = D._load_mono(wav, D.DRUMS_SR)
    est = D.transcribe_drums(y)
    ref = [l.split() for l in open(os.path.join(ROOT, f"annotations__class__{name}_class.txt"))]
    for c, p in CLS.items():
        r = np.array(sorted(float(t) for t, k in ref if k == c))
        e = np.array(sorted(n["start"] for n in est if n["pitch"] == p))
        used = np.zeros(len(r), bool); tp = 0
        for t in e:
            if len(r) == 0: break
            d = np.abs(r - t); d[used] = 9
            j = int(np.argmin(d))
            if d[j] <= 0.05: used[j] = True; tp += 1
        stats[p][0] += tp; stats[p][1] += len(e); stats[p][2] += len(r)
fs = []
out = []
for c, p in CLS.items():
    tp, ne, nr = stats[p]
    pr, rc = tp / max(1, ne), tp / max(1, nr)
    f = 2 * pr * rc / max(1e-9, pr + rc)
    if c != "CY": fs.append(f)
    out.append(f"{c} P {100*pr:4.0f} R {100*rc:4.0f} F {100*f:4.1f}")
print(f"{kind:9s} {split:6s} " + " | ".join(out) + f" | media F (KD/SD/HH) {100*np.mean(fs):.1f}  ({len(names)} canciones) {' '.join(sys.argv[3:])}")
