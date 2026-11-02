"""Transcripción: audio a notas (Basic Pitch), tono de la voz (CREPE o pYIN) e
instrumentos presentes (PANNs, etiquetas de AudioSet)."""

from __future__ import annotations

import importlib.util
import json
import os
import time
from pathlib import Path
from typing import Any

from ..jobs import Job
from ..system import best_device
from . import Task, register

NOTES_VERSION = 1
PITCH_VERSION = 1
INSTRUMENTS_VERSION = 1

NOTES_DEPS = ("numpy", "scipy", "soundfile", "librosa", "onnxruntime")
PITCH_DEPS = ("numpy", "soundfile", "librosa")
INSTRUMENT_DEPS = ("numpy", "soundfile", "librosa", "torch")


def _missing(mods) -> list[str]:
    return [m for m in mods if importlib.util.find_spec(m) is None]


def _load_mono(path: str, sr: int):
    import librosa
    import numpy as np
    import soundfile as sf

    audio, file_sr = sf.read(path, dtype="float32", always_2d=True)
    y = audio.mean(axis=1)
    if file_sr != sr:
        y = librosa.resample(y, orig_sr=file_sr, target_sr=sr, res_type="soxr_hq")
    return np.ascontiguousarray(y, dtype=np.float32)


def _write_json(path: str | Path, data: Any) -> Path:
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_suffix(".tmp")
    tmp.write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
    os.replace(tmp, out)
    return out


# ---- Audio a notas -----------------------------------------------------------------


def monophonic(notes: list[dict[str, Any]], min_len: float) -> list[dict[str, Any]]:
    """Deja una sola nota sonando a la vez: en cada solape gana la más fuerte y
    la otra se recorta (o desaparece si queda más corta que `min_len`)."""
    out: list[dict[str, Any]] = []
    for n in sorted(notes, key=lambda n: (n["start"], -n["velocity"])):
        n = dict(n)
        while out and out[-1]["end"] > n["start"]:
            prev = out[-1]
            if n["velocity"] > prev["velocity"]:
                prev["end"] = n["start"]
                if prev["end"] - prev["start"] < min_len:
                    out.pop()
                    continue
            else:
                n["start"] = prev["end"]
            break
        if n["end"] - n["start"] >= min_len:
            out.append(n)
    return out


# Curvas de tono dentro de cada nota («pitch bends»), en cents respecto a su altura.
BEND_TOLERANCE = 4.0  # error máximo al simplificar la curva
BEND_MIN = 12.0  # si nunca se aleja más que esto, la nota va sin curva
BEND_LIMIT = 200.0  # ±2 semitonos: el rango estándar de pitch bend en MIDI
BEND_OUTLIER = 250.0  # desvíos mayores de CREPE son saltos de octava o de nota


def simplify_curve(points: list[tuple[float, float]], tol: float) -> list[tuple[float, float]]:
    """Ramer–Douglas–Peucker: menos puntos, error vertical ≤ `tol`."""
    if len(points) <= 2:
        return list(points)
    keep = [False] * len(points)
    keep[0] = keep[-1] = True
    stack = [(0, len(points) - 1)]
    while stack:
        a, b = stack.pop()
        (ta, ca), (tb, cb) = points[a], points[b]
        worst, idx = 0.0, -1
        for i in range(a + 1, b):
            t, c = points[i]
            line = ca + (cb - ca) * ((t - ta) / (tb - ta) if tb > ta else 0.0)
            d = abs(c - line)
            if d > worst:
                worst, idx = d, i
        if worst > tol:
            keep[idx] = True
            stack += [(a, idx), (idx, b)]
    return [p for p, k in zip(points, keep) if k]


def note_bends(start: float, end: float, curve: list[tuple[float, float]]) -> list[list[float]]:
    """Curva de una nota como puntos [segundos desde el inicio, cents]: mediana
    de 5 puntos, limitada a ±2 semitonos y simplificada. Vacía si apenas se mueve."""
    import numpy as np

    pts = [(t, c) for t, c in curve if start - 1e-6 <= t <= end + 1e-6 and np.isfinite(c)]
    if len(pts) < 3:
        return []
    t = np.array([p[0] for p in pts])
    c = np.array([p[1] for p in pts])
    if len(c) >= 5:
        padded = np.pad(c, 2, mode="edge")
        c = np.median(np.lib.stride_tricks.sliding_window_view(padded, 5), axis=1)
    c = np.clip(c, -BEND_LIMIT, BEND_LIMIT)
    if np.max(np.abs(c)) < BEND_MIN:
        return []
    simple = simplify_curve(list(zip((t - start).tolist(), c.tolist())), BEND_TOLERANCE)
    return [[round(max(0.0, dt), 3), round(v, 1)] for dt, v in simple]


