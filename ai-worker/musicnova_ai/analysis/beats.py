"""Pulsos y tiempos fuertes.

- ``beat_this``: red Beat This! (CPJKU, MIT) con el postproceso «minimal» del
  proyecto original (máximos locales de ±60 ms con probabilidad > 0,5).
- ``librosa``: seguimiento clásico por programación dinámica, usado cuando no
  hay modelo instalado; los tiempos fuertes se estiman por acentos graves.
"""

from __future__ import annotations

import inspect
from pathlib import Path
from typing import Callable

import numpy as np

from . import HOP, SR

BT_SR = 22_050
BT_FPS = 50
BT_CHUNK = 1500
BT_BORDER = 6

Progress = Callable[[float, str], None]


# ---------------------------------------------------------------- Beat This!


def load_beat_this(path: Path):
    import torch

    from ..vendor.beat_this import replace_state_dict_key
    from ..vendor.beat_this.beat_tracker import BeatThis

    ckpt = torch.load(path, map_location="cpu", weights_only=True)
    params = set(inspect.signature(BeatThis).parameters)
    hparams = {k: v for k, v in ckpt["hyper_parameters"].items() if k in params}
    model = BeatThis(**hparams)
    model.load_state_dict(replace_state_dict_key(ckpt["state_dict"], "model.", ""))
    return model.eval()


def _log_mel(signal: np.ndarray, device: str):
    import torch
    import torchaudio

    mel = torchaudio.transforms.MelSpectrogram(
        sample_rate=BT_SR,
        n_fft=1024,
        hop_length=441,
        f_min=30,
        f_max=11000,
        n_mels=128,
        mel_scale="slaney",
        normalized="frame_length",
        power=1,
    ).to(device)
    x = torch.tensor(signal, dtype=torch.float32, device=device)
    return torch.log1p(1000 * mel(x).T)


def _predict_frames(model, spect, device: str, check: Callable[[], None], progress: Progress):
    """Inferencia por bloques de 30 s con bordes descartados (como `split_predict_aggregate`)."""
    import torch
    import torch.nn.functional as F

    n = spect.shape[0]
    starts = np.arange(-BT_BORDER, n - BT_BORDER, BT_CHUNK - 2 * BT_BORDER)
    if n > BT_CHUNK - 2 * BT_BORDER:
        starts[-1] = n - (BT_CHUNK - BT_BORDER)
    beat = torch.full((n,), -1000.0, device=device)
    down = torch.full((n,), -1000.0, device=device)
    use_amp = device.startswith("cuda")
    # "keep_first": se procesa al revés para que los bloques anteriores prevalezcan.
    order = list(reversed(range(len(starts))))
    for k, i in enumerate(order):
        check()
        start = int(starts[i])
        chunk = spect[max(start, 0) : min(start + BT_CHUNK, n)]
        left = max(0, -start)
        right = max(0, min(BT_BORDER, start + BT_CHUNK - n))
        if left or right:
            chunk = F.pad(chunk, (0, 0, left, right))
        with torch.inference_mode(), torch.autocast(device_type="cuda", dtype=torch.float16, enabled=use_amp):
            pred = model(chunk[None])
        b = pred["beat"][0].float()[BT_BORDER:-BT_BORDER]
        d = pred["downbeat"][0].float()[BT_BORDER:-BT_BORDER]
        lo = start + BT_BORDER
        hi = min(n, lo + len(b))
        beat[lo:hi] = b[: hi - lo]
        down[lo:hi] = d[: hi - lo]
        progress((k + 1) / len(order), "detectando pulsos")
    return beat.cpu(), down.cpu()


def _dedup(frames: np.ndarray) -> np.ndarray:
    """Une picos adyacentes (≤ 1 frame) en su media."""
    out: list[float] = []
    count = 0
    for f in frames.astype(float):
        if out and f - out[-1] <= 1 and count:
            count += 1
            out[-1] += (f - out[-1]) / count
        else:
            out.append(f)
            count = 1
    return np.array(out)


def _peaks(logits) -> np.ndarray:
    import torch.nn.functional as F

    x = logits[None, None]
    peaks = (logits == F.max_pool1d(x, 7, 1, 3)[0, 0]) & (logits > 0)
    return _dedup(peaks.nonzero()[:, 0].numpy()) / BT_FPS


