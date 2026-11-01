"""Acordes y tonalidad a partir de cromagramas CQT.

Acordes: cromagrama sincronizado con los pulsos, plantillas de 7 tipos de acorde
por cada una de las 12 fundamentales más «sin acorde», refuerzo con el croma
grave (la nota del bajo suele ser la fundamental) y decodificación de Viterbi
para evitar cambios espurios.

Tonalidad: correlación con los perfiles de Krumhansl-Kessler, desempatada con
el tiempo que suenan los acordes de tónica de cada candidata (distingue mejor
entre relativas, p. ej. Do mayor y La menor).
"""

from __future__ import annotations

import numpy as np

from . import HOP, NOTE_NAMES, SR

# Intervalos (semitonos) y peso de cada nota de la plantilla.
QUALITIES: dict[str, dict[int, float]] = {
    "": {0: 1.0, 4: 1.0, 7: 1.0},
    "m": {0: 1.0, 3: 1.0, 7: 1.0},
    "7": {0: 1.0, 4: 1.0, 7: 0.8, 10: 0.8},
    "maj7": {0: 1.0, 4: 1.0, 7: 0.8, 11: 1.0},
    "m7": {0: 1.0, 3: 1.0, 7: 0.8, 10: 0.8},
    "dim": {0: 1.0, 3: 1.0, 6: 1.0},
    "sus4": {0: 1.0, 5: 1.0, 7: 1.0},
}
# Penalización a los tipos menos comunes para que las tríadas ganen en caso de duda.
QUALITY_PRIOR = {"": 0.0, "m": 0.0, "7": 0.0, "maj7": 0.0, "m7": 0.0, "dim": 0.06, "sus4": 0.06}

# Tríada básica de cada tipo y acordes diatónicos (intervalo desde la tónica, tríada).
TRIAD_OF = {"": "maj", "7": "maj", "maj7": "maj", "m": "min", "m7": "min", "dim": "dim", "sus4": "sus"}
DIATONIC_MAJOR = {(0, "maj"), (2, "min"), (4, "min"), (5, "maj"), (7, "maj"), (9, "min"), (11, "dim"), (0, "sus"), (7, "sus"), (2, "sus")}
# Menor natural más la dominante mayor de la armónica.
DIATONIC_MINOR = {(0, "min"), (2, "dim"), (3, "maj"), (5, "min"), (7, "min"), (7, "maj"), (8, "maj"), (10, "maj"), (11, "dim")}

KK_MAJOR = np.array([6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88])
KK_MINOR = np.array([6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17])


def chord_label(root: int, quality: str) -> str:
    return NOTE_NAMES[root] + quality


def _states() -> list[tuple[int, str]]:
    return [(r, q) for q in QUALITIES for r in range(12)]


#: Amplitud relativa de cada armónico respecto al anterior.
HARMONIC_DECAY = 0.6


def _note_profile(decay: float) -> np.ndarray:
    """Croma de una nota con sus 6 primeros armónicos (octava, quinta, tercera...)."""
    p = np.zeros(12)
    for k in range(1, 7):
        p[int(round(12 * np.log2(k))) % 12] += decay ** (k - 1)
    return p


def _templates(states: list[tuple[int, str]]) -> np.ndarray:
    # Las plantillas incluyen los armónicos de cada nota: así el 3.er armónico
    # de la quinta (que cae en la séptima mayor) no se confunde con un maj7.
    note = _note_profile(HARMONIC_DECAY)
    t = np.zeros((len(states), 12))
    for i, (root, q) in enumerate(states):
        for interval, w in QUALITIES[q].items():
            t[i] += w * np.roll(note, (root + interval) % 12)
    return t / np.linalg.norm(t, axis=1, keepdims=True)