def crepe_curve(f0, hop: float, pitch: int, start: float, end: float) -> list[tuple[float, float]] | None:
    """Puntos (s, cents) de la f0 de CREPE dentro de una nota. `None` si hay voz
    en menos de la mitad de la nota (entonces se usa el contorno de Basic Pitch)."""
    import numpy as np

    a, b = int(np.ceil(start / hop)), int(np.floor(end / hop))
    if b - a < 3:
        return None
    seg = np.asarray(f0[max(0, a) : max(0, b) + 1], dtype=np.float64)
    if len(seg) == 0:
        return None
    ref = 440.0 * 2 ** ((pitch - 69) / 12)
    with np.errstate(divide="ignore", invalid="ignore"):
        cents = np.where(seg > 0, 1200 * np.log2(seg / ref), np.nan)
    ok = np.isfinite(cents) & (np.abs(np.nan_to_num(cents, nan=1e9)) < BEND_OUTLIER)
    if ok.sum() < 0.5 * len(seg):
        return None
    return [((a + k) * hop, float(v)) for k, v in enumerate(cents) if ok[k]]


# En monofónico, notas iguales separadas por menos que esto son la misma nota
# (Basic Pitch vuelve a disparar el inicio en cada ciclo de un vibrato amplio).
MERGE_GAP = 0.03


def merge_repeats(notes: list[dict[str, Any]], gap: float) -> list[dict[str, Any]]:
    """Une notas consecutivas de la misma altura casi pegadas (en orden de inicio)."""
    out: list[dict[str, Any]] = []
    for n in sorted(notes, key=lambda n: n["start"]):
        prev = out[-1] if out else None
        if prev and prev["pitch"] == n["pitch"] and n["start"] - prev["end"] <= gap:
            prev["end"] = max(prev["end"], n["end"])
            prev["velocity"] = max(prev["velocity"], n["velocity"])
            prev["_curve"] = prev.get("_curve", []) + n.get("_curve", [])
        else:
            out.append(dict(n))
    return out


