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
from . import analysis, drums, lyrics, mastering, separation, transcription  # noqa: E402,F401
