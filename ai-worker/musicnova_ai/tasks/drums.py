"""Transcripción de batería: bombo, caja y charles a notas MIDI de percusión.

Factorización no negativa semiadaptativa (Dittmar y Gärtner, «Real-time
transcription and separation of drum recordings based on NMF decomposition»,
DAFx 2014): cada instrumento parte de una plantilla espectral que se va
adaptando a la grabación, y unos componentes libres recogen lo demás (platos,
toms, restos de otros instrumentos). Los golpes son los picos de las
activaciones de cada plantilla.

Sin modelos que descargar: funciona con el stem de batería separado o con una
toma de batería grabada.
"""

from __future__ import annotations

import time
from typing import Any

from ..jobs import Job
from . import Task, register
from .transcription import NOTES_VERSION, _load_mono, _missing, _write_json

DRUMS_SR = 44_100
DRUMS_FFT = 2048
DRUMS_HOP = 256  # 5,8 ms
DRUMS_BANDS = 96
DRUMS_DEPS = ("numpy", "scipy", "soundfile", "librosa")

# Instrumento → (nota GM, nombre). El orden es el de las plantillas. Los platos
# tienen plantilla para que no se confundan con el charles, pero no se
# transcriben: su detección no llegaba al 15 % de acierto en MDB Drums.
DRUM_CLASSES = [("kick", 36, "Bombo"), ("snare", 38, "Caja"), ("hihat", 42, "Charles"), ("cymbal", 49, "Platos")]
TRANSCRIBED = ("kick", "snare", "hihat")
FREE_COMPONENTS = 1
NMF_ITERATIONS = 40
# El parecido con la plantilla inicial se va soltando con (iter/total)^ADAPT_POWER.
ADAPT_POWER = 2.0
NOTE_LENGTH = 0.1
MIN_GAP = 0.05  # dos golpes del mismo instrumento, como poco a 50 ms

# Umbral de pico por instrumento (fracción del nivel alto de su activación) y
# realce sobre la media local. Ajustados con la mitad de MDB Drums.
PEAK_THRESHOLD = {"kick": 0.25, "snare": 0.12, "hihat": 0.12}
PEAK_DELTA = 0.05


def _band_matrix(sr: int):
    import librosa

    return librosa.filters.mel(sr=sr, n_fft=DRUMS_FFT, n_mels=DRUMS_BANDS, fmin=20.0, fmax=sr / 2, htk=True, norm=None)


def band_freqs(sr: int = DRUMS_SR):
    import librosa

    return librosa.mel_frequencies(n_mels=DRUMS_BANDS, fmin=20.0, fmax=sr / 2, htk=True)


def initial_templates(sr: int = DRUMS_SR):
    """Plantillas espectrales (bandas × 4) de bombo, caja, charles y platos: formas
    típicas en escala logarítmica de frecuencia, normalizadas a suma 1."""
    import numpy as np

    f = band_freqs(sr)
    lf = np.log2(np.maximum(f, 20.0))

    def bump(center, width):
        return np.exp(-0.5 * ((lf - np.log2(center)) / width) ** 2)

    kick = bump(70, 0.6) + 0.08 * bump(3000, 1.2)
    snare = 0.7 * bump(200, 0.5) + 0.6 * bump(2500, 1.6)
    hihat = bump(9000, 0.8) * (f > 2000)
    cymbal = bump(5000, 1.1) * (f > 800)
    w = np.stack([kick, snare, hihat, cymbal], axis=1) + 1e-6
    return w / w.sum(axis=0, keepdims=True)


def spectrogram(y, sr: int = DRUMS_SR):
    """Magnitud en bandas mel (bandas × fotogramas)."""
    import librosa
    import numpy as np

    mag = np.abs(librosa.stft(y, n_fft=DRUMS_FFT, hop_length=DRUMS_HOP, center=True))
    return _band_matrix(sr) @ mag


def nmf_semi_adaptive(v, w0, free: int | None = None, iterations: int | None = None, seed: int = 0, check=None):
    """KL-NMF: V ≈ W·H. Las primeras columnas de W parten de `w0` y se mezclan
    con su actualización con un peso que crece durante las iteraciones; las
    `free` últimas son libres desde el principio."""
    import numpy as np

    free = FREE_COMPONENTS if free is None else free
    iterations = NMF_ITERATIONS if iterations is None else iterations
    rng = np.random.default_rng(seed)
    bands, frames = v.shape
    k = w0.shape[1]
    w = np.concatenate([w0, rng.uniform(0.1, 1.0, (bands, free))], axis=1)
    w /= w.sum(axis=0, keepdims=True)
    h = rng.uniform(0.1, 1.0, (k + free, frames)) * (v.mean() + 1e-9)
    eps = 1e-12
    for it in range(iterations):
        if check and it % 10 == 0:
            check()
        wh = w @ h + eps
        h *= (w.T @ (v / wh)) / (w.sum(axis=0)[:, None] + eps)
        wh = w @ h + eps
        upd = w * ((v / wh) @ h.T) / (h.sum(axis=1)[None, :] + eps)
        upd /= upd.sum(axis=0, keepdims=True) + eps
        alpha = ((it + 1) / iterations) ** ADAPT_POWER
        w[:, :k] = (1 - alpha) * w0 + alpha * upd[:, :k]
        w[:, k:] = upd[:, k:]
        w /= w.sum(axis=0, keepdims=True) + eps
    return w, h


