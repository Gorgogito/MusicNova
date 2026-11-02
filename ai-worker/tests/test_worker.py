import hashlib
import http.server
import io
import json
import subprocess
import sys
import threading
import time
from pathlib import Path

import pytest

from musicnova_ai.__main__ import build_server
from musicnova_ai.jobs import JobManager
from musicnova_ai.models import ModelManager

ROOT = Path(__file__).resolve().parents[1]


def call(server, out, method, params=None, req_id=1):
    server.handle_line(json.dumps({"id": req_id, "method": method, "params": params or {}}))
    lines = [json.loads(line) for line in out.getvalue().splitlines()]
    return next(m for m in reversed(lines) if m.get("id") == req_id)


def test_rpc_basics(monkeypatch, tmp_path):
    monkeypatch.setenv("MUSICNOVA_DATA", str(tmp_path))
    out = io.StringIO()
    server, jobs = build_server(out, io.StringIO())
    assert call(server, out, "ping")["result"]["pong"] is True
    assert call(server, out, "nope", req_id=2)["error"]["code"] == "method_not_found"
    models = call(server, out, "models.list", req_id=3)["result"]["models"]
    assert {"htdemucs", "htdemucs_6s"} <= {m["id"] for m in models}
    assert all(m["license"] for m in models)
    info = call(server, out, "system.info", req_id=4)["result"]
    assert info["cpu_count"] >= 1
    jobs.shutdown()


@pytest.fixture
def file_server(tmp_path):
    payload = b"MusicNova" * 200_000
    (tmp_path / "srv").mkdir()
    (tmp_path / "srv" / "w.bin").write_bytes(payload)

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *a, **kw):
            super().__init__(*a, directory=str(tmp_path / "srv"), **kw)

        def log_message(self, *a):
            pass

    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{httpd.server_address[1]}/w.bin", payload
    httpd.shutdown()


def write_manifest(d: Path, url: str, sha: str, size: int) -> None:
    d.mkdir(exist_ok=True)
    (d / "test.toml").write_text(
        "\n".join(
            [
                'id = "test"', 'name = "Test"', 'task = "separation"', 'family = "x"',
                'description = "d"', 'license = "MIT"', 'license_url = "u"', "commercial_use = true",
                "[[files]]", 'name = "w.bin"', f'url = "{url}"', f'sha256 = "{sha}"', f"size = {size}",
            ]
        ),
        encoding="utf-8",
    )


def wait_events(events, kinds, timeout=10):
    end = time.time() + timeout
    while time.time() < end:
        for e in events:
            if e[0] in kinds:
                return e
        time.sleep(0.02)
    raise AssertionError("tiempo de espera agotado")


def test_install_verifies_checksum(tmp_path, file_server):
    url, payload = file_server
    good = hashlib.sha256(payload).hexdigest()
    manifests = tmp_path / "man"
    events = []
    jm = JobManager(lambda ev, data: events.append((ev, data)))

    write_manifest(manifests, url, good, len(payload))
    mm = ModelManager(tmp_path / "models", [manifests])
    assert not mm.is_installed("test")
    jm.submit("install", lambda job: mm.install("test", job))
    ev = wait_events(events, {"job.done", "job.failed"})
    assert ev[0] == "job.done", ev
    assert mm.is_installed("test")
    assert any(e[0] == "job.progress" for e in events)

    mm.remove("test")
    events.clear()
    write_manifest(manifests, url, "0" * 64, len(payload))
    mm.reload()
    jm.submit("install", lambda job: mm.install("test", job))
    ev = wait_events(events, {"job.done", "job.failed"})
    assert ev[0] == "job.failed" and "SHA-256" in ev[1]["error"]
    assert not mm.is_installed("test")
    assert not list((tmp_path / "models" / "test").glob("*.part"))
    jm.shutdown()


