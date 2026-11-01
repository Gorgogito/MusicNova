"""Información del sistema: CPU, RAM, GPU y backends de IA disponibles."""

from __future__ import annotations

import ctypes
import importlib.util
import os
import platform
import shutil
import subprocess
import sys
from typing import Any


def _total_ram_bytes() -> int | None:
    if sys.platform == "win32":

        class MemoryStatusEx(ctypes.Structure):
            _fields_ = [
                ("dwLength", ctypes.c_ulong),
                ("dwMemoryLoad", ctypes.c_ulong),
                ("ullTotalPhys", ctypes.c_ulonglong),
                ("ullAvailPhys", ctypes.c_ulonglong),
                ("ullTotalPageFile", ctypes.c_ulonglong),
                ("ullAvailPageFile", ctypes.c_ulonglong),
                ("ullTotalVirtual", ctypes.c_ulonglong),
                ("ullAvailVirtual", ctypes.c_ulonglong),
                ("sullAvailExtendedVirtual", ctypes.c_ulonglong),
            ]

        stat = MemoryStatusEx()
        stat.dwLength = ctypes.sizeof(MemoryStatusEx)
        if ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(stat)):
            return int(stat.ullTotalPhys)
        return None
    try:
        return os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES")
    except (ValueError, OSError, AttributeError):
        return None


def _nvidia_gpus() -> list[dict[str, Any]]:
    """GPUs NVIDIA según nvidia-smi (funciona aunque PyTorch no esté instalado)."""
    exe = shutil.which("nvidia-smi")
    if not exe:
        return []
    try:
        out = subprocess.run(
            [exe, "--query-gpu=name,memory.total,driver_version", "--format=csv,noheader,nounits"],
            capture_output=True,
            text=True,
            timeout=10,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    gpus = []
    for line in out.strip().splitlines():
        parts = [p.strip() for p in line.split(",")]
        if len(parts) >= 3:
            try:
                vram = int(float(parts[1])) * 1024 * 1024
            except ValueError:
                vram = None
            gpus.append({"name": parts[0], "vram_bytes": vram, "driver": parts[2], "vendor": "nvidia"})
    return gpus


def _torch_info() -> dict[str, Any] | None:
    if importlib.util.find_spec("torch") is None:
        return None
    import torch  # noqa: PLC0415 - importación costosa, solo si está instalado

    info: dict[str, Any] = {
        "version": torch.__version__,
        "cuda": torch.cuda.is_available(),
        "cuda_version": torch.version.cuda,
        "devices": [],
    }
    if info["cuda"]:
        for i in range(torch.cuda.device_count()):
            p = torch.cuda.get_device_properties(i)
            info["devices"].append({"index": i, "name": p.name, "vram_bytes": p.total_memory})
    mps = getattr(torch.backends, "mps", None)
    info["mps"] = bool(mps and mps.is_available())
    return info


def _onnx_info() -> dict[str, Any] | None:
    if importlib.util.find_spec("onnxruntime") is None:
        return None
    import onnxruntime as ort  # noqa: PLC0415

    return {"version": ort.__version__, "providers": ort.get_available_providers()}


def info() -> dict[str, Any]:
    return {
        "python": platform.python_version(),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "cpu_count": os.cpu_count(),
        "ram_bytes": _total_ram_bytes(),
        "gpus": _nvidia_gpus(),
        "torch": _torch_info(),
        "onnxruntime": _onnx_info(),
    }


def best_device() -> str:
    """Dispositivo preferido para PyTorch: cuda > mps > cpu."""
    t = _torch_info()
    if t and t["cuda"]:
        return "cuda"
    if t and t.get("mps"):
        return "mps"
    return "cpu"
