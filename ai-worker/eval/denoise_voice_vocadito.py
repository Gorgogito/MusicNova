"""Limpieza de voz con DeepFilterNet3: voces reales de Vocadito con ruido blanco
a 5 dB de SNR. SI-SDR antes y después, y frente a dos variantes de control
(solo la máscara ERB y el filtrado profundo desalineado) que validan el port."""
import glob
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DATA  # noqa: E402

import librosa  # noqa: E402
import numpy as np  # noqa: E402
import soundfile as sf  # noqa: E402

import musicnova_ai.vendor.deepfilternet as D  # noqa: E402
from musicnova_ai.models import ModelManager  # noqa: E402


def sisdr(ref, est):
    a = np.dot(est, ref) / np.dot(ref, ref)
    return 10 * np.log10(np.sum((a * ref) ** 2) / np.sum((est - a * ref) ** 2))


model = D.DeepFilterNet3(ModelManager().model_dir("deepfilternet3"))
rng = np.random.default_rng(1)
res = {k: [] for k in ["ruidosa", "DeepFilterNet3", "solo máscara", "filtrado desalineado"]}
for w in sorted(glob.glob(os.path.join(DATA, "vocadito", "x", "Audio", "*.wav")))[::5]:
    s, sr = sf.read(w, dtype="float32", always_2d=True)
    s = librosa.resample(s[:, 0], orig_sr=sr, target_sr=D.SR)[: D.SR * 12]
    s /= np.abs(s).max() * 2
    noise = rng.standard_normal(len(s)).astype(np.float32)
    noise *= np.sqrt(np.mean(s**2) / np.mean(noise**2)) * 10 ** (-5 / 20)
    n = s + noise
    res["ruidosa"].append(sisdr(s, n))
    res["DeepFilterNet3"].append(sisdr(s, model.enhance(n)))
    sp = D.analysis(np.concatenate([n, np.zeros(D.FFT_SIZE, np.float32)]))
    fe, fs = D.features(sp, model.widths, model.alpha)
    e0, e1, e2, e3, emb, c0, _ = model.enc.run(None, {"feat_erb": D._lookahead(fe, 2), "feat_spec": D._lookahead(fs, 2)})
    (m,) = model.erb_dec.run(None, {"emb": emb, "e3": e3, "e2": e2, "e1": e1, "e0": e0})
    masked = sp * np.repeat(m[0, 0], model.widths, axis=1)
    d = D.FFT_SIZE - D.HOP
    res["solo máscara"].append(sisdr(s, D.synthesis(masked)[d : d + len(s)]))
    D.DF_LOOKAHEAD = 0
    res["filtrado desalineado"].append(sisdr(s, model.enhance(n)))
    D.DF_LOOKAHEAD = 2
for k, v in res.items():
    print(f"{k:22s} SI-SDR medio {np.mean(v):5.1f} dB ({len(v)} voces)")
