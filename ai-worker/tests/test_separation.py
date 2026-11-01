"""Prueba real de separación. Requiere PyTorch, Demucs y el modelo htdemucs instalado."""

import importlib.util

import pytest

from musicnova_ai.jobs import Job
from musicnova_ai.models import ModelManager
from musicnova_ai.tasks.separation import SeparateTask

pytestmark = pytest.mark.skipif(
    any(importlib.util.find_spec(m) is None for m in ("torch", "demucs", "soundfile")),
    reason="dependencias de separación no instaladas",
)


def synth_mix(sr: int, seconds: float):
    import numpy as np

    t = np.arange(int(sr * seconds)) / sr
    bass = 0.35 * np.sin(2 * np.pi * np.where((t % 2) < 1, 55.0, 73.4) * t)
    rng = np.random.default_rng(0)
    drums = np.zeros_like(t)
    for beat in np.arange(0, seconds, 0.5):
        i = int(beat * sr)
        n = int(0.08 * sr)
        drums[i : i + n] += 0.5 * rng.standard_normal(min(n, len(t) - i)) * np.exp(-np.arange(min(n, len(t) - i)) / (0.015 * sr))
    f0 = 440 * (1 + 0.01 * np.sin(2 * np.pi * 5.5 * t))
    phase = 2 * np.pi * np.cumsum(f0) / sr
    voice = 0.2 * sum(np.sin(k * phase) / k for k in range(1, 6)) * (np.sin(2 * np.pi * 0.5 * t) > -0.3)
    return bass, drums, voice


def band_energy(x, sr, lo, hi):
    import numpy as np

    spec = np.abs(np.fft.rfft(x.mean(axis=1) if x.ndim == 2 else x)) ** 2
    freqs = np.fft.rfftfreq(len(x), 1 / sr)
    return spec[(freqs >= lo) & (freqs < hi)].sum() / spec.sum()


def test_htdemucs_separates_bass_and_reconstructs(tmp_path):
    import numpy as np
    import soundfile as sf

    mm = ModelManager()
    if not mm.is_installed("htdemucs"):
        pytest.skip("htdemucs no está instalado")
    sr = 44_100
    seconds = 70.0  # > 1 bloque para probar el fundido cruzado
    bass, drums, voice = synth_mix(sr, seconds)
    mix = np.stack([bass + drums + voice] * 2, axis=1).astype("float32")
    src = tmp_path / "mix.wav"
    sf.write(src, mix, sr, subtype="FLOAT")

    events = []
    job = Job("t", "separate", lambda e, d: events.append(d))
    res = SeparateTask(mm).run({"model_id": "htdemucs", "input": str(src), "output_dir": str(tmp_path / "out")}, job)

    stems = {s["name"]: sf.read(s["file"], dtype="float32")[0] for s in res["stems"]}
    assert set(stems) == {"drums", "bass", "other", "vocals"}
    assert all(len(x) == len(mix) for x in stems.values())
    assert events and events[-1]["progress"] > 0.9

    # La suma de los stems reconstruye la mezcla (residuo < -20 dB).
    total = sum(stems.values())
    residual_db = 10 * np.log10(((total - mix) ** 2).sum() / (mix**2).sum())
    assert residual_db < -20, residual_db

    # El stem de bajo concentra la energía grave.
    assert band_energy(stems["bass"], sr, 30, 120) > 0.8
    print(f"\ndispositivo={res['device']} tiempo={res['seconds']}s residuo={residual_db:.1f} dB")


def test_mel_roformer_isolates_voice(tmp_path):
    import numpy as np
    import soundfile as sf

    mm = ModelManager()
    if not mm.is_installed("mel_roformer_kim_vocals"):
        pytest.skip("mel_roformer_kim_vocals no está instalado")
    sr = 44_100
    bass, drums, voice = synth_mix(sr, 20.0)
    mix = np.stack([bass + drums + voice] * 2, axis=1).astype("float32")
    src = tmp_path / "mix.wav"
    sf.write(src, mix, sr, subtype="FLOAT")

    job = Job("t", "separate", lambda e, d: None)
    res = SeparateTask(mm).run(
        {"model_id": "mel_roformer_kim_vocals", "input": str(src), "output_dir": str(tmp_path / "out")}, job
    )
    stems = {s["name"]: sf.read(s["file"], dtype="float32")[0] for s in res["stems"]}
    assert set(stems) == {"vocals", "instrumental"}
    assert all(len(x) == len(mix) for x in stems.values())

    # La voz separada se parece a la melodía vocal y no al bajo.
    v = stems["vocals"].mean(axis=1)
    corr = lambda a, b: float(np.dot(a, b) / (np.linalg.norm(a) * np.linalg.norm(b) + 1e-9))
    print(f"\ndispositivo={res['device']} tiempo={res['seconds']}s corr_voz={corr(v, voice):.2f} corr_bajo={corr(v, bass):.2f}")
    assert corr(v, voice) > corr(v, bass)
    assert band_energy(stems["vocals"], sr, 30, 120) < 0.2
