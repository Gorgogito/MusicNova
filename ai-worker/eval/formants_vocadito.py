"""¿Conserva el timbre la afinación? Voces reales de Vocadito transpuestas con
el motor de MusicNova (apply_shifts), con y sin conservar formantes.

Para cada voz se calcula la envolvente espectral media (LPC) de los fotogramas
con voz y se busca el factor α que mejor alinea la envolvente transpuesta con la
original (env_t(f) ≈ env_o(f/α)). Formantes conservados: α ≈ 1. Formantes que
se mueven con la nota: α ≈ 2^(semitonos/12)."""
import glob
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DATA  # noqa: E402
import librosa
import numpy as np
import soundfile as sf
from pathlib import Path

from musicnova_ai.jobs import Job
from musicnova_ai.models import ModelManager
from musicnova_ai.tasks.transcription import PitchTask

pitch = PitchTask(ModelManager(Path(tempfile.mkdtemp())))

ROOT = os.path.join(DATA, "vocadito", "x")
EXE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "target", "release", "examples", "shift_file.exe")
SR = 16_000
GRID = np.geomspace(250, 4500, 200)
ALPHAS = np.geomspace(0.6, 1.6, 401)


def envelope(y):
    y = librosa.effects.preemphasis(y)
    frames = librosa.util.frame(y, frame_length=512, hop_length=160).T
    rms = np.sqrt((frames**2).mean(axis=1))
    frames = frames[rms > np.percentile(rms, 60)] * np.hanning(512)
    envs = []
    for f in frames:
        a = librosa.lpc(f, order=18)
        w = np.exp(-2j * np.pi * np.outer(GRID / SR, np.arange(len(a))))
        envs.append(-20 * np.log10(np.abs(w @ a) + 1e-9))
    return np.mean(envs, axis=0)


def warp(orig, shifted):
    """α que minimiza el error entre shifted(f) y orig(f/α) (sin el nivel medio)."""
    best = (1e9, 1.0)
    for a in ALPHAS:
        ref = np.interp(np.log(GRID / a), np.log(GRID), orig, left=np.nan, right=np.nan)
        ok = np.isfinite(ref)
        if ok.sum() < 100:
            continue
        d = shifted[ok] - ref[ok]
        err = np.sqrt(np.mean((d - d.mean()) ** 2))
        best = min(best, (err, a))
    return best[1], best[0]


wavs = sorted(glob.glob(os.path.join(ROOT, "Audio", "*.wav")))[::4]  # 10 voces distintas
tmp = tempfile.mkdtemp()
rows = {}
for wav in wavs:
    y, sr = sf.read(wav, dtype="float32", always_2d=True)
    src = os.path.join(tmp, "src.wav")
    sf.write(src, np.repeat(y[:, :1], 2, axis=1), sr)
    o = envelope(librosa.resample(y[:, 0], orig_sr=sr, target_sr=SR))
    curve = os.path.join(tmp, "curve.json")
    pitch.run({"input": src, "output": curve, "engine": "crepe", "device": "cuda"}, Job("p", "p", lambda *a, **k: None))
    for semis in (-5, -3, 3, 5):
        for formants in (0, 1, 2):
            out = os.path.join(tmp, "out.wav")
            extra = [curve] if formants == 2 else []
            subprocess.run([EXE, src, out, str(semis), str(min(formants, 1))] + extra, check=True)
            z, _ = sf.read(out, dtype="float32", always_2d=True)
            s = envelope(librosa.resample(z[:, 0], orig_sr=sr, target_sr=SR))
            a, err = warp(o, s)
            d = s - o
            flat = np.sqrt(np.mean((d - d.mean()) ** 2))
            rows.setdefault((semis, formants), []).append((a, flat))
print(f"{len(wavs)} voces reales de Vocadito; α = desplazamiento de formantes medido (1 = timbre intacto)")
for (semis, formants), r in sorted(rows.items()):
    a = np.array([x[0] for x in r])
    flat = np.array([x[1] for x in r])
    print(
        f"  {semis:+d} semitonos, formantes {['libres             ', 'conservados        ', 'conservados (guía)'][formants]}: "
        f"α mediana {np.median(a):.3f} (si se movieran con la nota: {2 ** (semis / 12):.3f})  "
        f"diferencia de envolvente {np.median(flat):.1f} dB"
    )
