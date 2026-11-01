"""Transcripción de letras con Whisper (faster-whisper / CTranslate2).

Se transcribe preferentemente el stem de voz: sin instrumentos, Whisper se
equivoca y alucina mucho menos. Los segmentos de Whisper se reparten en líneas
de karaoke por pausas, puntuación y longitud, y cada palabra conserva su marca
de tiempo para el resaltado palabra a palabra.
"""

from __future__ import annotations

import importlib.util
import json
import os
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from ..jobs import Job
from ..system import best_device
from . import Task, register

LYRICS_VERSION = 1
WHISPER_SR = 16_000
DEPS = ("faster_whisper", "ctranslate2", "numpy", "soundfile", "librosa")

# Límites de una línea de karaoke.
MAX_LINE_WORDS = 12
MAX_LINE_SECONDS = 7.0
SPLIT_GAP = 1.0  # una pausa así siempre parte la línea
MIN_SIDE_WORDS = 2

_dlls_registered = False


def register_cuda12_dlls() -> None:
    """En Windows, CTranslate2 busca las DLL de CUDA 12 (cuBLAS) que instala el
    paquete `nvidia-cublas-cu12`; PyTorch trae otra versión de CUDA."""
    global _dlls_registered
    if _dlls_registered or sys.platform != "win32":
        return
    _dlls_registered = True
    spec = importlib.util.find_spec("nvidia")
    if spec is None or not spec.submodule_search_locations:
        return
    for base in spec.submodule_search_locations:
        for bin_dir in Path(base).glob("*/bin"):
            os.add_dll_directory(str(bin_dir))
            os.environ["PATH"] = str(bin_dir) + os.pathsep + os.environ.get("PATH", "")


@dataclass
class Word:
    start: float
    end: float
    text: str
    prob: float = 1.0


def _gap_after(words: list[Word], i: int) -> float:
    return words[i + 1].start - words[i].end


_ALWAYS_UPPER = {"I", "I'm", "I'll", "I've", "I'd", "I’m", "I’ll", "I’ve", "I’d"}


def _starts_line(w: Word) -> bool:
    """Palabra con mayúscula inicial que no lo es siempre (como «I» en inglés)."""
    t = w.text.strip().lstrip("¿¡\"'(«")
    return t[:1].isupper() and t.rstrip(",.;:!?…\"')»") not in _ALWAYS_UPPER


def _split_score(words: list[Word], i: int) -> float:
    """Lo apropiado que es cortar entre `words[i]` y `words[i + 1]`."""
    score = _gap_after(words, i)
    if words[i].text.rstrip()[-1:] in ",.;:!?…":
        score += 0.4
    if _starts_line(words[i + 1]):
        score += 0.2
    return score


def split_lines(words: list[Word]) -> list[list[Word]]:
    """Parte una secuencia de palabras en líneas de karaoke."""
    if not words:
        return []
    # 1. Las pausas largas siempre separan líneas.
    groups: list[list[Word]] = [[words[0]]]
    for prev, w in zip(words, words[1:]):
        if w.start - prev.end >= SPLIT_GAP:
            groups.append([w])
        else:
            groups[-1].append(w)

    # 2. Whisper empieza cada verso con mayúscula pero a veces corta sus segmentos
    #    a destiempo ("me, girl On the tunnel"): se corta antes de la mayúscula y
    #    los fragmentos sueltos en minúscula vuelven a la línea anterior.
    pieces: list[list[Word]] = []
    for g in groups:
        pieces.append([g[0]])
        for w in g[1:]:
            if _starts_line(w):
                pieces.append([w])
            else:
                pieces[-1].append(w)
    # Una palabra suelta seguida de una continuación en minúscula ("She" | "said,
    # baby") es el inicio del verso siguiente con la marca de tiempo mal puesta:
    # se une a él y se recoloca justo antes de su primera palabra.
    forward: list[list[Word]] = []
    i = 0
    while i < len(pieces):
        p = pieces[i]
        nxt = pieces[i + 1] if i + 1 < len(pieces) else None
        if len(p) == 1 and nxt and not _starts_line(nxt[0]) and len(nxt) < MAX_LINE_WORDS:
            w = p[0]
            if nxt[0].start - w.end > 0.5:
                dur = min(w.end - w.start, 0.4)
                w.end = max(w.start, nxt[0].start - 0.05)
                w.start = max(w.start, w.end - dur)
            forward.append([w] + nxt)
            i += 2
        else:
            forward.append(p)
            i += 1
    merged: list[list[Word]] = []
    for p in forward:
        orphan = len(p) <= 2 and not _starts_line(p[0])
        if orphan and merged and p[0].start - merged[-1][-1].end < 0.6 and len(merged[-1]) + len(p) <= MAX_LINE_WORDS:
            merged[-1].extend(p)
        else:
            merged.append(p)
    groups = merged

    # 3. Las líneas largas se parten por el mejor punto, recursivamente.
    def refine(g: list[Word]) -> list[list[Word]]:
        too_long = len(g) > MAX_LINE_WORDS or (g[-1].end - g[0].start) > MAX_LINE_SECONDS
        if not too_long or len(g) < 2 * MIN_SIDE_WORDS:
            return [g]
        candidates = range(MIN_SIDE_WORDS - 1, len(g) - MIN_SIDE_WORDS)
        # Ligera preferencia por cortar cerca del centro.
        mid = (len(g) - 1) / 2
        best = max(candidates, key=lambda i: _split_score(g, i) - 0.02 * abs(i - mid))
        return refine(g[: best + 1]) + refine(g[best + 1 :])

    return [line for g in groups for line in refine(g)]


