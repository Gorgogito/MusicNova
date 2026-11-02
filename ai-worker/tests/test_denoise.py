"""Fase 7: reducción de ruido de voz con DeepFilterNet3 (port del procesado de libDF).

Las pruebas del modelo necesitan DeepFilterNet3 instalado (se saltan si no) y usan
voz de SAPI en Windows con ruido blanco añadido.
"""

from __future__ import annotations

import shutil
import subprocess
import sys

import numpy as np
import pytest
import soundfile as sf

import musicnova_ai.vendor.deepfilternet as dfn
from musicnova_ai.jobs import Job
from musicnova_ai.models import ModelManager
from musicnova_ai.tasks.denoise import DenoiseTask


def job() -> Job:
    return Job("test", "test", lambda *a, **k: None)


def sisdr(ref: np.ndarray, est: np.ndarray) -> float:
    a = np.dot(est, ref) / np.dot(ref, ref)
    err = est - a * ref
    return float(10 * np.log10(np.sum((a * ref) ** 2) / np.sum(err**2)))


def test_stft_reconstructs_and_erb_bands_cover_the_spectrum():
    x = np.random.default_rng(0).standard_normal(48_000).astype(np.float32)
    padded = np.concatenate([x, np.zeros(dfn.FFT_SIZE, np.float32)])
    y = dfn.synthesis(dfn.analysis(padded))
    d = dfn.FFT_SIZE - dfn.HOP
    assert np.abs(y[d : d + len(x)] - x).max() < 1e-5
    widths = dfn.erb_widths()
    assert len(widths) == dfn.NB_ERB and sum(widths) == dfn.FFT_SIZE // 2 + 1 and min(widths) >= 2
    assert dfn.norm_alpha() == pytest.approx(0.99)


def test_deep_filter_alignment():
    # Coeficiente 1 solo en la toma «actual» (orden − 1 − anticipación): identidad.
    rng = np.random.default_rng(1)
    spec = (rng.standard_normal((10, 481)) + 1j * rng.standard_normal((10, 481))).astype(np.complex64)
    coefs = np.zeros((dfn.DF_ORDER, 10, dfn.NB_DF), np.complex64)
    coefs[dfn.DF_ORDER - 1 - dfn.DF_LOOKAHEAD] = 1.0
    assert np.allclose(dfn.deep_filter(spec, coefs), spec[:, : dfn.NB_DF])
    # La última toma mira DF_LOOKAHEAD tramas hacia delante.
    coefs[:] = 0
    coefs[-1] = 1.0
    out = dfn.deep_filter(spec, coefs)
    assert np.allclose(out[: 10 - dfn.DF_LOOKAHEAD], spec[dfn.DF_LOOKAHEAD :, : dfn.NB_DF])
    assert np.allclose(out[10 - dfn.DF_LOOKAHEAD :], 0)


def _sapi_speech(path, sr_out=48_000):
    script = (
        "Add-Type -AssemblyName System.Speech; "
        "$s = New-Object System.Speech.Synthesis.SpeechSynthesizer; "
        f"$s.SetOutputToWaveFile('{path}'); "
        "$s.Speak('Esta es una prueba de limpieza de voz. Quitamos el ruido de fondo y la voz debe quedar clara y natural.'); "
        "$s.Dispose()"
    )
    subprocess.run(["powershell.exe", "-NoProfile", "-Command", script], check=True, timeout=60)
    y, sr = sf.read(path, dtype="float32", always_2d=True)
    return y[:, 0], sr


@pytest.mark.skipif(sys.platform != "win32" or shutil.which("powershell.exe") is None, reason="requiere SAPI de Windows")
def test_denoise_task_cleans_noisy_speech(tmp_path):
    models = ModelManager()
    if not models.is_installed("deepfilternet3"):
        pytest.skip("DeepFilterNet3 no instalado")
    speech, sr = _sapi_speech(tmp_path / "voz.wav")
    speech = speech / np.abs(speech).max() * 0.5
    noise = np.random.default_rng(2).standard_normal(len(speech)).astype(np.float32)
    noise *= np.sqrt(np.mean(speech**2) / np.mean(noise**2)) * 10 ** (-5 / 20)  # SNR 5 dB
    noisy = speech + noise
    src = tmp_path / "ruidosa.wav"
    sf.write(src, np.stack([noisy, noisy], axis=1), sr, subtype="FLOAT")
    out = tmp_path / "limpia.wav"
    r = DenoiseTask(models).run({"input": str(src), "output": str(out), "device": "cpu"}, job())
    clean, out_sr = sf.read(out, dtype="float32", always_2d=True)
    assert out_sr == sr and clean.shape == (len(noisy), 2)
    before, after = sisdr(speech, noisy), sisdr(speech, clean[:, 0])
    assert after > before + 6, (before, after)
    assert r["seconds"] < 60
