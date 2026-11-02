"""Transcripción: audio a notas (Basic Pitch), tono de la voz (pYIN) e
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
            progress=lambda p: job.report(0.05 + 0.8 * p, "reconociendo notas"),
            check=job.check,
        )
        job.report(0.9, "ordenando notas", force=True)
        notes = [
            {
                "start": round(s, 4),
                "end": round(e, 4),
                "pitch": int(p),
                "velocity": int(max(1, min(127, round(a * 127)))),
            }
            for s, e, p, a, _ in raw
            if e > s
        ]
        if mode == "mono":
            notes = monophonic(notes, min_note_ms / 1000)
        notes.sort(key=lambda n: (n["start"], n["pitch"]))
        result = {
            "version": NOTES_VERSION,
            "source": "basic_pitch",
            "mode": mode,
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


def track_pitch(y, progress=None, check=None, fmin: float = 65.0, fmax: float = 1100.0):
    """f0 (Hz, 0 = sin voz) y probabilidad de voz cada 10 ms con pYIN, por
    trozos solapados (para informar del avance y poder cancelar)."""
    import librosa
    import numpy as np

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
        if len(seg) < PITCH_FRAME or float(np.max(np.abs(seg))) < 1e-4:
            continue
        f, voiced, vp = librosa.pyin(
            seg, fmin=fmin, fmax=fmax, sr=PITCH_SR, frame_length=PITCH_FRAME, hop_length=PITCH_HOP, center=True
        )
        f = np.where(voiced & np.isfinite(f), f, 0.0)
        first = start // PITCH_HOP
        last = min(n_frames, (start + PITCH_CHUNK) // PITCH_HOP)
        offset = a // PITCH_HOP
        lo, hi = first - offset, last - offset
        hi = min(hi, len(f))
        f0[first : first + (hi - lo)] = f[lo:hi]
        prob[first : first + (hi - lo)] = np.nan_to_num(vp[lo:hi])
        if progress:
            progress((k + 1) / len(starts))
    return f0, prob


@register
class PitchTask(Task):
    id = "track_pitch"
    name = "Afinación vocal"
    description = "Sigue el tono de la voz (pYIN) para medir y corregir su afinación."

    def available(self) -> tuple[bool, str]:
        missing = _missing(PITCH_DEPS)
        if missing:
            return False, "faltan dependencias de Python: " + ", ".join(missing)
        return True, ""

    def run(self, params: dict[str, Any], job: Job) -> dict[str, Any]:
        started = time.monotonic()
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
        )
        result = {
            "version": PITCH_VERSION,
            "source": "pyin",
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
        return {"output": str(out), "voiced_seconds": round(voiced * PITCH_HOP / PITCH_SR, 1)} if out else result


# ---- Instrumentos -----------------------------------------------------------------

PANNS_SR = 32_000
INSTRUMENT_WINDOW = 3.0
INSTRUMENT_HOP = 1.0
#: Umbral de activación por ventana y presencia mínima para listar un instrumento.
ACTIVE_THRESHOLD = 0.2
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
        active = p >= ACTIVE_THRESHOLD
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

        win, hop = int(INSTRUMENT_WINDOW * PANNS_SR), int(INSTRUMENT_HOP * PANNS_SR)
        starts = list(range(0, max(1, len(y) - win + hop), hop))
        batch = 16
        probs = []
        with torch.inference_mode():
            for b in range(0, len(starts), batch):
                job.check()
                chunk = []
                for s in starts[b : b + batch]:
                    w = y[s : s + win]
                    if len(w) < win:
                        w = np.pad(w, (0, win - len(w)))
                    chunk.append(w)
                x = torch.from_numpy(np.stack(chunk)).to(device)
                probs.append(model(x).float().cpu().numpy())
                job.report(0.1 + 0.85 * min(1.0, (b + batch) / len(starts)), "escuchando instrumentos")
        probs = np.concatenate(probs)
        del model
        if device.startswith("cuda"):
            torch.cuda.empty_cache()

        result = {
            "version": INSTRUMENTS_VERSION,
            "source": model_id,
            "window": INSTRUMENT_WINDOW,
            "hop": INSTRUMENT_HOP,
            "duration": round(duration, 3),
            "instruments": summarize_instruments(probs, _labels(), duration),
            "seconds": round(time.monotonic() - started, 2),
            "device": device,
        }
        out = params.get("output")
        if out:
            _write_json(out, result)
        job.report(1.0, "listo", force=True)
        found = ", ".join(i["label"] for i in result["instruments"][:5])
        return {"output": str(out), "found": found, "seconds": result["seconds"]} if out else result