def _clean_text(words: list[Word]) -> str:
    text = "".join(w.text for w in words).strip()
    return " ".join(text.split())


def _load_audio(path: str):
    import librosa
    import numpy as np
    import soundfile as sf

    audio, sr = sf.read(path, dtype="float32", always_2d=True)
    y = audio.mean(axis=1)
    if sr != WHISPER_SR:
        y = librosa.resample(y, orig_sr=sr, target_sr=WHISPER_SR, res_type="soxr_hq")
    return np.ascontiguousarray(y, dtype=np.float32)


@register
class TranscribeTask(Task):
    id = "transcribe"
    name = "Letras"
    description = "Transcribe la letra con marcas de tiempo por palabra (99 idiomas)."
    model_task = "lyrics"

    def available(self) -> tuple[bool, str]:
        missing = [m for m in DEPS if importlib.util.find_spec(m) is None]
        if missing:
            return False, "faltan dependencias de Python: " + ", ".join(missing)
        return True, ""

    def _model_id(self, requested: str | None) -> str:
        if requested:
            if requested not in self.models.manifests or not self.models.is_installed(requested):
                raise ValueError(f"el modelo de letras {requested} no está instalado")
            return requested
        installed = [m for m in self.models.manifests.values() if m.task == "lyrics" and self.models.is_installed(m.id)]
        if not installed:
            raise ValueError("no hay ningún modelo de letras instalado")
        return max(installed, key=lambda m: m.extra.get("priority", 0)).id

    def _load_model(self, model_dir: Path, device: str):
        """Carga en GPU (float16) y, si falla la GPU, en CPU (int8)."""
        from faster_whisper import WhisperModel

        if device == "cuda":
            register_cuda12_dlls()
            try:
                model = WhisperModel(str(model_dir), device="cuda", compute_type="float16")
                # La primera inferencia es la que carga cuBLAS: se prueba ya.
                import numpy as np

                model.detect_language(np.zeros(WHISPER_SR, dtype=np.float32))
                return model, "cuda", None
            except (RuntimeError, ValueError, OSError) as e:
                warning = f"GPU no disponible para Whisper ({e}); se usa la CPU"
                print(f"[lyrics] {warning}", file=sys.stderr)
                return WhisperModel(str(model_dir), device="cpu", compute_type="int8"), "cpu", warning
        return WhisperModel(str(model_dir), device="cpu", compute_type="int8"), "cpu", None

    def run(self, params: dict[str, Any], job: Job) -> dict[str, Any]:
        started = time.monotonic()
        model_id = self._model_id(params.get("model_id"))
        device = params.get("device") or "auto"
        if device == "auto":
            device = best_device()
        device = "cuda" if device.startswith("cuda") else "cpu"  # CTranslate2 no admite MPS
        language = params.get("language") or None
        if language == "auto":
            language = None

        job.report(0.0, "leyendo audio", force=True)
        audio = _load_audio(params["input"])
        duration = len(audio) / WHISPER_SR
        job.report(0.03, "cargando modelo", force=True)
        model, device, warning = self._load_model(self.models.model_dir(model_id), device)
        job.check()

        segments, info = model.transcribe(
            audio,
            language=language,
            task="transcribe",
            beam_size=5,
            word_timestamps=True,
            vad_filter=True,
            vad_parameters={"min_silence_duration_ms": 700},
            # Sin condicionar al texto anterior se evitan los bucles de repetición
            # típicos de Whisper con música.
            condition_on_previous_text=False,
        )
        words: list[Word] = []
        for seg in segments:  # generador: cada paso decodifica una ventana
            job.check()
            # Descarta segmentos que casi seguro son alucinaciones (silencio o ruido).
            if seg.no_speech_prob > 0.6 and seg.avg_logprob < -1.0:
                continue
            if seg.compression_ratio > 2.6:
                continue
            for w in seg.words or []:
                if w.word.strip():
                    words.append(Word(float(w.start), float(w.end), w.word, float(w.probability)))
            job.report(0.08 + 0.87 * min(1.0, seg.end / max(duration, 1e-6)), "transcribiendo")

        # Marcas de tiempo monótonas y sin solapes.
        words.sort(key=lambda w: w.start)
        for a, b in zip(words, words[1:]):
            a.end = min(a.end, b.start)
            a.end = max(a.end, a.start)

        lines = []
        for line in split_lines(words):
            lines.append(
                {
                    "start": round(line[0].start, 3),
                    "end": round(line[-1].end, 3),
                    "text": _clean_text(line),
                    "words": [
                        {"start": round(w.start, 3), "end": round(w.end, 3), "text": w.text.strip(), "prob": round(w.prob, 3)}
                        for w in line
                    ],
                }
            )

        result = {
            "version": LYRICS_VERSION,
            "language": info.language,
            "language_probability": round(float(info.language_probability), 3),
            "model_id": model_id,
            "source": params.get("source", "mix"),
            "duration": round(duration, 3),
            "lines": lines,
            "seconds": round(time.monotonic() - started, 2),
            "device": device,
            "warning": warning,
        }
        out = params.get("output")
        if out:
            out = Path(out)
            out.parent.mkdir(parents=True, exist_ok=True)
            tmp = out.with_suffix(".tmp")
            tmp.write_text(json.dumps(result, ensure_ascii=False), encoding="utf-8")
            os.replace(tmp, out)
        job.report(1.0, "listo", force=True)
        if out:
            return {"output": str(out), "language": info.language, "lines": len(lines), "seconds": result["seconds"]}
        return result
