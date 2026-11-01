"""Separación de fuentes con varias familias de modelos.

- ``demucs``: HTDemucs y combinaciones (bags) como htdemucs_ft.
- ``mel_roformer``: Mel-Band RoFormer (voz de alta calidad; el instrumental se
  obtiene restando la voz a la mezcla).

Cada backend procesa el audio por bloques con fundido cruzado; entre bloques se
informa del progreso y se comprueba la cancelación.
"""

from __future__ import annotations

import importlib.util
import time
from pathlib import Path
from typing import Any, Callable

from ..jobs import Job
from ..models import ModelManifest
from ..system import best_device
from . import Task, register

# Bloques de Demucs más cortos en CPU para que cancelar responda en pocos segundos.
CHUNK_SECONDS = {"cuda": 30.0, "mps": 20.0, "cpu": 10.0}
CROSSFADE_SECONDS = 2.0

STEM_LABELS = {
    "vocals": "Voz",
    "instrumental": "Instrumental",
    "drums": "Batería",
    "bass": "Bajo",
    "guitar": "Guitarra",
    "piano": "Piano",
    "other": "Otros",
}

# Dependencias de Python por familia de modelo.
FAMILY_DEPS = {
    "demucs": ("torch", "numpy", "soundfile", "demucs", "julius"),
    "mel_roformer": ("torch", "numpy", "soundfile", "julius", "librosa", "beartype", "rotary_embedding_torch", "einops"),
}

Progress = Callable[[float, str], None]


def _resolve_device(requested: str | None) -> str:
    if requested in (None, "", "auto"):
        return best_device()
    return requested


def _resample(wav, sr: int, target: int):
    if sr == target:
        return wav
    import julius

    return julius.resample_frac(wav, sr, target)


# ---------------------------------------------------------------- Demucs


def _load_demucs(path: Path):
    import torch
    from demucs.states import load_model

    # Los checkpoints de Demucs son pickles con la clase del modelo; solo se
    # cargan archivos descargados desde el manifiesto y verificados por SHA-256.
    pkg = torch.load(path, map_location="cpu", weights_only=False)
    model = load_model(pkg)
    model.eval()
    return model


def _separate_demucs(manifest: ModelManifest, model_dir: Path, wav, sr: int, device: str, params, job: Job, progress: Progress):
    import torch
    from demucs.apply import apply_model

    shifts = int(params.get("shifts", 1))
    overlap = float(params.get("overlap", 0.25))
    models = [_load_demucs(model_dir / f.name) for f in manifest.files]
    sources: list[str] = list(models[0].sources)
    samplerate: int = models[0].samplerate
    # weights[i][s]: peso del modelo i para la fuente s (combinaciones tipo htdemucs_ft).
    weights = manifest.extra.get("weights") or [[1.0] * len(sources) for _ in models]
    if len(weights) != len(models) or any(len(w) != len(sources) for w in weights):
        raise ValueError(f"pesos inválidos en el manifiesto de {manifest.id}")
    job.check()

    wav = _resample(wav, sr, samplerate)
    # Normalización igual que la CLI de Demucs.
    ref = wav.mean(0)
    mean, std = ref.mean(), ref.std() + 1e-8
    wav = (wav - mean) / std

    total = wav.shape[-1]
    chunk = int(CHUNK_SECONDS.get(device.split(":")[0], 10.0) * samplerate)
    fade = int(CROSSFADE_SECONDS * samplerate)
    out = torch.zeros(len(sources), 2, total)
    norm = torch.zeros(total)
    starts = list(range(0, max(total - fade, 1), chunk)) if total > chunk + fade else [0]
    steps = len(starts) * len(models)
    step = 0

    for i, model in enumerate(models):
        w = torch.tensor(weights[i], dtype=torch.float32)[:, None, None]
        if float(w.abs().sum()) == 0:
            step += len(starts)
            continue
        model.to(device)
        for c, start in enumerate(starts):
            job.check()
            end = total if c == len(starts) - 1 else min(total, start + chunk + fade)
            with torch.inference_mode():
                est = apply_model(
                    model, wav[None, :, start:end], device=device, shifts=shifts, split=True, overlap=overlap, progress=False
                )[0].cpu()
            n = end - start
            win = torch.ones(n)
            k = min(fade, n)
            if c > 0:
                win[:k] = torch.linspace(0, 1, k)
            if c < len(starts) - 1:
                win[n - k :] = torch.minimum(win[n - k :], torch.linspace(1, 0, k))
            out[:, :, start:end] += est * w * win
            if i == 0:
                norm[start:end] += win
            step += 1
            progress(step / steps, f"separando ({step}/{steps})")
        model.to("cpu")

    total_w = torch.tensor(weights, dtype=torch.float32).sum(0)[:, None, None]
    out = out / norm.clamp_min(1e-8) / total_w.clamp_min(1e-8)
    out = out * std + mean
    return {name: out[s] for s, name in enumerate(sources)}, samplerate


# ---------------------------------------------------------------- Mel-Band RoFormer