@register
class NotesTask(Task):
    id = "transcribe_notes"
    name = "Audio a MIDI"
    description = "Convierte una pista en notas (Basic Pitch, polifónico)."

    def available(self) -> tuple[bool, str]:
        missing = _missing(NOTES_DEPS)
        if missing:
            return False, "faltan dependencias de Python: " + ", ".join(missing)
        return True, ""

    def run(self, params: dict[str, Any], job: Job) -> dict[str, Any]:
        from ..vendor import basic_pitch as bp

        started = time.monotonic()
        device = params.get("device") or "auto"
        if device == "auto":
            device = best_device()
        mode = params.get("mode") or "poly"
        if mode not in ("poly", "mono"):
            raise ValueError(f"modo desconocido: {mode}")
        min_note_ms = float(params.get("min_note_ms", bp.DEFAULT_MINIMUM_NOTE_LENGTH_MS))
        # Curva fina de cada nota: CREPE en monofónico si está instalado; si no,
        # el contorno de Basic Pitch. "none" la desactiva.
        bend_source = params.get("bends") or "auto"
        if bend_source == "auto":
            bend_source = "crepe" if mode == "mono" and crepe_available() else "contour"
        if bend_source not in ("crepe", "contour", "none"):
            raise ValueError(f"origen de curvas desconocido: {bend_source}")

        job.report(0.0, "leyendo audio", force=True)
        y = _load_mono(params["input"], bp.AUDIO_SAMPLE_RATE)
        if len(y) < bp.AUDIO_SAMPLE_RATE // 2:
            raise ValueError("el audio es demasiado corto para transcribirlo")
        job.check()
        job.report(0.05, "cargando el modelo", force=True)
        model = bp.Model(device=device)
        raw = bp.predict_notes(
            y,
            model,
            onset_threshold=float(params.get("onset_threshold", bp.DEFAULT_ONSET_THRESHOLD)),
            frame_threshold=float(params.get("frame_threshold", bp.DEFAULT_FRAME_THRESHOLD)),
            minimum_note_length_ms=min_note_ms,
            minimum_frequency=params.get("min_hz"),
            maximum_frequency=params.get("max_hz"),
            progress=lambda p: job.report(0.05 + (0.6 if bend_source == "crepe" else 0.8) * p, "reconociendo notas"),
            check=job.check,
        )
        notes = [
            {
                "start": round(s0, 4),
                "end": round(e0, 4),
                "pitch": int(p),
                "velocity": int(max(1, min(127, round(a * 127)))),
                # Contorno de Basic Pitch en cents (monophonic() conserva las claves).
                "_curve": [(t, v * 100.0 / 3.0) for t, v in curve],
            }
            for s0, e0, p, a, curve in raw
            if e0 > s0
        ]
        if mode == "mono":
            notes = monophonic(notes, min_note_ms / 1000)
            notes = merge_repeats(notes, float(params.get("merge_gap", MERGE_GAP)))
        if bend_source != "none":
            f0 = None
            if bend_source == "crepe":
                job.check()
                job.report(0.68, "siguiendo el tono fino", force=True)
                y16 = _load_mono(params["input"], PITCH_SR)
                f0, _ = track_pitch(
                    y16,
                    progress=lambda p: job.report(0.68 + 0.22 * p, "siguiendo el tono fino"),
                    check=job.check,
                    fmin=32.0,  # el intervalo más grave de CREPE es 31,7 Hz
                    fmax=1975.0,
                    engine="crepe",
                    model="full",
                    device=device,
                )
            for n in notes:
                curve = None
                if f0 is not None:
                    curve = crepe_curve(f0, PITCH_HOP / PITCH_SR, n["pitch"], n["start"], n["end"])
                if curve is None:
                    curve = n["_curve"]
                bends = note_bends(n["start"], n["end"], curve)
                if bends:
                    n["bends"] = bends
        for n in notes:
            n.pop("_curve", None)
        job.report(0.92, "ordenando notas", force=True)
        notes.sort(key=lambda n: (n["start"], n["pitch"]))
        result = {
            "version": NOTES_VERSION,
            "source": "basic_pitch",
            "mode": mode,
            "bends": bend_source,
            "notes": notes,
            "seconds": round(time.monotonic() - started, 2),
        }
        out = params.get("output")
        if out:
            _write_json(out, result)
        job.report(1.0, "listo", force=True)
        return {"output": str(out), "notes": len(notes), "seconds": result["seconds"]} if out else result


# ---- Tono de la voz ---------------------------------------------------------------

PITCH_SR = 16_000
PITCH_HOP = 160  # 10 ms
PITCH_FRAME = 1024
PITCH_CHUNK = 20 * PITCH_SR
PITCH_OVERLAP = PITCH_SR // 2
PITCH_ENGINES = ("crepe", "pyin")
CREPE_MODELS = ("full", "tiny")
# Periodicidad mínima para dar un fotograma por cantado (medido con Vocadito).
CREPE_VOICING = 0.6
# Por debajo de este nivel (RMS en dBFS) el fotograma es silencio.
CREPE_SILENCE_DB = -60.0


def frame_db(seg):
    """RMS en dBFS de ventanas de PITCH_FRAME centradas cada PITCH_HOP muestras
    (los mismos fotogramas que pYIN y CREPE con relleno)."""
    import numpy as np

    pad = PITCH_FRAME // 2
    x = np.pad(np.asarray(seg, dtype=np.float64), pad)
    c = np.concatenate([[0.0], np.cumsum(x * x)])
    n = len(seg) // PITCH_HOP + 1
    starts = np.arange(n) * PITCH_HOP
    ends = np.minimum(starts + PITCH_FRAME, len(x))
    rms = np.sqrt((c[ends] - c[starts]) / PITCH_FRAME)
    return 20 * np.log10(np.maximum(rms, 1e-10))


def crepe_available() -> bool:
    return not _missing(("torch", "torchcrepe"))


def _pyin_segment(fmin: float, fmax: float):
    def run(seg):
        import librosa
        import numpy as np

        f, voiced, vp = librosa.pyin(
            seg, fmin=fmin, fmax=fmax, sr=PITCH_SR, frame_length=PITCH_FRAME, hop_length=PITCH_HOP, center=True
        )
        return np.where(voiced & np.isfinite(f), f, 0.0), np.nan_to_num(vp)

    return run


