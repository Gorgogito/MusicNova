"""Tarea de análisis musical completo (pulsos, compás, tempo, tonalidad, acordes, secciones)."""

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

ANALYSIS_VERSION = 1
CORE_DEPS = ("numpy", "scipy", "soundfile", "librosa")
BEAT_THIS_DEPS = ("torch", "torchaudio", "einops", "rotary_embedding_torch", "soxr")
# Stems que aportan armonía (se excluyen batería y voz).
HARMONIC_STEMS = ("bass", "guitar", "piano", "other", "instrumental")


def _missing(mods) -> list[str]:
    return [m for m in mods if importlib.util.find_spec(m) is None]


def _load_mono(path: str):
    import librosa
    import numpy as np
    import soundfile as sf

    from ..analysis import SR

    audio, sr = sf.read(path, dtype="float32", always_2d=True)
    y = audio.mean(axis=1)
    if sr != SR:
        y = librosa.resample(y, orig_sr=sr, target_sr=SR, res_type="soxr_hq")
    return np.ascontiguousarray(y, dtype=np.float32)


@register
class AnalyzeTask(Task):
    id = "analyze"
    name = "Análisis musical"
    description = "Detecta pulsos, compás, tempo, tonalidad, acordes y secciones."
    model_task = "beats"

    def available(self) -> tuple[bool, str]:
        missing = _missing(CORE_DEPS)
        if missing:
            return False, "faltan dependencias de Python: " + ", ".join(missing)
        return True, ""

    def _beat_model(self, requested: str | None) -> str | None:
        """Modelo de pulsos a usar: el pedido o el primero instalado (prefiere el completo)."""
        if requested == "none" or _missing(BEAT_THIS_DEPS):
            return None
        if requested:
            if requested not in self.models.manifests or not self.models.is_installed(requested):
                raise ValueError(f"el modelo de pulsos {requested} no está instalado")
            return requested
        installed = [m for m in self.models.manifests.values() if m.task == "beats" and self.models.is_installed(m.id)]
        installed.sort(key=lambda m: m.extra.get("priority", 0), reverse=True)
        return installed[0].id if installed else None

    def run(self, params: dict[str, Any], job: Job) -> dict[str, Any]:
        import numpy as np

        from ..analysis import beats as bt
        from ..analysis import harmony, structure

        started = time.monotonic()
        device = params.get("device") or "auto"
        if device == "auto":
            device = best_device()
        stems: dict[str, str] = params.get("stems") or {}
        report = lambda p, msg: job.report(p, msg, force=True)  # noqa: E731

        report(0.0, "leyendo audio")
        y = _load_mono(params["input"])
        duration = len(y) / 22_050
        if duration < 5:
            raise ValueError("el audio es demasiado corto para analizarlo (mínimo 5 s)")

        # 1. Pulsos y tiempos fuertes.
        model_id = self._beat_model(params.get("beat_model_id"))
        if model_id:
            report(0.05, "cargando modelo de pulsos")
            manifest = self.models.get(model_id)
            path = self.models.model_dir(model_id) / manifest.files[0].name
            beats, downbeats = bt.beats_beat_this(y, path, device, job.check, lambda p, m: job.report(0.05 + 0.3 * p, m))
            if device.startswith("cuda"):
                import torch

                torch.cuda.empty_cache()
            source = model_id
        else:
            report(0.05, "detectando pulsos (sin modelo)")
            beats = bt.beats_librosa(y)
            downbeats = np.array([])
            source = "librosa"
        job.check()
        if len(downbeats) < 2:
            downbeats, bpb = bt.estimate_downbeats(y, beats)
        else:
            bpb = bt.beats_per_bar(beats, downbeats)
        bpm, curve, stable = bt.tempo_summary(beats)

        # 2. Armonía: con stems se analiza solo lo armónico (sin batería ni voz).
        report(0.4, "calculando cromagrama")
        harm_paths = [stems[k] for k in HARMONIC_STEMS if k in stems and k != "bass"]
        bass_path = stems.get("bass")
        if harm_paths or bass_path:
            parts = [_load_mono(p) for p in harm_paths + ([bass_path] if bass_path else [])]
            n = min(len(p) for p in parts)
            y_harm = np.sum([p[:n] for p in parts], axis=0)
            y_bass = _load_mono(bass_path)[:n] if bass_path else None
            chord_source = "stems"
        else:
            import librosa

            y_harm = librosa.effects.harmonic(y, margin=2.0)
            y_bass = None
            chord_source = "mix"
        job.check()
        treble, bass, rms = harmony.chroma_features(y_harm, y_bass)
        job.check()
        report(0.65, "reconociendo acordes")
        chords = harmony.recognize_chords(treble, bass, rms, beats, duration)
        report(0.75, "estimando tonalidad")
        key = harmony.estimate_key(treble, rms, chords)
        job.check()

        # 3. Secciones.
        report(0.8, "buscando secciones")
        sections = structure.detect_sections(y, treble, beats, downbeats, bpb, duration)

        result = {
            "version": ANALYSIS_VERSION,
            "duration": round(duration, 3),
            "beat_source": source,
            "chord_source": chord_source,
            "bpm": bpm,
            "tempo_stable": stable,
            "tempo_curve": curve,
            "beats_per_bar": bpb,
            "beats": [round(float(b), 4) for b in beats],
            "downbeats": [round(float(d), 4) for d in downbeats],
            "key": key,
            "chords": chords,
            "sections": sections,
            "seconds": round(time.monotonic() - started, 2),
            "device": device if model_id else "cpu",
        }
        out = params.get("output")
        if out:
            out = Path(out)
            out.parent.mkdir(parents=True, exist_ok=True)
            tmp = out.with_suffix(".tmp")
            tmp.write_text(json.dumps(result, ensure_ascii=False), encoding="utf-8")
            os.replace(tmp, out)
        report(1.0, "listo")
        return {"output": str(out) if out else None, "bpm": bpm, "key": key["label"], "seconds": result["seconds"]} if out else result