def _separate_mel_roformer(manifest: ModelManifest, model_dir: Path, wav, sr: int, device: str, params, job: Job, progress: Progress):
    import torch
    import torch.nn.functional as F

    from ..vendor.msst.mel_band_roformer import MelBandRoformer

    # TOML no tiene tuplas y el modelo las exige (beartype).
    cfg = {k: tuple(v) if isinstance(v, list) else v for k, v in manifest.extra["config"].items()}
    inf = manifest.extra.get("inference", {})
    samplerate = int(cfg.get("sample_rate", 44100))
    chunk = int(inf.get("chunk_size", 352800))
    num_overlap = int(inf.get("num_overlap", 2))
    batch_size = int(inf.get("batch_size", 2)) if device.startswith("cuda") else 1

    model = MelBandRoformer(**cfg)
    state = torch.load(model_dir / manifest.files[0].name, map_location="cpu", weights_only=True)
    if isinstance(state, dict) and "state_dict" in state:
        state = state["state_dict"]
    model.load_state_dict(state)
    model.eval().to(device)
    job.check()

    mix = _resample(wav, sr, samplerate)
    total = mix.shape[-1]
    step = chunk // num_overlap
    border = chunk - step
    padded = F.pad(mix[None], (border, border), mode="reflect" if total > 2 * border else "constant")[0]
    length = padded.shape[-1]

    fade = chunk // 10
    window = torch.ones(chunk)
    window[:fade] = torch.linspace(0, 1, fade)
    window[-fade:] = torch.linspace(1, 0, fade)

    result = torch.zeros(2, length)
    counter = torch.zeros(length)
    starts = list(range(0, length - border, step))
    use_amp = device.startswith("cuda")

    for b in range(0, len(starts), batch_size):
        job.check()
        group = starts[b : b + batch_size]
        segs = []
        for s in group:
            seg = padded[:, s : s + chunk]
            if seg.shape[-1] < chunk:
                seg = F.pad(seg, (0, chunk - seg.shape[-1]))
            segs.append(seg)
        batch = torch.stack(segs).to(device)
        with torch.inference_mode(), torch.autocast(device_type="cuda", dtype=torch.float16, enabled=use_amp):
            est = model(batch).float().cpu()
        if est.shape[-1] < chunk:
            est = F.pad(est, (0, chunk - est.shape[-1]))
        for j, s in enumerate(group):
            n = min(chunk, length - s)
            win = window[:n].clone()
            if s == 0:
                win[:fade] = 1.0
            if s + chunk >= length:
                win[-min(fade, n):] = 1.0
            result[:, s : s + n] += est[j, :, :n] * win
            counter[s : s + n] += win
        progress(min(1.0, (b + len(group)) / len(starts)), f"separando ({b + len(group)}/{len(starts)})")

    model.to("cpu")
    vocals = (result / counter.clamp_min(1e-8))[:, border : border + total]
    return {"vocals": vocals, "instrumental": mix - vocals}, samplerate


BACKENDS = {"demucs": _separate_demucs, "mel_roformer": _separate_mel_roformer}


@register
class SeparateTask(Task):
    id = "separate"
    name = "Separación de stems"
    description = "Divide una canción en voz, batería, bajo, guitarra, piano y otros."
    model_task = "separation"

    def available(self) -> tuple[bool, str]:
        needed = sorted({m for deps in FAMILY_DEPS.values() for m in deps})
        missing = [m for m in needed if importlib.util.find_spec(m) is None]
        if missing:
            return False, "faltan dependencias de Python: " + ", ".join(missing)
        return True, ""

    def run(self, params: dict[str, Any], job: Job) -> dict[str, Any]:
        import numpy as np
        import soundfile as sf
        import torch

        model_id = params["model_id"]
        manifest: ModelManifest = self.models.get(model_id)
        if manifest.task != "separation":
            raise ValueError(f"{model_id} no es un modelo de separación")
        backend = BACKENDS.get(manifest.family)
        if backend is None:
            raise ValueError(f"familia de modelo no soportada: {manifest.family}")
        if not self.models.is_installed(model_id):
            raise ValueError(f"el modelo {model_id} no está instalado")
        out_dir = Path(params["output_dir"])
        device = _resolve_device(params.get("device"))
        started = time.monotonic()

        job.report(0.0, "leyendo audio", force=True)
        audio, sr = sf.read(str(params["input"]), dtype="float32", always_2d=True)
        wav = torch.from_numpy(np.ascontiguousarray(audio.T))
        if wav.shape[0] == 1:
            wav = wav.repeat(2, 1)
        elif wav.shape[0] > 2:
            wav = wav[:2]

        job.report(0.02, "cargando modelo", force=True)
        stems, samplerate = backend(
            manifest,
            self.models.model_dir(model_id),
            wav,
            sr,
            device,
            params,
            job,
            lambda p, msg: job.report(0.05 + 0.9 * p, msg),
        )
        if device.startswith("cuda"):
            torch.cuda.empty_cache()

        job.report(0.96, "guardando stems", force=True)
        out_dir.mkdir(parents=True, exist_ok=True)
        written = []
        for name, data in stems.items():
            job.check()
            path = out_dir / f"{name}.flac"
            # FLAC de 24 bits: sin pérdida a efectos prácticos y ~3 veces más pequeño que WAV flotante.
            sf.write(str(path), np.clip(data.T.numpy(), -1.0, 1.0), samplerate, subtype="PCM_24", format="FLAC")
            written.append({"name": name, "label": STEM_LABELS.get(name, name.capitalize()), "file": str(path)})

        any_stem = next(iter(stems.values()))
        return {
            "model_id": model_id,
            "sample_rate": samplerate,
            "device": device,
            "seconds": round(time.monotonic() - started, 2),
            "duration": round(any_stem.shape[-1] / samplerate, 2),
            "stems": written,
        }
