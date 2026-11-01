"""Punto de entrada: `python -m musicnova_ai`."""

from __future__ import annotations

import io
import sys
from typing import Any

from . import PROTOCOL_VERSION, __version__, system
from .jobs import JobManager
from .models import ModelManager, data_dir
from .rpc import RpcError, Server
from .tasks import REGISTRY, Task


def build_server(out: io.TextIOBase, inp: io.TextIOBase) -> tuple[Server, JobManager]:
    server = Server(out, inp)
    jobs = JobManager(server.emit)
    models = ModelManager()
    tasks: dict[str, Task] = {tid: cls(models) for tid, cls in REGISTRY.items()}

    def require(params: dict[str, Any], key: str) -> Any:
        if key not in params:
            raise RpcError("invalid_params", f"falta el parámetro '{key}'")
        return params[key]

    def model_or_error(params: dict[str, Any]) -> str:
        mid = require(params, "model_id")
        if mid not in models.manifests:
            raise RpcError("not_found", f"modelo desconocido: {mid}")
        return mid

    server.register("ping", lambda p: {"pong": True, "version": __version__, "protocol": PROTOCOL_VERSION})
    server.register("system.info", lambda p: {**system.info(), "data_dir": str(data_dir())})
    server.register("rpc.methods", lambda p: server.methods())

    server.register("models.list", lambda p: {"models": models.list(), "errors": models.errors})

    def models_reload(p: dict[str, Any]) -> dict[str, Any]:
        models.reload()
        return {"models": models.list(), "errors": models.errors}

    server.register("models.reload", models_reload)

    def models_install(p: dict[str, Any]) -> dict[str, str]:
        mid = model_or_error(p)
        return {"job_id": jobs.submit("models.install", lambda job: models.install(mid, job))}

    server.register("models.install", models_install)

    def models_remove(p: dict[str, Any]) -> dict[str, bool]:
        models.remove(model_or_error(p))
        return {"removed": True}

    server.register("models.remove", models_remove)

    server.register("tasks.list", lambda p: [t.describe() for t in tasks.values()])

    def tasks_run(p: dict[str, Any]) -> dict[str, str]:
        tid = require(p, "task")
        task = tasks.get(tid)
        if task is None:
            raise RpcError("not_found", f"tarea desconocida: {tid}")
        ok, reason = task.available()
        if not ok:
            raise RpcError("unavailable", reason)
        params = p.get("params") or {}
        return {"job_id": jobs.submit(tid, lambda job: task.run(params, job))}

    server.register("tasks.run", tasks_run)
    server.register("jobs.list", lambda p: jobs.list())
    server.register("jobs.cancel", lambda p: {"cancelled": jobs.cancel(require(p, "job_id"))})

    def shutdown(p: dict[str, Any]) -> dict[str, bool]:
        jobs.shutdown()
        server.stop()
        return {"ok": True}

    server.register("shutdown", shutdown)
    return server, jobs


def main() -> None:
    proto_out = io.TextIOWrapper(sys.stdout.buffer, encoding="utf-8", newline="\n", line_buffering=True)
    proto_in = io.TextIOWrapper(sys.stdin.buffer, encoding="utf-8")
    # Las bibliotecas que impriman en stdout no deben corromper el protocolo.
    sys.stdout = sys.stderr
    server, jobs = build_server(proto_out, proto_in)
    server.emit("ready", {"version": __version__, "protocol": PROTOCOL_VERSION})
    try:
        server.serve_forever()
    finally:
        jobs.shutdown()


if __name__ == "__main__":
    main()