def chroma_features(y_harm: np.ndarray, y_bass: np.ndarray | None = None) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Croma de agudos (C3–C7, sin el bajo), croma grave (E1–E3) y energía por frame."""
    import librosa

    tuning = float(librosa.estimate_tuning(y=y_harm, sr=SR, bins_per_octave=36)) if np.any(y_harm) else 0.0
    treble = librosa.feature.chroma_cqt(
        y=y_harm, sr=SR, hop_length=HOP, fmin=librosa.note_to_hz("C3"), n_octaves=4, bins_per_octave=36, norm=None, tuning=tuning
    )
    src_bass = y_bass if y_bass is not None else y_harm
    bass = librosa.feature.chroma_cqt(
        y=src_bass, sr=SR, hop_length=HOP, fmin=librosa.note_to_hz("E1"), n_octaves=2, bins_per_octave=36, norm=None, tuning=tuning
    )
    n = min(treble.shape[1], bass.shape[1])
    rms = librosa.feature.rms(y=y_harm, hop_length=HOP)[0]
    rms = np.pad(rms, (0, max(0, n - len(rms))))[:n]
    return treble[:, :n], bass[:, :n], rms


def _segments(beats: np.ndarray, duration: float, n_frames: int) -> tuple[np.ndarray, np.ndarray]:
    """Límites (en segundos) de los segmentos de análisis: un pulso cada uno."""
    import librosa

    times = np.concatenate([[0.0], beats[(beats > 0.05) & (beats < duration - 0.05)], [duration]])
    frames = np.clip(librosa.time_to_frames(times, sr=SR, hop_length=HOP), 0, n_frames)
    keep = np.concatenate([[True], np.diff(frames) > 0])
    return times[keep], frames[keep]


def recognize_chords(
    treble: np.ndarray,
    bass: np.ndarray,
    rms: np.ndarray,
    beats: np.ndarray,
    duration: float,
    stay: float = 0.85,
) -> list[dict]:
    import librosa

    times, frames = _segments(beats, duration, treble.shape[1])
    n_seg = len(times) - 1
    if n_seg <= 0:
        return []
    tr = np.stack([np.median(treble[:, frames[i] : max(frames[i] + 1, frames[i + 1])], axis=1) for i in range(n_seg)], axis=1)
    bs = np.stack([np.median(bass[:, frames[i] : max(frames[i] + 1, frames[i + 1])], axis=1) for i in range(n_seg)], axis=1)
    energy = np.array([rms[frames[i] : max(frames[i] + 1, frames[i + 1])].mean() for i in range(n_seg)])

    # Compresión logarítmica y normalización L2 por segmento.
    tr = np.log1p(3 * tr / (tr.max() + 1e-9))
    tr /= np.linalg.norm(tr, axis=0, keepdims=True) + 1e-9
    bs = bs / (bs.max(axis=0, keepdims=True) + 1e-9)

    states = _states()
    templates = _templates(states)
    score = templates @ tr  # (estados, segmentos)
    roots = np.array([r for r, _ in states])
    score += 0.12 * bs[roots]
    score -= np.array([QUALITY_PRIOR[q] for _, q in states])[:, None]

    # Estado «N» (sin acorde) para silencios y pasajes sin armonía.
    silent = energy < 0.08 * np.percentile(energy, 90)
    n_score = np.where(silent, 2.0, 0.55)
    score = np.vstack([score, n_score])

    beta = 60.0
    probs = np.exp(beta * (score - score.max(axis=0, keepdims=True)))
    probs /= probs.sum(axis=0, keepdims=True)
    trans = librosa.sequence.transition_loop(len(states) + 1, stay)
    path = librosa.sequence.viterbi(probs, trans)

    chords: list[dict] = []
    for i, s in enumerate(path):
        if s == len(states):
            root, quality, label = None, None, "N"
        else:
            root, quality = states[s]
            label = chord_label(root, quality)
        if chords and chords[-1]["label"] == label:
            chords[-1]["end"] = round(float(times[i + 1]), 3)
        else:
            chords.append(
                {"start": round(float(times[i]), 3), "end": round(float(times[i + 1]), 3), "label": label, "root": root, "quality": quality}
            )
    return chords


def estimate_key(treble: np.ndarray, rms: np.ndarray, chords: list[dict]) -> dict:
    profile = (treble * rms[None, :]).sum(axis=1)
    profile = profile / (profile.sum() + 1e-9)
    total = sum(c["end"] - c["start"] for c in chords if c["root"] is not None) or 1.0

    def tonic_share(tonic: int, minor: bool) -> float:
        dur = 0.0
        for c in chords:
            if c["root"] != tonic or c["quality"] is None:
                continue
            is_minor = c["quality"] in ("m", "m7")
            if is_minor == minor and c["quality"] != "dim":
                dur += c["end"] - c["start"]
        return dur / total

    def diatonic_share(tonic: int, minor: bool) -> float:
        allowed = DIATONIC_MINOR if minor else DIATONIC_MAJOR
        dur = 0.0
        for c in chords:
            if c["root"] is None:
                continue
            if ((c["root"] - tonic) % 12, TRIAD_OF[c["quality"]]) in allowed:
                dur += c["end"] - c["start"]
        return dur / total

    candidates = []
    for tonic in range(12):
        for minor, kk in ((False, KK_MAJOR), (True, KK_MINOR)):
            corr = float(np.corrcoef(profile, np.roll(kk, tonic))[0, 1])
            score = corr + 0.6 * tonic_share(tonic, minor) + 0.8 * diatonic_share(tonic, minor)
            candidates.append({"tonic": tonic, "mode": "minor" if minor else "major", "corr": corr, "score": score})
    candidates.sort(key=lambda c: c["score"], reverse=True)
    best = candidates[0]
    margin = best["score"] - candidates[1]["score"]
    return {
        "tonic": best["tonic"],
        "mode": best["mode"],
        "label": NOTE_NAMES[best["tonic"]] + ("m" if best["mode"] == "minor" else ""),
        "confidence": round(float(min(1.0, max(0.0, best["corr"]) * (0.5 + 2 * margin))), 3),
        "alternatives": [
            {"tonic": c["tonic"], "mode": c["mode"], "label": NOTE_NAMES[c["tonic"]] + ("m" if c["mode"] == "minor" else "")}
            for c in candidates[1:4]
        ],
    }
