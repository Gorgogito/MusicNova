"""Gestor de modelos: manifiestos TOML, descarga verificada e instalación.

Cada modelo se describe con un manifiesto (`manifests/*.toml` o los que el
usuario añada en `<datos>/manifests`). Los pesos nunca se incluyen en el
instalador: se descargan bajo demanda y se verifican con SHA-256.
"""

from __future__ import annotations

import fnmatch
import hashlib
import os
import shutil
import sys
import tarfile
import tomllib
import urllib.request
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .jobs import Job

USER_AGENT = "MusicNova/0.1 (+https://github.com/musicnova)"
CHUNK = 1 << 20


def data_dir() -> Path:
    env = os.environ.get("MUSICNOVA_DATA")
    if env:
        return Path(env)
    if sys.platform == "win32":
        base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
        return Path(base) / "MusicNova"
    return Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share")) / "musicnova"


@dataclass
class ModelFile:
    name: str
    url: str
    sha256: str
    size: int | None = None


@dataclass
class ModelManifest:
    id: str
    name: str
    task: str
    family: str
    description: str
    license: str
    license_url: str
    commercial_use: bool
    files: list[ModelFile]
    extra: dict[str, Any] = field(default_factory=dict)
    source: str = ""

    @classmethod
    def from_toml(cls, path: Path) -> "ModelManifest":
        with path.open("rb") as f:
            raw = tomllib.load(f)
        known = {"id", "name", "task", "family", "description", "license", "license_url", "commercial_use", "files"}
        missing = [k for k in known if k not in raw]
        if missing:
            raise ValueError(f"{path.name}: faltan campos {missing}")
        files = [ModelFile(**f) for f in raw["files"]]
        for f in files:
            if len(f.sha256) < 8 or any(c not in "0123456789abcdef" for c in f.sha256.lower()):
                raise ValueError(f"{path.name}: sha256 inválido para {f.name}")
            # Se admiten subcarpetas («vae/config.json»), pero nada que salga de
            # la carpeta del modelo ni nombres ocultos.
            parts = f.name.split("/")
            if "\\" in f.name or ":" in f.name or any(not p or p.startswith(".") for p in parts):
                raise ValueError(f"{path.name}: nombre de archivo inválido {f.name}")
        return cls(
            id=raw["id"],
            name=raw["name"],
            task=raw["task"],
            family=raw["family"],
            description=raw["description"],
            license=raw["license"],
            license_url=raw["license_url"],
            commercial_use=bool(raw["commercial_use"]),
            files=files,
            extra={k: v for k, v in raw.items() if k not in known},
            source=str(path),
        )


