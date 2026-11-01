"""Master por referencia: el resultado se parece a la referencia en nivel y brillo."""

import importlib.util

import numpy as np
import pytest

pytestmark = pytest.mark.skipif(importlib.util.find_spec("matchering") is None, reason="matchering no instalado")

SR = 44_100


def song(seconds: float, bright: float, gain: float, seed: int) -> np.ndarray:
    """Acordes + ruido rosa filtrado; `bright` sube los agudos."""
    import scipy.signal as ss

    rng = np.random.default_rng(seed)
    t = np.arange(int(SR * seconds)) / SR
    chords = sum(np.sin(2 * np.pi * f * t) for f in (220, 277.2, 329.6, 440)) / 4
    noise = ss.lfilter([0.049922, -0.095993, 0.050612, -0.004408], [1, -2.494956, 2.017265, -0.522189], rng.standard_normal(t.size))
    b, a = ss.butter(2, 4000 / (SR / 2), "high")
    x = 0.5 * chords + 0.3 * noise / np.abs(noise).max()
    x = x + bright * ss.lfilter(b, a, x)
    x = x * (0.7 + 0.3 * np.sin(2 * np.pi * 0.25 * t))
    x = gain * x / np.abs(x).max()
    return np.stack([x, x * 0.97], axis=1).astype(np.float32)


def tilt_db(x: np.ndarray) -> float:
    import scipy.signal as ss

    f, p = ss.welch(x.mean(axis=1), SR, nperseg=8192)
    hi = p[(f >= 5000) & (f < 12000)].mean()
    mid = p[(f >= 300) & (f < 2000)].mean()
    return 10 * np.log10(hi / mid)


def rms_db(x: np.ndarray) -> float:
    return 20 * np.log10(np.sqrt((x**2).mean()))


def test_matches_reference_level_and_brightness(tmp_path):
    import soundfile as sf

    from musicnova_ai.jobs import Job
    from musicnova_ai.models import ModelManager
    from musicnova_ai.tasks.mastering import MasterReferenceTask

    target, reference = song(30, 0.0, 0.3, 1), song(30, 2.0, 0.95, 2)
    sf.write(tmp_path / "t.wav", target, SR, subtype="PCM_24")
    sf.write(tmp_path / "r.wav", reference, SR, subtype="PCM_24")
    events = []
    res = MasterReferenceTask(ModelManager()).run(
        {"target": str(tmp_path / "t.wav"), "reference": str(tmp_path / "r.wav"), "output": str(tmp_path / "out.wav")},
        Job("t", "master_reference", lambda e, d: events.append(d)),
    )
    out, sr = sf.read(res["output"], dtype="float32")
    assert sr == SR and len(out) == len(target)
    print(f"\nbrillo dB: objetivo {tilt_db(target):.1f} ref {tilt_db(reference):.1f} -> {tilt_db(out):.1f}; "
          f"RMS: {rms_db(target):.1f} ref {rms_db(reference):.1f} -> {rms_db(out):.1f}")
    # El brillo y el nivel se acercan claramente a los de la referencia.
    assert abs(tilt_db(out) - tilt_db(reference)) < 0.3 * abs(tilt_db(target) - tilt_db(reference))
    assert abs(rms_db(out) - rms_db(reference)) < 2.0
    assert np.abs(out).max() <= 1.0
    assert events[-1]["progress"] == 1.0


def test_missing_reference_is_a_clear_error(tmp_path):
    from musicnova_ai.jobs import Job
    from musicnova_ai.models import ModelManager
    from musicnova_ai.tasks.mastering import MasterReferenceTask

    with pytest.raises(ValueError, match="no se encuentra la referencia"):
        MasterReferenceTask(ModelManager()).run(
            {"target": "x.wav", "reference": str(tmp_path / "nope.wav"), "output": str(tmp_path / "o.wav")},
            Job("t", "m", lambda e, d: None),
        )
