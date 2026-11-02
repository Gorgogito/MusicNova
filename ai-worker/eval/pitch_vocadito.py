"""Precisión del tono vocal con Vocadito (40 voces reales, f0 anotado).

Métricas (como mir_eval.melody): RPA = % de fotogramas con voz de referencia
cuya f0 estimada está a menos de 50 cents; error mediano en cents; recall de voz
y falsa alarma de voz."""
import glob
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DATA  # noqa: E402
import numpy as np

from musicnova_ai.tasks.transcription import PITCH_HOP, PITCH_SR, _load_mono, track_pitch

ROOT = os.path.join(DATA, "vocadito", "x")
configs = [(e, m) for e, m in [("pyin", None), ("crepe", "tiny"), ("crepe", "full")]]
if len(sys.argv) > 1:
    configs = [c for c in configs if c[0] + (c[1] or "") in sys.argv[1:]]

voicings = [0.3, 0.4, 0.5, 0.6, 0.7]
for engine, model in configs:
    rows = []
    t0 = time.time()
    total = 0.0
    for wav in sorted(glob.glob(os.path.join(ROOT, "Audio", "*.wav"))):
        name = os.path.basename(wav)[:-4]
        ref = np.loadtxt(os.path.join(ROOT, "Annotations", "F0", name + "_f0.csv"), delimiter=",")
        y = _load_mono(wav, PITCH_SR)
        total += len(y) / PITCH_SR
        if engine == "crepe":
            f0, conf = track_pitch(y, engine="crepe", model=model, device="cuda", voicing=0.0)
        else:
            f0, conf = track_pitch(y, engine="pyin")
        t = np.arange(len(f0)) * PITCH_HOP / PITCH_SR
        rf = np.interp(t, ref[:, 0], ref[:, 1])
        # Fotogramas de referencia: sin voz si cualquiera de los vecinos interpolados es 0.
        rv = np.interp(t, ref[:, 0], (ref[:, 1] > 0).astype(float)) > 0.999
        rows.append((rf, rv, f0, conf))
    secs = time.time() - t0
    for v in voicings if engine == "crepe" else [None]:
        hit = n_ref = recall = fa = n_unv = 0
        errs = []
        for rf, rv, f0, conf in rows:
            ev = (f0 > 0) & (conf >= v) if v is not None else f0 > 0
            n_ref += rv.sum()
            both = rv & (f0 > 0)
            c = 1200 * np.abs(np.log2(f0[both] / rf[both]))
            hit += (c < 50).sum()
            errs.append(c)
            recall += (rv & ev).sum()
            fa += (~rv & ev).sum()
            n_unv += (~rv).sum()
        errs = np.concatenate(errs)
        label = engine + ("-" + model if model else "") + (f" voz>={v}" if v is not None else "")
        print(
            f"{label:22s} RPA {100*hit/n_ref:5.1f}%  mediana {np.median(errs):5.1f} c  p90 {np.percentile(errs,90):6.1f} c  "
            f"recall voz {100*recall/n_ref:5.1f}%  falsa alarma {100*fa/n_unv:5.1f}%  ({secs:.1f} s para {total:.0f} s de audio)"
        )
