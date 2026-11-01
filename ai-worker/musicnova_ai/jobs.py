"""Trabajos de larga duración (descargas, separación, análisis...).

Un método RPC que arranca un trabajo devuelve `{"job_id": ...}` al instante y
el progreso llega como eventos `job.progress`, `job.done`, `job.failed` y
`job.cancelled`.
"""

from __future__ import annotations

import itertools
import sys
import threading
import time
import traceback
from concurrent.futures import ThreadPoolExecutor
from typing import Any, Callable


class JobCancelled(Exception):
    pass


class Job:
    def __init__(self, job_id: str, kind: str, emit: Callable[[str, Any], None]):
        self.id = job_id
        self.kind = kind
        self._emit = emit
        self._cancel = threading.Event()
        self._last_report = 0.0
        self.progress = 0.0
        self.status = "queued"

    @property
    def cancelled(self) -> bool:
        return self._cancel.is_set()

    def cancel(self) -> None:
        self._cancel.set()

    def check(self) -> None:
        """Lanza `JobCancelled` si se pidió cancelar. Llamar con frecuencia."""
        if self._cancel.is_set():
            raise JobCancelled()

    def report(self, progress: float, message: str = "", force: bool = False) -> None:
        self.check()
        self.progress = max(0.0, min(1.0, progress))
        now = time.monotonic()
        # Limita los eventos a ~10 por segundo.
        if force or now - self._last_report >= 0.1 or self.progress >= 1.0:
            self._last_report = now
            self._emit("job.progress", {"job_id": self.id, "kind": self.kind, "progress": self.progress, "message": message})

    def info(self) -> dict[str, Any]:
        return {"job_id": self.id, "kind": self.kind, "status": self.status, "progress": self.progress}


class JobManager:
    def __init__(self, emit: Callable[[str, Any], None], max_workers: int = 2):
        self._emit = emit
        self._pool = ThreadPoolExecutor(max_workers=max_workers, thread_name_prefix="mn-job")
        self._jobs: dict[str, Job] = {}
        self._ids = itertools.count(1)
        self._lock = threading.Lock()

    def submit(self, kind: str, fn: Callable[[Job], Any]) -> str:
        job = Job(f"job-{next(self._ids)}", kind, self._emit)
        with self._lock:
            self._jobs[job.id] = job
        self._pool.submit(self._run, job, fn)
        return job.id

    def _run(self, job: Job, fn: Callable[[Job], Any]) -> None:
        job.status = "running"
        try:
            job.check()
            result = fn(job)
            job.status = "done"
            job.progress = 1.0
            self._emit("job.done", {"job_id": job.id, "kind": job.kind, "result": result})
        except JobCancelled:
            job.status = "cancelled"
            self._emit("job.cancelled", {"job_id": job.id, "kind": job.kind})
        except Exception as e:  # noqa: BLE001
            traceback.print_exc(file=sys.stderr)
            job.status = "failed"
            self._emit("job.failed", {"job_id": job.id, "kind": job.kind, "error": f"{type(e).__name__}: {e}"})

    def cancel(self, job_id: str) -> bool:
        job = self._jobs.get(job_id)
        if job is None:
            return False
        job.cancel()
        return True

    def list(self) -> list[dict[str, Any]]:
        with self._lock:
            return [j.info() for j in self._jobs.values()]

    def shutdown(self) -> None:
        for j in self._jobs.values():
            j.cancel()
        self._pool.shutdown(wait=False, cancel_futures=True)
