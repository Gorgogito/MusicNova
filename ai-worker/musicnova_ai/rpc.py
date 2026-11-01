"""Protocolo JSON-RPC por líneas sobre stdin/stdout.

Petición:   {"id": 1, "method": "models.list", "params": {}}
Respuesta:  {"id": 1, "result": ...}  o  {"id": 1, "error": {"code": "...", "message": "..."}}
Evento:     {"event": "job.progress", "data": {...}}

stdout queda reservado al protocolo; cualquier `print` de bibliotecas de
terceros se redirige a stderr.
"""

from __future__ import annotations

import json
import sys
import threading
import traceback
from typing import Any, Callable, TextIO

Handler = Callable[[dict[str, Any]], Any]


class RpcError(Exception):
    def __init__(self, code: str, message: str):
        super().__init__(message)
        self.code = code
        self.message = message


class Server:
    def __init__(self, out: TextIO, inp: TextIO):
        self._out = out
        self._inp = inp
        self._lock = threading.Lock()
        self._methods: dict[str, Handler] = {}
        self._running = True

    def method(self, name: str) -> Callable[[Handler], Handler]:
        def deco(fn: Handler) -> Handler:
            self._methods[name] = fn
            return fn

        return deco

    def register(self, name: str, fn: Handler) -> None:
        self._methods[name] = fn

    def methods(self) -> list[str]:
        return sorted(self._methods)

    def _write(self, msg: dict[str, Any]) -> None:
        line = json.dumps(msg, ensure_ascii=False, separators=(",", ":"))
        with self._lock:
            self._out.write(line + "\n")
            self._out.flush()

    def emit(self, event: str, data: Any) -> None:
        self._write({"event": event, "data": data})

    def stop(self) -> None:
        self._running = False

    def handle_line(self, line: str) -> None:
        line = line.strip()
        if not line:
            return
        req_id = None
        try:
            req = json.loads(line)
            req_id = req.get("id")
            name = req.get("method")
            fn = self._methods.get(name)
            if fn is None:
                raise RpcError("method_not_found", f"método desconocido: {name}")
            params = req.get("params") or {}
            if not isinstance(params, dict):
                raise RpcError("invalid_params", "params debe ser un objeto")
            result = fn(params)
            self._write({"id": req_id, "result": result})
        except RpcError as e:
            self._write({"id": req_id, "error": {"code": e.code, "message": e.message}})
        except json.JSONDecodeError as e:
            self._write({"id": None, "error": {"code": "parse_error", "message": str(e)}})
        except Exception as e:  # noqa: BLE001 - el worker nunca debe caerse por una petición
            traceback.print_exc(file=sys.stderr)
            self._write({"id": req_id, "error": {"code": "internal", "message": f"{type(e).__name__}: {e}"}})

    def serve_forever(self) -> None:
        for line in self._inp:
            self.handle_line(line)
            if not self._running:
                break