def beats_beat_this(y: np.ndarray, model_path: Path, device: str, check, progress: Progress):
    import soxr

    model = load_beat_this(model_path).to(device)
    signal = y if SR == BT_SR else soxr.resample(y, SR, BT_SR)
    spect = _log_mel(signal, device)
    beat_logits, down_logits = _predict_frames(model, spect, device, check, progress)
    model.to("cpu")
    beats = _peaks(beat_logits)
    downs = _peaks(down_logits)
    if len(beats):
        downs = np.unique(beats[np.abs(beats[None, :] - downs[:, None]).argmin(axis=1)]) if len(downs) else downs
    return beats, downs


# ---------------------------------------------------------------- librosa


def beats_librosa(y: np.ndarray) -> np.ndarray:
    import librosa

    onset = librosa.onset.onset_strength(y=y, sr=SR, hop_length=HOP, aggregate=np.median)
    _, frames = librosa.beat.beat_track(onset_envelope=onset, sr=SR, hop_length=HOP, trim=False)
    return librosa.frames_to_time(frames, sr=SR, hop_length=HOP)


def estimate_downbeats(y: np.ndarray, beats: np.ndarray) -> tuple[np.ndarray, int]:
    """Elige compás (3 o 4) y fase según la energía grave y los ataques en cada pulso."""
    import librosa

    if len(beats) < 8:
        return beats[:1], 4
    spec = np.abs(librosa.stft(y, n_fft=2048, hop_length=HOP))
    freqs = librosa.fft_frequencies(sr=SR, n_fft=2048)
    low = spec[freqs < 200].sum(axis=0)
    onset = librosa.onset.onset_strength(S=librosa.amplitude_to_db(spec), sr=SR)
    frames = np.clip(librosa.time_to_frames(beats, sr=SR, hop_length=HOP), 0, len(low) - 1)
    accent = low[frames] / (low[frames].mean() + 1e-9) + onset[frames] / (onset[frames].mean() + 1e-9)
    best = (-np.inf, 4, 0)
    for bpb in (4, 3):
        for phase in range(bpb):
            on = accent[phase::bpb].mean()
            off = np.delete(accent, np.arange(phase, len(accent), bpb)).mean()
            # Ligera preferencia por 4/4, el compás más frecuente.
            score = on - off + (0.05 if bpb == 4 else 0.0)
            if score > best[0]:
                best = (score, bpb, phase)
    _, bpb, phase = best
    return beats[phase::bpb], bpb


# ---------------------------------------------------------------- común


def beats_per_bar(beats: np.ndarray, downbeats: np.ndarray) -> int:
    """Moda de pulsos entre tiempos fuertes consecutivos."""
    if len(downbeats) < 2:
        return 4
    idx = np.searchsorted(beats, downbeats - 1e-6)
    counts = np.diff(idx)
    counts = counts[(counts >= 2) & (counts <= 12)]
    if len(counts) == 0:
        return 4
    values, freq = np.unique(counts, return_counts=True)
    return int(values[freq.argmax()])


def tempo_summary(beats: np.ndarray) -> tuple[float, list[list[float]], bool]:
    """BPM global (mediana), curva de tempo suavizada y si el tempo es estable."""
    if len(beats) < 3:
        return 0.0, [], True
    ibi = np.diff(beats)
    # Media de los intervalos válidos: la mediana sola queda cuantizada a la
    # resolución de frames del detector (p. ej. 117,5 en vez de 120 BPM).
    med = np.median(ibi)
    inliers = ibi[np.abs(ibi - med) < 0.1 * med]
    bpm = float(60.0 / (inliers.mean() if len(inliers) else med))
    local = 60.0 / ibi
    k = min(8, len(local))
    smooth = np.convolve(local, np.ones(k) / k, mode="same")
    step = max(1, len(smooth) // 400)
    curve = [[round(float(beats[i]), 3), round(float(smooth[i]), 2)] for i in range(0, len(smooth), step)]
    inner = smooth[k // 2 : len(smooth) - k // 2] if len(smooth) > k else smooth
    stable = bool(np.percentile(inner, 95) - np.percentile(inner, 5) < 0.06 * bpm)
    return round(bpm, 2), curve, stable