def test_subprocess_protocol(tmp_path):
    env = {**__import__("os").environ, "MUSICNOVA_DATA": str(tmp_path), "PYTHONPATH": str(ROOT)}
    p = subprocess.Popen(
        [sys.executable, "-u", "-m", "musicnova_ai"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, cwd=ROOT,
    )
    ready = json.loads(p.stdout.readline())
    assert ready["event"] == "ready"
    p.stdin.write(b'{"id":7,"method":"ping"}\n{"id":8,"method":"shutdown"}\n')
    p.stdin.flush()
    assert json.loads(p.stdout.readline()) ["result"]["pong"] is True
    assert json.loads(p.stdout.readline())["id"] == 8
    assert p.wait(timeout=10) == 0


def test_install_extracts_zip_tools(tmp_path):
    import zipfile

    srv = tmp_path / "srv"
    srv.mkdir()
    archive = srv / "tool.zip"
    with zipfile.ZipFile(archive, "w") as z:
        z.writestr("tool-1.0/bin/tool.exe", b"binario")
        z.writestr("tool-1.0/LICENSE.txt", b"licencia")
        z.writestr("tool-1.0/bin/otro.exe", b"no se extrae")
    payload = archive.read_bytes()

    class Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *a, **kw):
            super().__init__(*a, directory=str(srv), **kw)

        def log_message(self, *a):
            pass

    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    try:
        man = tmp_path / "man"
        man.mkdir()
        (man / "tool.toml").write_text(
            "\n".join([
                'id = "tool"', 'name = "Tool"', 'task = "tool"', 'family = "x"', 'description = "d"',
                'license = "MIT"', 'license_url = "u"', "commercial_use = true",
                "[extract]", '"tool.exe" = "*/bin/tool.exe"', '"LICENSE.txt" = "*/LICENSE.txt"',
                "[[files]]", 'name = "tool.zip"', f'url = "http://127.0.0.1:{httpd.server_address[1]}/tool.zip"',
                f'sha256 = "{hashlib.sha256(payload).hexdigest()}"', f"size = {len(payload)}",
            ]),
            encoding="utf-8",
        )
        events = []
        jm = JobManager(lambda ev, data: events.append((ev, data)))
        mm = ModelManager(tmp_path / "models", [man])
        jm.submit("install", lambda job: mm.install("tool", job))
        ev = wait_events(events, {"job.done", "job.failed"})
        assert ev[0] == "job.done", ev
        d = mm.model_dir("tool")
        assert sorted(p.name for p in d.iterdir()) == ["LICENSE.txt", "tool.exe"]
        assert (d / "tool.exe").read_bytes() == b"binario"
        assert mm.is_installed("tool")
        jm.shutdown()
    finally:
        httpd.shutdown()


def test_job_importing_native_modules_does_not_wait_for_stdin(tmp_path):
    """Regresión: en Windows un trabajo que cargaba numpy en su hilo se quedaba
    parado mientras el hilo principal esperaba la siguiente línea de stdin."""
    import numpy as np
    import soundfile as sf

    wav = tmp_path / "golpes.wav"
    sr = 44_100
    y = np.zeros(sr * 2, dtype=np.float32)
    for k in range(4):
        y[int(k * 0.5 * sr) : int(k * 0.5 * sr) + 400] = np.hanning(400)
    sf.write(wav, y, sr)
    env = {**__import__("os").environ, "MUSICNOVA_DATA": str(tmp_path), "PYTHONPATH": str(ROOT)}
    p = subprocess.Popen(
        [sys.executable, "-u", "-m", "musicnova_ai"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, cwd=ROOT,
    )
    lines: list[dict] = []
    reader = threading.Thread(target=lambda: [lines.append(json.loads(l)) for l in p.stdout], daemon=True)
    reader.start()
    try:
        request = {"id": 1, "method": "tasks.run", "params": {"task": "transcribe_drums", "params": {"input": str(wav)}}}
        p.stdin.write((json.dumps(request) + "\n").encode())
        p.stdin.flush()
        # No se envía nada más: el trabajo tiene que acabar solo.
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline and not any(m.get("event") in ("job.done", "job.failed") for m in lines):
            time.sleep(0.2)
        events = [m.get("event") for m in lines]
        assert "job.done" in events, events
    finally:
        p.kill()
