"""Masterización por referencia con Matchering 2.0 (GPL-3.0).

Iguala la respuesta en frecuencia, el nivel RMS y la anchura estéreo de la
mezcla con los de una canción de referencia, y termina con su limitador.
"""

from __future__ import annotations

import importlib.util
import time
from pathlib import Path
from typing import Any

from ..jobs import Job
from . import Task, register

DEPS = ("matchering", "numpy", "soundfile")

# Etapas que anuncia Matchering en su registro, para estimar el progreso.
STAGES = [
    ("loading", 0.05, "leyendo audio"),
    ("resampl", 0.10, "remuestreando"),
    ("mid", 0.20, "analizando la referencia"),
    ("rms", 0.35, "igualando el nivel"),
    ("fir", 0.50, "igualando la respuesta en frecuencia"),
    ("frequenc", 0.50, "igualando la respuesta en frecuencia"),
    ("correct", 0.70, "corrigiendo"),
    ("limit", 0.85, "limitando"),
    ("export", 0.95, "guardando"),
    ("sav", 0.95, "guardando"),
]


@register
class MasterReferenceTask(Task):
    id = "master_reference"
    name = "Master por referencia"
    description = "Iguala el sonido de la mezcla con el de una canción de referencia (Matchering)."

    def available(self) -> tuple[bool, str]:
        missing = [m for m in DEPS if importlib.util.find_spec(m) is None]
        if missing:
            return False, "faltan dependencias de Python: " + ", ".join(missing)
        return True, ""

    def run(self, params: dict[str, Any], job: Job) -> dict[str, Any]:
        # La primera importación tarda (numpy, scipy, resampy...): que se vea.
        job.report(0.01, "cargando Matchering", force=True)
        import matchering as mg

        target = Path(params["target"])
        reference = Path(params["reference"])
        output = Path(params["output"])
        if not reference.is_file():
            raise ValueError(f"no se encuentra la referencia: {reference}")
        output.parent.mkdir(parents=True, exist_ok=True)
        started = time.monotonic()
        progress = {"value": 0.0}
        warnings: list[str] = []

        def on_info(text: str) -> None:
            low = text.lower()
            for key, value, label in STAGES:
                if key in low and value > progress["value"]:
                    progress["value"] = value
                    job.report(value, label, force=True)
                    break

        mg.log(warning_handler=warnings.append, info_handler=on_info, debug_handler=lambda _t: None)
        job.report(0.02, "preparando", force=True)
        job.check()
        try:
            mg.process(
                target=str(target),
                reference=str(reference),
                results=[mg.pcm24(str(output))],
            )
        except Exception as e:  # noqa: BLE001 - Matchering lanza tipos propios
            raise ValueError(f"Matchering no pudo procesar el audio: {e}") from e
        job.check()
        job.report(1.0, "listo", force=True)
        return {
            "output": str(output),
            "reference": reference.name,
            "warnings": warnings,
            "seconds": round(time.monotonic() - started, 2),
        }
