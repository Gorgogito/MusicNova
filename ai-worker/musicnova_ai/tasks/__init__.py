"""Registro de tareas de IA.

Cada tarea (separación, acordes, letras...) es una subclase de `Task` que se
registra con `@register`. La interfaz descubre las tareas disponibles con
`tasks.list` y las ejecuta con `tasks.run`, que crea un trabajo cancelable.
"""

from __future__ import annotations

from typing import Any, ClassVar

from ..jobs import Job
from ..models import ModelManager


class Task:
    id: ClassVar[str]
    name: ClassVar[str]
    description: ClassVar[str]
    #: Tipo de modelo que necesita (coincide con `task` en los manifiestos).
    model_task: ClassVar[str | None] = None
    #: Módulos de Python que usa, incluidas las mejoras opcionales (p. ej. Beat
    #: This! en el análisis). En la app instalada se instalan la primera vez.
    requires: ClassVar[tuple[str, ...]] = ()

    def __init__(self, models: ModelManager):
        self.models = models

    def available(self) -> tuple[bool, str]:
        """Indica si la tarea puede ejecutarse (dependencias instaladas)."""
        return True, ""

    def run(self, params: dict[str, Any], job: Job) -> Any:
        raise NotImplementedError

    def describe(self) -> dict[str, Any]:
        ok, reason = self.available()
        models = [m for m in self.models.list() if m["task"] == self.model_task] if self.model_task else []
        return {
            "id": self.id,
            "name": self.name,
            "description": self.description,
            "available": ok,
            "unavailable_reason": reason,
            "models": [m["id"] for m in models],
        }


REGISTRY: dict[str, type[Task]] = {}


def register(cls: type[Task]) -> type[Task]:
    REGISTRY[cls.id] = cls
    return cls


# Las tareas se registran al importar sus módulos.
from . import analysis, denoise, drums, generation, lyrics, mastering, separation, transcription  # noqa: E402,F401

# Lo que instala cada tarea en la app instalada (todo lo que puede usar).
_REQUIRES = {
    "analyze": analysis.CORE_DEPS + analysis.BEAT_THIS_DEPS,
    "denoise_voice": denoise.DENOISE_DEPS,
    "transcribe_drums": drums.DRUMS_DEPS,
    "transcribe": lyrics.DEPS,
    "master_reference": mastering.DEPS,
    "separate": tuple(sorted({m for deps in separation.FAMILY_DEPS.values() for m in deps})),
    "transcribe_notes": transcription.NOTES_DEPS,
    "track_pitch": transcription.PITCH_DEPS + ("torch", "torchcrepe"),
    "detect_instruments": transcription.INSTRUMENT_DEPS,
}
for _tid, _mods in _REQUIRES.items():
    REGISTRY[_tid].requires = tuple(dict.fromkeys(_mods))


def requires_for_model_task(model_task: str) -> tuple[str, ...]:
    """Módulos que necesitan las tareas que usan modelos de tipo `model_task`."""
    mods: dict[str, None] = {}
    for cls in REGISTRY.values():
        if cls.model_task == model_task or (model_task == "chords" and cls.id == "analyze"):
            mods.update(dict.fromkeys(cls.requires))
    return tuple(mods)