def _crepe_decoder():
    """Viterbi para elegir el intervalo de cada fotograma y media ponderada de
    ±4 intervalos a su alrededor para la f0 fina (como el CREPE original). Los
    decodificadores de torchcrepe devuelven el centro del intervalo con ruido
    triangular de ±20 cents, demasiado para medir afinación."""
    import torch
    import torchcrepe

    def decode(probs):
        bins, _ = torchcrepe.decode.viterbi(probs)
        p = torch.nan_to_num(probs, neginf=0.0).clamp_min(0.0)
        offsets = torch.arange(-4, 5, device=p.device)[None, :, None]
        idx = (bins[:, None, :] + offsets).clamp(0, p.size(1) - 1)
        w = p.gather(1, idx)
        cents = torchcrepe.CENTS_PER_BIN * idx.to(p.dtype) + 1997.3794084376191
        fine = (w * cents).sum(1) / w.sum(1).clamp_min(1e-9)
        coarse = torchcrepe.CENTS_PER_BIN * bins.to(p.dtype) + 1997.3794084376191
        fine = torch.where(w.sum(1) > 1e-9, fine, coarse)
        return bins, torchcrepe.convert.cents_to_frequency(fine)

    return decode


def _crepe_segment(fmin: float, fmax: float, model: str, device: str, voicing: float):
    import torch
    import torchcrepe

    decoder = _crepe_decoder()

    def run(seg):
        import numpy as np

        audio = torch.from_numpy(np.ascontiguousarray(seg, dtype=np.float32))[None]
        with torch.inference_mode():
            f, per = torchcrepe.predict(
                audio,
                PITCH_SR,
                hop_length=PITCH_HOP,
                fmin=fmin,
                fmax=fmax,
                model=model,
                decoder=decoder,
                return_periodicity=True,
                batch_size=1024,
                device=device,
                pad=True,
            )
            per = torchcrepe.filter.median(per, 3)
        f = f[0].cpu().numpy()
        per = np.nan_to_num(per[0].cpu().numpy())
        # El umbral de silencio de torchcrepe promedia la sonoridad de todas las
        # bandas y da por mudos los tonos limpios; se usa el RMS de cada ventana.
        db = frame_db(seg)
        k = min(len(per), len(db))
        per[:k][db[:k] < CREPE_SILENCE_DB] = 0.0
        return np.where(per >= voicing, f, 0.0), per

    return run