class ModelManager:
    def __init__(self, models_dir: Path | None = None, manifest_dirs: list[Path] | None = None):
        root = data_dir()
        self.models_dir = models_dir or root / "models"
        self.manifest_dirs = manifest_dirs or [Path(__file__).parent / "manifests", root / "manifests"]
        self.models_dir.mkdir(parents=True, exist_ok=True)
        self.manifests: dict[str, ModelManifest] = {}
        self.errors: list[str] = []
        self.reload()

    def reload(self) -> None:
        self.manifests.clear()
        self.errors.clear()
        for d in self.manifest_dirs:
            if not d.is_dir():
                continue
            for p in sorted(d.glob("*.toml")):
                try:
                    m = ModelManifest.from_toml(p)
                    self.manifests[m.id] = m
                except (ValueError, TypeError, tomllib.TOMLDecodeError) as e:
                    self.errors.append(f"{p}: {e}")

    def get(self, model_id: str) -> ModelManifest:
        m = self.manifests.get(model_id)
        if m is None:
            raise KeyError(f"modelo desconocido: {model_id}")
        return m

    def model_dir(self, model_id: str) -> Path:
        return self.models_dir / model_id

    def is_installed(self, model_id: str) -> bool:
        m = self.get(model_id)
        d = self.model_dir(model_id)
        # Las herramientas en zip quedan instaladas cuando existen los archivos extraídos.
        extract = m.extra.get("extract")
        if extract:
            return all((d / name).is_file() for name in extract)
        return all((d / f.name).is_file() for f in m.files)

    def describe(self, m: ModelManifest) -> dict[str, Any]:
        sizes = [f.size for f in m.files]
        return {
            "id": m.id,
            "name": m.name,
            "task": m.task,
            "family": m.family,
            "description": m.description,
            "license": m.license,
            "license_url": m.license_url,
            "commercial_use": m.commercial_use,
            "size_bytes": sum(s for s in sizes if s) if all(sizes) else None,
            "installed": self.is_installed(m.id),
            "path": str(self.model_dir(m.id)),
            **m.extra,
        }

    def list(self) -> list[dict[str, Any]]:
        return [self.describe(m) for m in self.manifests.values()]

    def install(self, model_id: str, job: Job) -> dict[str, Any]:
        m = self.get(model_id)
        target = self.model_dir(model_id)
        target.mkdir(parents=True, exist_ok=True)
        total = sum(f.size or 0 for f in m.files) or None
        done_bytes = 0
        for f in m.files:
            dest = target / f.name
            dest.parent.mkdir(parents=True, exist_ok=True)
            if dest.is_file() and _sha256_matches(_sha256_file(dest), f.sha256):
                done_bytes += dest.stat().st_size
                continue
            done_bytes = self._download(f, dest, job, done_bytes, total)
        extract = m.extra.get("extract")
        if extract:
            job.report(1.0, "extrayendo", force=True)
            for f in m.files:
                _extract_members(target / f.name, target, extract)
                (target / f.name).unlink()
        job.report(1.0, "instalado", force=True)
        return self.describe(m)

    def _download(self, f: ModelFile, dest: Path, job: Job, offset: int, total: int | None) -> int:
        part = dest.with_name(dest.name + ".part")
        req = urllib.request.Request(f.url, headers={"User-Agent": USER_AGENT})
        h = hashlib.sha256()
        got = 0
        try:
            with urllib.request.urlopen(req, timeout=60) as resp, part.open("wb") as out:
                size = f.size or int(resp.headers.get("Content-Length") or 0) or None
                denom = total or size
                while True:
                    job.check()
                    chunk = resp.read(CHUNK)
                    if not chunk:
                        break
                    out.write(chunk)
                    h.update(chunk)
                    got += len(chunk)
                    if denom:
                        job.report((offset + got) / denom, f"descargando {f.name}")
            digest = h.hexdigest()
            if not _sha256_matches(digest, f.sha256):
                raise ValueError(f"la suma SHA-256 de {f.name} no coincide (esperado {f.sha256}, obtenido {digest})")
            if f.size and got != f.size:
                raise ValueError(f"tamaño inesperado de {f.name}: {got} bytes (esperado {f.size})")
            os.replace(part, dest)
        finally:
            if part.exists():
                part.unlink()
        return offset + got

    def remove(self, model_id: str) -> None:
        self.get(model_id)
        d = self.model_dir(model_id)
        if d.exists():
            shutil.rmtree(d)


def _extract_members(archive: Path, target: Path, mapping: dict[str, str]) -> None:
    """Extrae de un zip o un tar.gz los miembros indicados: {nombre_destino:
    patrón_glob}. Solo se escriben los nombres de destino (sin rutas del archivo)."""
    for dest_name in mapping:
        if "/" in dest_name or "\\" in dest_name or dest_name.startswith("."):
            raise ValueError(f"nombre de destino inválido: {dest_name}")
    if archive.name.endswith((".tar.gz", ".tgz")):
        with tarfile.open(archive, "r:gz") as t:
            members = [m for m in t.getmembers() if m.isfile()]
            for dest_name, pattern in mapping.items():
                match = next((m for m in members if fnmatch.fnmatch(m.name, pattern)), None)
                if match is None:
                    raise ValueError(f"{archive.name} no contiene {pattern}")
                src = t.extractfile(match)
                assert src is not None
                with src, (target / dest_name).open("wb") as out:
                    shutil.copyfileobj(src, out)
        return
    with zipfile.ZipFile(archive) as z:
        names = z.namelist()
        for dest_name, pattern in mapping.items():
            match = next((n for n in names if fnmatch.fnmatch(n, pattern)), None)
            if match is None:
                raise ValueError(f"{archive.name} no contiene {pattern}")
            with z.open(match) as src, (target / dest_name).open("wb") as out:
                shutil.copyfileobj(src, out)


def _sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        while chunk := f.read(CHUNK):
            h.update(chunk)
    return h.hexdigest()


def _sha256_matches(digest: str, expected: str) -> bool:
    # Se admite un prefijo (mínimo 8 hex), como hacen los checkpoints de Demucs.
    return digest.lower().startswith(expected.lower())
