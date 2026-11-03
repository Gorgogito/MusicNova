"""Genera una pista con ACE-Step 1.5 (tarea «lego»). Se ejecuta con el Python
del entorno de ACE-Step, no con el del worker:

    python acestep_runner.py parametros.json

Parámetros (JSON): checkpoints (carpeta con acestep-v15-base, vae y
Qwen3-Embedding-0.6B), src_audio (contexto), output (wav), track (nombre de
pista de ACE-Step), caption, bpm, keyscale, timesignature, steps, guidance,
seed, device, offload, start, end.

Salida por stdout, una línea por evento:
    MN-PROGRESS <0–1> <mensaje>
    MN-RESULT <json>
"""

from __future__ import annotations

import json
import os
import sys
import time

TRACKS = ["woodwinds", "brass", "fx", "synth", "strings", "percussion", "keyboard", "guitar", "bass", "drums", "backing_vocals", "vocals"]


def emit(kind: str, payload) -> None:
    sys.stdout.write(f"MN-{kind} {payload}\n")
    sys.stdout.flush()


def emit_progress(value: float, message: str) -> None:
    """Avance (0–1) y mensaje visible (la interfaz lo traduce)."""
    emit("PROGRESS", f"{value:.3f} {message}")


def main() -> int:
    params = json.load(open(sys.argv[1], encoding="utf-8"))
    track = params["track"]
    if track not in TRACKS:
        raise ValueError(f"pista desconocida para ACE-Step: {track}")
    os.environ["ACESTEP_CHECKPOINTS_DIR"] = params["checkpoints"]
    # Sin telemetría ni descargas: los pesos ya están en `checkpoints`.
    os.environ.setdefault("HF_HUB_OFFLINE", "1")
    os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
    started = time.monotonic()
    emit_progress(0.01, "cargando ACE-Step")

    from acestep.handler import AceStepHandler
    from acestep.inference import GenerationConfig, GenerationParams, generate_music
    from acestep.llm_inference import LLMHandler

    dit = AceStepHandler()
    status, ok = dit.initialize_service(
        project_root="",
        config_path="acestep-v15-base",
        device=params.get("device", "auto"),
        offload_to_cpu=bool(params.get("offload", True)),
        offload_dit_to_cpu=bool(params.get("offload", True)),
        use_flash_attention=False,
        compile_model=False,
    )
    if not ok:
        raise RuntimeError(f"no se pudo cargar ACE-Step: {status}")
    emit_progress(0.08, "modelo cargado")

    def progress(value, desc=None, **_):
        try:
            v = float(value)
        except (TypeError, ValueError):
            return
        emit_progress(0.08 + 0.9 * max(0.0, min(1.0, v)), desc or "generando")

    gp = GenerationParams(
        task_type="lego",
        src_audio=params["src_audio"],
        instruction=f"Generate the {track.upper()} track based on the audio context:",
        caption=params.get("caption", ""),
        bpm=params.get("bpm"),
        keyscale=params.get("keyscale", ""),
        timesignature=params.get("timesignature", ""),
        inference_steps=int(params.get("steps", 32)),
        guidance_scale=float(params.get("guidance", 7.0)),
        seed=int(params.get("seed", -1)),
        repainting_start=float(params.get("start", 0.0)),
        repainting_end=float(params.get("end", -1)),
        thinking=False,
    )
    seed = int(params.get("seed", -1))
    cfg = GenerationConfig(batch_size=1, use_random_seed=seed < 0, seeds=None if seed < 0 else [seed], audio_format="wav32")
    out_dir = os.path.dirname(os.path.abspath(params["output"]))
    result = generate_music(dit, LLMHandler(), gp, cfg, save_dir=out_dir, progress=progress)
    if not result.success or not result.audios:
        raise RuntimeError(result.error or result.status_message or "ACE-Step no generó audio")
    audio = result.audios[0]
    path = audio.get("path") or audio.get("audio_path")
    if not path or not os.path.exists(path):
        raise RuntimeError(f"ACE-Step no dejó el archivo de audio ({sorted(audio)})")
    os.replace(path, params["output"])
    emit(
        "RESULT",
        json.dumps(
            {"output": params["output"], "seconds": round(time.monotonic() - started, 1), "seed": audio.get("params", {}).get("seed")},
            ensure_ascii=False,
        ),
    )
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as e:  # el worker lee el error de esta línea
        emit("ERROR", json.dumps(f"{type(e).__name__}: {e}", ensure_ascii=False))
        raise
