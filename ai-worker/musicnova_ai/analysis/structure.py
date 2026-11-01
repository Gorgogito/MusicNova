"""Secciones de la canción.

Matriz de autosimilitud sobre rasgos sincronizados con los pulsos (croma + MFCC
con incrustación temporal), curva de novedad de Foote con núcleo de tablero de
ajedrez, picos alineados con los tiempos fuertes y agrupación jerárquica de las
secciones para etiquetar las que se repiten (A, B, C...).
"""

from __future__ import annotations

import string

import numpy as np

from . import HOP, SR


def _beat_sync(feat: np.ndarray, frames: np.ndarray) -> np.ndarray:
    cols = [feat[:, a : max(a + 1, b)].mean(axis=1) for a, b in zip(frames[:-1], frames[1:])]
    return np.stack(cols, axis=1)


def _checkerboard(half: int) -> np.ndarray:
    g = np.outer(*(2 * [np.exp(-0.5 * (np.linspace(-2, 2, 2 * half)) ** 2)]))
    sign = np.sign(np.arange(-half, half) + 0.5)
    return g * np.outer(sign, sign)


def detect_sections(
    y: np.ndarray, treble: np.ndarray, beats: np.ndarray, downbeats: np.ndarray, beats_per_bar: int, duration: float
) -> list[dict]:
    import librosa
    from scipy.cluster.hierarchy import fcluster, linkage
    from scipy.signal import find_peaks

    if len(beats) < 16 or duration < 30:
        return [{"start": 0.0, "end": round(duration, 3), "label": "A"}]

    mfcc = librosa.feature.mfcc(y=y, sr=SR, hop_length=HOP, n_mfcc=14)[1:]
    n = min(mfcc.shape[1], treble.shape[1])
    frames = np.unique(np.clip(librosa.time_to_frames(np.concatenate([[0.0], beats, [duration]]), sr=SR, hop_length=HOP), 0, n))
    chroma = treble[:, :n] / (treble[:, :n].max(axis=0, keepdims=True) + 1e-9)
    feats = []
    for f in (chroma, mfcc[:, :n]):
        s = _beat_sync(f, frames)
        s = (s - s.mean(axis=1, keepdims=True)) / (s.std(axis=1, keepdims=True) + 1e-9)
        feats.append(s)
    x = np.vstack(feats)
    x = librosa.feature.stack_memory(x, n_steps=4, mode="edge")
    x /= np.linalg.norm(x, axis=0, keepdims=True) + 1e-9
    sim = x.T @ x

    bpb = max(2, beats_per_bar)
    half = 4 * bpb  # 4 compases a cada lado
    kernel = _checkerboard(half)
    padded = np.pad(sim, half, mode="edge")
    m = sim.shape[0]
    novelty = np.array([(padded[i : i + 2 * half, i : i + 2 * half] * kernel).sum() for i in range(m)])
    novelty = np.maximum(novelty, 0)
    novelty /= novelty.max() + 1e-9

    beat_times = frames[:-1] * HOP / SR
    peaks, props = find_peaks(novelty, distance=4 * bpb, height=max(0.15, float(novelty.mean())))
    # Como mucho un límite cada ~20 s (una canción pop típica tiene 8–12 secciones).
    max_bounds = int(np.clip(duration / 20.0, 2, 24))
    strongest = np.argsort(props["peak_heights"])[::-1][:max_bounds]
    bounds = sorted(float(beat_times[peaks[i]]) for i in strongest)
    # Alinea cada límite con el tiempo fuerte más cercano.
    if len(downbeats):
        bounds = [float(downbeats[np.abs(downbeats - b).argmin()]) for b in bounds]
    edges = [0.0] + [b for b in sorted(set(bounds)) if 4.0 < b < duration - 4.0] + [duration]

    # Fusiona secciones de menos de 2 compases.
    bar_len = float(np.median(np.diff(beats))) * bpb
    merged = [edges[0]]
    for e in edges[1:]:
        if e - merged[-1] < 2 * bar_len and e != duration:
            continue
        merged.append(e)
    if len(merged) > 2 and merged[-1] - merged[-2] < 2 * bar_len:
        merged.pop(-2)
    edges = merged

    # Etiquetas: agrupa secciones con rasgos medios parecidos.
    seg_feats = []
    for a, b in zip(edges[:-1], edges[1:]):
        idx = np.where((beat_times >= a) & (beat_times < b))[0]
        seg_feats.append(x[:, idx].mean(axis=1) if len(idx) else np.zeros(x.shape[0]))
    seg_feats = np.array(seg_feats)
    n_seg = len(seg_feats)
    if n_seg > 2:
        # Unas dos apariciones por etiqueta de media, con un máximo de 8 etiquetas.
        k = int(np.clip(round(n_seg / 2), 2, 8))
        clusters = fcluster(linkage(seg_feats, method="average", metric="cosine"), t=k, criterion="maxclust")
    else:
        clusters = np.arange(1, n_seg + 1)
    names: dict[int, str] = {}
    sections: list[dict] = []
    for (a, b), c in zip(zip(edges[:-1], edges[1:]), clusters):
        if c not in names:
            names[c] = string.ascii_uppercase[len(names) % 26]
        if sections and sections[-1]["label"] == names[c]:
            sections[-1]["end"] = round(b, 3)  # secciones contiguas iguales: una sola
        else:
            sections.append({"start": round(a, 3), "end": round(b, 3), "label": names[c]})
    # Las etiquetas se renumeran por orden de aparición tras la fusión.
    order: dict[str, str] = {}
    for s in sections:
        s["label"] = order.setdefault(s["label"], string.ascii_uppercase[len(order) % 26])
    return sections
