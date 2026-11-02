"""Reducción de ruido constante con instrumentos reales de IRMAS: siseo blanco a
10 y 20 dB de SNR con 1 s de solo ruido delante. SNR antes y después.
Necesita `cargo build --release -p mn-core --example denoise_file`.
uso: denoise_steady_irmas.py [reducción dB] [sensibilidad]"""
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DATA  # noqa: E402

import numpy as np  # noqa: E402
import soundfile as sf  # noqa: E402

R = os.path.join(DATA, "irmas")
EXE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "release", "examples", "denoise_file.exe")
extra = sys.argv[1:3]
rows = [(l, open(os.path.join(R, l), encoding="utf-8", errors="replace").read().split()) for l in open(os.path.join(R, "sample.txt"), encoding="utf-8").read().split("\n")]
picked = {}
for txt, labs in rows:
    if len(labs) == 1 and labs[0] not in picked and labs[0] != "voi" and os.path.exists(os.path.join(R, txt[:-4] + ".wav")):
        picked[labs[0]] = txt
rng = np.random.default_rng(0)
tmp = tempfile.mkdtemp()


def snr(ref, est):
    return 10 * np.log10(np.sum(ref**2) / np.sum((est - ref) ** 2))


for snr_in in (10, 20):
    res = []
    for lab, txt in sorted(picked.items()):
        y, sr = sf.read(os.path.join(R, txt[:-4] + ".wav"), dtype="float32", always_2d=True)
        clean = np.concatenate([np.zeros((sr, 2), np.float32), y])
        noise = rng.standard_normal(clean.shape).astype(np.float32)
        noise *= np.sqrt(np.mean(y**2) / np.mean(noise**2)) * 10 ** (-snr_in / 20)
        sf.write(os.path.join(tmp, "in.wav"), clean + noise, sr, subtype="FLOAT")
        subprocess.run([EXE, os.path.join(tmp, "in.wav"), os.path.join(tmp, "out.wav"), *extra], check=True)
        out, _ = sf.read(os.path.join(tmp, "out.wav"), dtype="float32", always_2d=True)
        m = slice(sr, None)
        res.append((lab, snr(clean[m], clean[m] + noise[m]), snr(clean[m], out[m])))
    print(f"SNR de entrada {snr_in} dB: " + "  ".join(f"{l} {a:.1f}→{b:.1f}" for l, a, b in res) + f"   media +{np.mean([b - a for _, a, b in res]):.1f} dB")