def track_pitch(
    y,
    progress=None,
    check=None,
    fmin: float = 65.0,
    fmax: float = 1100.0,
    engine: str = "pyin",
    model: str = "full",
    device: str = "cpu",
    voicing: float = CREPE_VOICING,
):
    """f0 (Hz, 0 = sin voz) y confianza cada 10 ms, por trozos solapados (para
    informar del avance y poder cancelar). `engine` es "crepe" (red neuronal,
    más precisa) o "pyin" (estadístico, sin modelos)."""
    import numpy as np

    if engine == "crepe":
        segment = _crepe_segment(fmin, fmax, model, device, voicing)
    elif engine == "pyin":
        segment = _pyin_segment(fmin, fmax)
    else:
        raise ValueError(f"motor de tono desconocido: {engine}")
    n_frames = len(y) // PITCH_HOP + 1
    f0 = np.zeros(n_frames, dtype=np.float32)
    prob = np.zeros(n_frames, dtype=np.float32)
    starts = list(range(0, len(y), PITCH_CHUNK))
    for k, start in enumerate(starts):
        if check:
            check()
        a = max(0, start - PITCH_OVERLAP)
        b = min(len(y), start + PITCH_CHUNK + PITCH_OVERLAP)
        seg = y[a:b]
        if len(seg) >= PITCH_FRAME and float(np.max(np.abs(seg))) >= 1e-4:
            f, conf = segment(seg)
            first = start // PITCH_HOP
            last = min(n_frames, (start + PITCH_CHUNK) // PITCH_HOP)
            offset = a // PITCH_HOP
            lo, hi = first - offset, min(last - offset, len(f))
            f0[first : first + (hi - lo)] = f[lo:hi]
            prob[first : first + (hi - lo)] = conf[lo:hi]
        if progress:
            progress((k + 1) / len(starts))
    return f0, prob


@register
class PitchTask(Task):
    id = "track_pitch"
    name = "Afinación vocal"
    description = "Sigue el tono de la voz (CREPE o pYIN) para medir y corregir su afinación."

    def available(self) -> tuple[bool, str]:
        missing = _missing(PITCH_DEPS)
        if missing:
            return False, "faltan dependencias de Python: " + ", ".join(missing)
        return True, ""

    def run(self, params: dict[str, Any], job: Job) -> dict[str, Any]:
        started = time.monotonic()
        engine = params.get("engine") or "auto"
        if engine == "auto":
            engine = "crepe" if crepe_available() else "pyin"
        if engine == "crepe" and not crepe_available():
            raise RuntimeError("CREPE no está instalado (falta el paquete torchcrepe)")
        model = params.get("model") or "full"
        if model not in CREPE_MODELS:
            raise ValueError(f"modelo de CREPE desconocido: {model}")
        device = params.get("device") or "auto"
        if device == "auto":
            device = best_device()
        job.report(0.0, "leyendo audio", force=True)
        y = _load_mono(params["input"], PITCH_SR)
        if len(y) < PITCH_SR:
            raise ValueError("el audio es demasiado corto para seguir su tono")
        f0, prob = track_pitch(
            y,
            progress=lambda p: job.report(0.02 + 0.95 * p, "siguiendo el tono de la voz"),
            check=job.check,
            fmin=float(params.get("min_hz", 65.0)),
            fmax=float(params.get("max_hz", 1100.0)),
            engine=engine,
            model=model,
            device=device,
        )
        result = {
            "version": PITCH_VERSION,
            "source": f"crepe-{model}" if engine == "crepe" else "pyin",
            "hop": PITCH_HOP / PITCH_SR,
            "f0": [round(float(v), 2) for v in f0],
            "confidence": [round(float(v), 2) for v in prob],
            "seconds": round(time.monotonic() - started, 2),
        }
        out = params.get("output")
        if out:
            _write_json(out, result)
        job.report(1.0, "listo", force=True)
        voiced = int((f0 > 0).sum())
        summary = {"output": str(out), "voiced_seconds": round(voiced * PITCH_HOP / PITCH_SR, 1), "source": result["source"]}
        return summary if out else result


# ---- Instrumentos -----------------------------------------------------------------

PANNS_SR = 32_000
INSTRUMENT_WINDOW = 3.0
INSTRUMENT_HOP = 1.0
#: Umbral de activación por ventana y presencia mínima para listar un instrumento.
ACTIVE_THRESHOLD = 0.2
# Umbral propio de cada grupo: las probabilidades de AudioSet tienen escalas muy
# distintas según la clase (el piano rara vez pasa de 0,2 aunque suene solo).
GROUP_THRESHOLD: dict[str, float] = {}
MIN_COVERAGE = 0.05

# Instrumentos de AudioSet que se muestran, con su nombre en español. Un
# instrumento puede reunir varias clases (se toma el máximo).
INSTRUMENTS: list[tuple[str, str, tuple[str, ...]]] = [
    ("vocals", "Voz cantada", ("Singing", "Male singing", "Female singing", "Choir", "Rapping")),
    ("speech", "Voz hablada", ("Speech",)),
    ("piano", "Piano", ("Piano", "Electric piano")),
    ("keyboard", "Teclado / órgano", ("Keyboard (musical)", "Organ", "Electronic organ", "Hammond organ", "Harpsichord")),
    ("synth", "Sintetizador", ("Synthesizer", "Sampler")),
    ("acoustic_guitar", "Guitarra acústica", ("Acoustic guitar", "Steel guitar, slide guitar")),
    ("electric_guitar", "Guitarra eléctrica", ("Electric guitar",)),
    ("guitar", "Guitarra", ("Guitar", "Plucked string instrument", "Strum")),
    ("bass", "Bajo", ("Bass guitar", "Double bass")),
    ("drums", "Batería", ("Drum kit", "Drum", "Snare drum", "Bass drum", "Drum machine", "Cymbal", "Hi-hat")),
    ("percussion", "Percusión", ("Percussion", "Tambourine", "Maraca", "Cowbell", "Shaker", "Bongo", "Conga", "Tabla", "Timpani")),
    ("strings", "Cuerdas", ("Bowed string instrument", "String section", "Violin, fiddle", "Cello", "Orchestra")),
    ("violin", "Violín", ("Violin, fiddle", "Pizzicato")),
    ("brass", "Metales", ("Brass instrument", "Trumpet", "Trombone", "French horn")),
    ("saxophone", "Saxofón", ("Saxophone",)),
    ("woodwind", "Viento madera", ("Wind instrument, woodwind instrument", "Flute", "Clarinet", "Oboe", "Bagpipes")),
    ("harmonica", "Armónica", ("Harmonica",)),
    ("accordion", "Acordeón", ("Accordion",)),
    ("mallets", "Láminas", ("Marimba, xylophone", "Glockenspiel", "Vibraphone", "Steelpan")),
    ("banjo", "Banjo / mandolina", ("Banjo", "Mandolin", "Ukulele", "Sitar", "Zither")),
    ("harp", "Arpa", ("Harp",)),
]


# Stem del que se lee cada grupo cuando el proyecto está separado (el primero
# que exista; ninguno = la mezcla); el resto de grupos sale del stem «other».
# Sin stems, todo sale de la mezcla.
# Medido con IRMAS: separar con htdemucs sube el F1 del 52 % al 60 % (la voz
# pasa del 41 % al 94 % de aciertos y el piano del 11 % al 36 %).
GROUP_STEMS: dict[str, tuple[str, ...]] = {
    "vocals": ("vocals",),
    # La voz cantada aislada se parece mucho al habla para PANNs: leída del
    # stem de voz daba «voz hablada» en 68 de 157 fragmentos de IRMAS sin
    # habla; desde la mezcla, en 14. Por eso sale de la mezcla.
    "speech": (),
    "drums": ("drums",),
    "percussion": ("drums",),
    "bass": ("bass",),
    "piano": ("piano", "other"),
    "keyboard": ("piano", "other"),
    "acoustic_guitar": ("guitar", "other"),
    "electric_guitar": ("guitar", "other"),
    "guitar": ("guitar", "other"),
}
STEM_NAMES = ("vocals", "drums", "bass", "guitar", "piano", "other")


def combine_stem_probs(mix, stems: dict[str, Any], labels: list[str]):
    """Probabilidades por ventana (n × 527) tomando cada grupo de su stem."""
    out = mix.copy()
    index = {name: i for i, name in enumerate(labels)}
    for key, _, classes in INSTRUMENTS:
        src = next((stems[s] for s in GROUP_STEMS.get(key, ("other",)) if s in stems), None)
        if src is None:
            continue
        for c in classes:
            if c in index:
                out[:, index[c]] = src[:, index[c]]
    return out


def _labels() -> list[str]:
    import csv

    path = Path(__file__).resolve().parent.parent / "vendor" / "panns" / "class_labels_indices.csv"
    with open(path, encoding="utf-8", newline="") as f:
        rows = list(csv.DictReader(f))
    return [r["display_name"] for r in rows]


def segments_from(active, hop: float, window: float, min_gap: float = 2.0) -> list[list[float]]:
    """Tramos (inicio, fin) en segundos a partir de ventanas activas; une huecos cortos."""
    segs: list[list[float]] = []
    for i, on in enumerate(active):
        if not on:
            continue
        a, b = i * hop, i * hop + window
        if segs and a - segs[-1][1] <= min_gap:
            segs[-1][1] = max(segs[-1][1], b)
        else:
            segs.append([a, b])
    return [[round(a, 2), round(b, 2)] for a, b in segs]


def summarize_instruments(probs, labels: list[str], duration: float) -> list[dict[str, Any]]:
    """Agrupa las probabilidades por ventana (n_ventanas × 527) en instrumentos."""
    import numpy as np

    index = {name: i for i, name in enumerate(labels)}
    out = []
    for key, label, classes in INSTRUMENTS:
        cols = [index[c] for c in classes if c in index]
        if not cols:
            continue
        p = probs[:, cols].max(axis=1)
        active = p >= GROUP_THRESHOLD.get(key, ACTIVE_THRESHOLD)
        coverage = float(active.mean()) if len(active) else 0.0
        if coverage < MIN_COVERAGE:
            continue
        top = np.sort(p)[-max(1, len(p) // 10) :]
        segs = segments_from(active, INSTRUMENT_HOP, INSTRUMENT_WINDOW)
        if segs:
            segs[-1][1] = min(segs[-1][1], round(duration, 2))
        out.append(
            {
                "id": key,
                "label": label,
                "confidence": round(float(top.mean()), 3),
                "coverage": round(coverage, 3),
                "segments": segs,
            }
        )
    # «Guitarra» genérica solo si no se distingue acústica o eléctrica.
    ids = {i["id"] for i in out}
    if "guitar" in ids and ids & {"acoustic_guitar", "electric_guitar"}:
        out = [i for i in out if i["id"] != "guitar"]
    out.sort(key=lambda i: (-i["confidence"] * i["coverage"]))
    return out


@register
class InstrumentsTask(Task):
    id = "detect_instruments"
    name = "Detección de instrumentos"
    description = "Reconoce los instrumentos de la canción y cuándo suenan (PANNs, AudioSet)."
    model_task = "instruments"

    def available(self) -> tuple[bool, str]:
        missing = _missing(INSTRUMENT_DEPS)
        if missing:
            return False, "faltan dependencias de Python: " + ", ".join(missing)
        return True, ""

    def _model_id(self, requested: str | None) -> str:
        if requested:
            if requested not in self.models.manifests or not self.models.is_installed(requested):
                raise ValueError(f"el modelo {requested} no está instalado")
            return requested
        installed = [m for m in self.models.manifests.values() if m.task == "instruments" and self.models.is_installed(m.id)]
        if not installed:
            raise ValueError("instala un modelo de detección de instrumentos (PANNs) en «Modelos»")
        installed.sort(key=lambda m: m.extra.get("priority", 0), reverse=True)
        return installed[0].id

    def run(self, params: dict[str, Any], job: Job) -> dict[str, Any]:
        import numpy as np
        import torch

        from ..vendor import panns

        started = time.monotonic()
        device = params.get("device") or "auto"
        if device == "auto":
            device = best_device()
        model_id = self._model_id(params.get("model_id"))
        manifest = self.models.get(model_id)

        job.report(0.0, "leyendo audio", force=True)
        y = _load_mono(params["input"], PANNS_SR)
        duration = len(y) / PANNS_SR
        if duration < 2:
            raise ValueError("el audio es demasiado corto para reconocer instrumentos")
        job.check()
        job.report(0.05, "cargando el modelo", force=True)
        model = panns.load(manifest.extra.get("arch", "Cnn14"), self.models.model_dir(model_id) / manifest.files[0].name, device)

        stem_paths = {k: v for k, v in (params.get("stems") or {}).items() if k in STEM_NAMES and v}
        sources = [("mix", y)] + [(k, None) for k in stem_paths]
        win, hop = int(INSTRUMENT_WINDOW * PANNS_SR), int(INSTRUMENT_HOP * PANNS_SR)
        starts = list(range(0, max(1, len(y) - win + hop), hop))
        batch = 16
        results: dict[str, Any] = {}
        for n_src, (name, audio) in enumerate(sources):
            if audio is None:
                # Mismo largo que la mezcla para que las ventanas coincidan.
                audio = _load_mono(stem_paths[name], PANNS_SR)[: len(y)]
                audio = np.pad(audio, (0, len(y) - len(audio)))
            probs = []
            with torch.inference_mode():
                for b in range(0, len(starts), batch):
                    job.check()
                    chunk = []
                    for s in starts[b : b + batch]:
                        w = audio[s : s + win]
                        if len(w) < win:
                            w = np.pad(w, (0, win - len(w)))
                        chunk.append(w)
                    x = torch.from_numpy(np.stack(chunk)).to(device)
                    probs.append(model(x).float().cpu().numpy())
                    done = (n_src + min(1.0, (b + batch) / len(starts))) / len(sources)
                    job.report(0.1 + 0.85 * done, "escuchando instrumentos" if name == "mix" else f"escuchando el stem «{name}»")
            results[name] = np.concatenate(probs)
        labels = _labels()
        probs = combine_stem_probs(results.pop("mix"), results, labels)
        del model
        if device.startswith("cuda"):
            torch.cuda.empty_cache()

        result = {
            "version": INSTRUMENTS_VERSION,
            "source": model_id,
            "window": INSTRUMENT_WINDOW,
            "hop": INSTRUMENT_HOP,
            "duration": round(duration, 3),
            "instruments": summarize_instruments(probs, labels, duration),
            "stems": sorted(stem_paths),
            "seconds": round(time.monotonic() - started, 2),
            "device": device,
        }
        out = params.get("output")
        if out:
            _write_json(out, result)
        job.report(1.0, "listo", force=True)
        found = ", ".join(i["label"] for i in result["instruments"][:5])
        return {"output": str(out), "found": found, "seconds": result["seconds"]} if out else result