def pick_peaks(act, fps: float, threshold: float, delta: float | None = None, min_gap: float = MIN_GAP):
    """Golpes de una activación: máximos locales (±min_gap) por encima de
    `threshold` × nivel alto y de la media de los 100 ms anteriores + `delta`."""
    import numpy as np
    from scipy.ndimage import maximum_filter1d, uniform_filter1d

    delta = PEAK_DELTA if delta is None else delta
    a = np.asarray(act, dtype=np.float64)
    if a.size == 0:
        return np.zeros(0, dtype=int), np.zeros(0)
    top = np.percentile(a, 99.5)
    if top <= 0:
        return np.zeros(0, dtype=int), np.zeros(0)
    a = a / top
    w = max(1, int(round(min_gap * fps)))
    local_max = maximum_filter1d(a, size=2 * w + 1, mode="nearest")
    n_mean = max(1, int(round(0.1 * fps)))
    mean = uniform_filter1d(a, size=n_mean, origin=(n_mean - 1) // 2, mode="nearest")
    cand = np.where((a == local_max) & (a >= threshold) & (a >= mean + delta))[0]
    peaks: list[int] = []
    for i in cand:
        if not peaks or i - peaks[-1] > w:
            peaks.append(int(i))
        elif a[i] > a[peaks[-1]]:
            peaks[-1] = int(i)
    peaks_arr = np.array(peaks, dtype=int)
    return peaks_arr, a[peaks_arr] if len(peaks_arr) else np.zeros(0)


def transcribe_drums(y, sr: int = DRUMS_SR, sensitivity: float = 0.5, check=None, progress=None) -> list[dict[str, Any]]:
    """Notas de percusión GM (inicio, fin, altura, velocidad) de una toma de batería."""
    import numpy as np

    v = spectrogram(y, sr)
    if progress:
        progress(0.2)
    w0 = initial_templates(sr)
    _, h = nmf_semi_adaptive(v, w0, check=check)
    if progress:
        progress(0.85)
    fps = sr / DRUMS_HOP
    # Sensibilidad 0–1 (0,5 = umbral ajustado): más alta, más golpes débiles.
    scale = float(np.interp(sensitivity, [0.0, 0.5, 1.0], [2.0, 1.0, 0.4]))
    notes = []
    for k, (name, pitch, _) in enumerate(DRUM_CLASSES):
        if name not in TRANSCRIBED:
            continue
        peaks, heights = pick_peaks(h[k], fps, PEAK_THRESHOLD[name] * scale)
        for i, a in zip(peaks, heights):
            t = max(0.0, (i * DRUMS_HOP) / sr)
            vel = int(np.clip(round(40 + 87 * min(1.0, float(a)) ** 0.5), 1, 127))
            notes.append({"start": round(t, 4), "end": round(t + NOTE_LENGTH, 4), "pitch": pitch, "velocity": vel})
    notes.sort(key=lambda n: (n["start"], n["pitch"]))
    return notes


@register
class DrumsTask(Task):
    id = "transcribe_drums"
    name = "Batería a MIDI"
    description = "Transcribe bombo, caja y charles de una pista de batería (NMF semiadaptativa)."

    def available(self) -> tuple[bool, str]:
        missing = _missing(DRUMS_DEPS)
        if missing:
            return False, "faltan dependencias de Python: " + ", ".join(missing)
        return True, ""

    def run(self, params: dict[str, Any], job: Job) -> dict[str, Any]:
        started = time.monotonic()
        job.report(0.0, "leyendo audio", force=True)
        y = _load_mono(params["input"], DRUMS_SR)
        if len(y) < DRUMS_SR // 2:
            raise ValueError("el audio es demasiado corto para transcribirlo")
        job.check()
        notes = transcribe_drums(
            y,
            DRUMS_SR,
            sensitivity=float(params.get("sensitivity", 0.5)),
            check=job.check,
            progress=lambda p: job.report(0.05 + 0.9 * p, "reconociendo golpes"),
        )
        result = {
            "version": NOTES_VERSION,
            "source": "nmf_drums",
            "mode": "drums",
            "notes": notes,
            "seconds": round(time.monotonic() - started, 2),
        }
        out = params.get("output")
        if out:
            _write_json(out, result)
        job.report(1.0, "listo", force=True)
        return {"output": str(out), "notes": len(notes), "seconds": result["seconds"]} if out else result
