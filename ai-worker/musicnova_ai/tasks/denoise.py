"""Reducción de ruido de voz con DeepFilterNet3 (ONNX, 48 kHz).

El audio se procesa por trozos de CHUNK segundos solapados OVERLAP segundos y
unidos con fundido cruzado: la red es recurrente, pero sus tensores
intermedios crecen con la duración (c0 ocupa 64 × tramas × 96 valores).
"""

from __future__ import annotations

import time
from typing import Any

from ..jobs import Job
from ..system import best_device
from . import Task, register
from .transcription import _load_mono, _missing

DENOISE_DEPS = ("numpy", "scipy", "soundfile", "librosa", "onnxruntime")
CHUNK = 20.0
OVERLAP = 2.0


def enhance_long(model, y, sr: int, atten_lim_db: float | None, check=None, progress=None):
    """Aplica `model.enhance` por trozos solapados con fundido cruzado lineal."""
    import numpy as np

    chunk, overlap = int(CHUNK * sr), int(OVERLAP * sr)
    if len(y) <= chunk:
        out = model.enhance(y, atten_lim_db)
        if progress:
            progress(1.0)
        return out
    out = np.zeros_like(y)
    step = chunk - overlap
    starts = list(range(0, max(1, len(y) - overlap), step))
    fade_in = np.linspace(0.0, 1.0, overlap, dtype=np.float32)
    for k, a in enumerate(starts):
        if check:
            check()
        b = min(len(y), a + chunk)
        seg = model.enhance(y[a:b], atten_lim_db)
        if k > 0:
            n = min(overlap, len(seg))
            seg[:n] *= fade_in[:n]
            out[a : a + n] *= 1.0 - fade_in[:n]
        out[a:b] += seg
        if progress:
            progress((k + 1) / len(starts))
    return out


@register
class DenoiseTask(Task):
    id = "denoise_voice"
    name = "Limpieza de voz"
    description = "Quita el ruido de fondo de voces y habla (DeepFilterNet3)."
    model_task = "denoise"

    def available(self) -> tuple[bool, str]:
        missing = _missing(DENOISE_DEPS)
        if missing:
            return False, "faltan dependencias de Python: " + ", ".join(missing)
        return True, ""

    def run(self, params: dict[str, Any], job: Job) -> dict[str, Any]:
        import numpy as np
        import soundfile as sf

        from ..vendor import deepfilternet as dfn

        started = time.monotonic()
        model_id = params.get("model_id") or "deepfilternet3"
        if model_id not in self.models.manifests or not self.models.is_installed(model_id):
            raise ValueError("instala el modelo DeepFilterNet3 en «Modelos»")
        device = params.get("device") or "auto"
        if device == "auto":
            device = best_device()
        lim = params.get("atten_lim_db")
        lim = float(lim) if lim not in (None, 0, "0") else None

        job.report(0.0, "leyendo audio", force=True)
        info = sf.info(params["input"])
        audio, sr = sf.read(params["input"], dtype="float32", always_2d=True)
        job.report(0.03, "cargando el modelo", force=True)
        model = dfn.DeepFilterNet3(self.models.model_dir(model_id), device)
        channels = []
        n_ch = audio.shape[1]
        for c in range(n_ch):
            import librosa

            y = audio[:, c]
            if sr != dfn.SR:
                y = librosa.resample(y, orig_sr=sr, target_sr=dfn.SR, res_type="soxr_hq")
            out = enhance_long(
                model,
                np.ascontiguousarray(y, dtype=np.float32),
                dfn.SR,
                lim,
                check=job.check,
                progress=lambda p, c=c: job.report(0.05 + 0.9 * (c + p) / n_ch, "limpiando la voz"),
            )
            if sr != dfn.SR:
                out = librosa.resample(out, orig_sr=dfn.SR, target_sr=sr, res_type="soxr_hq")[: audio.shape[0]]
                out = np.pad(out, (0, audio.shape[0] - len(out)))
            channels.append(out)
        result = np.stack(channels, axis=1)
        out_path = params["output"]
        sf.write(out_path, result, sr, subtype="FLOAT")
        job.report(1.0, "listo", force=True)
        return {"output": str(out_path), "seconds": round(time.monotonic() - started, 2), "duration": info.duration}
