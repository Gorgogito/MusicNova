# Copyright (c) 2021 Hendrik Schröter. Licensed under MIT or Apache-2.0 (ver LICENSE).
"""DeepFilterNet3 (Schröter et al., 2023): reducción de ruido de voz a 48 kHz.

Port en numpy del procesado de señal de libDF (`libDF/src/lib.rs`, `tract.rs`)
y del `forward` de `df/deepfilternet3.py` y `df/multiframe.py` del repositorio
Rikorose/DeepFilterNet (commit 84d57ec2), para usar los tres grafos ONNX
publicados (`enc.onnx`, `erb_dec.onnx`, `df_dec.onnx`) con onnxruntime, sin
PyTorch ni la extensión en Rust. Proceso de archivo completo (como
`df.enhance.enhance`), no en tiempo real.

Etapas: STFT con ventana Vorbis (960/480) → rasgos ERB (32 bandas, dB con media
exponencial) y espectro complejo normalizado (96 bins) → codificador →
máscara ERB para todo el espectro y filtrado profundo (orden 5, 2 tramas de
anticipación) en los 96 bins graves → ISTFT.
"""

from __future__ import annotations

import math
from pathlib import Path

import numpy as np

SR = 48_000
FFT_SIZE = 960
HOP = 480
NB_ERB = 32
NB_DF = 96
DF_ORDER = 5
DF_LOOKAHEAD = 2
CONV_LOOKAHEAD = 2
MIN_NB_ERB_FREQS = 2
NORM_TAU = 1.0
MEAN_NORM_INIT = (-60.0, -90.0)
UNIT_NORM_INIT = (0.001, 0.0001)
MODEL_FILES = ("enc.onnx", "erb_dec.onnx", "df_dec.onnx")


def _freq2erb(f: float) -> float:
    return 9.265 * math.log1p(f / (24.7 * 9.265))


def _erb2freq(e: float) -> float:
    return 24.7 * 9.265 * (math.exp(e / 9.265) - 1.0)


def erb_widths(sr: int = SR, fft_size: int = FFT_SIZE, nb_bands: int = NB_ERB, min_nb_freqs: int = MIN_NB_ERB_FREQS) -> list[int]:
    """Número de bins de FFT por banda ERB (`erb_fb` de libDF)."""
    freq_width = sr / fft_size
    erb_low, erb_high = _freq2erb(0.0), _freq2erb(sr / 2)
    step = (erb_high - erb_low) / nb_bands
    widths = [0] * nb_bands
    prev_freq = 0
    freq_over = 0
    for i in range(1, nb_bands + 1):
        fb = int(round(_erb2freq(erb_low + i * step) / freq_width))
        nb = fb - prev_freq - freq_over
        if nb < min_nb_freqs:
            freq_over = min_nb_freqs - nb
            nb = min_nb_freqs
        else:
            freq_over = 0
        widths[i - 1] = nb
        prev_freq = fb
    widths[-1] += 1
    too_large = sum(widths) - (fft_size // 2 + 1)
    if too_large > 0:
        widths[-1] -= too_large
    assert sum(widths) == fft_size // 2 + 1
    return widths


def norm_alpha(sr: int = SR, hop: int = HOP, tau: float = NORM_TAU) -> float:
    """Factor de la media exponencial, redondeado como `calc_norm_alpha`."""
    alpha = math.exp(-(hop / sr) / tau)
    precision = 3
    a = 1.0
    while a >= 1.0:
        a = round(alpha * 10**precision) / 10**precision
        precision += 1
    return a


def vorbis_window(n: int = FFT_SIZE) -> np.ndarray:
    half = n // 2
    i = np.arange(n, dtype=np.float64)
    s = np.sin(0.5 * np.pi * (i + 0.5) / half)
    return np.sin(0.5 * np.pi * s * s).astype(np.float32)


def analysis(x: np.ndarray) -> np.ndarray:
    """STFT de libDF: la trama t cubre [t·hop − (N − hop), (t + 1)·hop) con ceros
    antes del inicio; normalizada solo en el análisis. → (T, F) complejo."""
    w = vorbis_window()
    n_frames = len(x) // HOP
    padded = np.concatenate([np.zeros(FFT_SIZE - HOP, dtype=np.float32), x[: n_frames * HOP]])
    idx = np.arange(n_frames)[:, None] * HOP + np.arange(FFT_SIZE)[None, :]
    frames = padded[idx] * w
    wnorm = 1.0 / (FFT_SIZE**2 / (2 * HOP))
    return (np.fft.rfft(frames, axis=1) * wnorm).astype(np.complex64)


def synthesis(spec: np.ndarray) -> np.ndarray:
    """ISTFT con solapamiento y suma (la FFT inversa de libDF no divide por N)."""
    w = vorbis_window()
    frames = np.fft.irfft(spec, n=FFT_SIZE, axis=1).astype(np.float32) * FFT_SIZE * w
    n = spec.shape[0]
    out = np.zeros((n + 1) * HOP, dtype=np.float32)
    for k in range(FFT_SIZE // HOP):
        out[k * HOP : k * HOP + n * HOP] += frames[:, k * HOP : (k + 1) * HOP].reshape(-1)
    return out[: n * HOP]


def _exp_mean(x: np.ndarray, init: np.ndarray, alpha: float) -> np.ndarray:
    """s_t = (1 − α)·x_t + α·s_{t−1} por columnas, con s_{−1} = init."""
    from scipy.signal import lfilter

    zi = (alpha * init)[None, :]
    s, _ = lfilter([1.0 - alpha], [1.0, -alpha], x, axis=0, zi=zi)
    return s


def features(spec: np.ndarray, widths: list[int], alpha: float) -> tuple[np.ndarray, np.ndarray]:
    """Rasgos de entrada: ERB (1, 1, T, 32) y espectro complejo (1, 2, T, 96)."""
    power = (spec.real**2 + spec.imag**2).astype(np.float64)
    edges = np.concatenate([[0], np.cumsum(widths)])
    band = np.stack([power[:, a:b].mean(axis=1) for a, b in zip(edges[:-1], edges[1:])], axis=1)
    erb_db = 10.0 * np.log10(band + 1e-10)
    erb_init = np.linspace(MEAN_NORM_INIT[0], MEAN_NORM_INIT[1], NB_ERB)
    feat_erb = (erb_db - _exp_mean(erb_db, erb_init, alpha)) / 40.0

    low = spec[:, :NB_DF].astype(np.complex128)
    unit_init = np.linspace(UNIT_NORM_INIT[0], UNIT_NORM_INIT[1], NB_DF)
    norm = np.sqrt(_exp_mean(np.abs(low), unit_init, alpha))
    feat_cplx = low / norm
    feat_spec = np.stack([feat_cplx.real, feat_cplx.imag])[None].astype(np.float32)
    return feat_erb[None, None].astype(np.float32), feat_spec


def _lookahead(x: np.ndarray, n: int) -> np.ndarray:
    """ConstantPad2d((0, 0, −n, n)) en el eje temporal (penúltimo eje)."""
    if n == 0:
        return x
    pad = np.zeros(x.shape[:-2] + (n, x.shape[-1]), dtype=x.dtype)
    return np.concatenate([x[..., n:, :], pad], axis=-2)


def deep_filter(spec: np.ndarray, coefs: np.ndarray) -> np.ndarray:
    """y[t, f] = Σ_n x[t + n − (orden − 1 − anticipación), f] · c[n, t, f] en los
    NB_DF bins graves. spec (T, F) complejo; coefs (orden, T, NB_DF) complejo."""
    t = spec.shape[0]
    front = DF_ORDER - 1 - DF_LOOKAHEAD
    padded = np.concatenate(
        [np.zeros((front, NB_DF), spec.dtype), spec[:, :NB_DF], np.zeros((DF_LOOKAHEAD, NB_DF), spec.dtype)]
    )
    out = np.zeros((t, NB_DF), dtype=np.complex64)
    for n in range(DF_ORDER):
        out += padded[n : n + t] * coefs[n]
    return out


class DeepFilterNet3:
    def __init__(self, model_dir: str | Path, device: str = "cpu"):
        import onnxruntime as ort

        providers = ["CPUExecutionProvider"]
        if device.startswith("cuda") and "CUDAExecutionProvider" in ort.get_available_providers():
            providers.insert(0, "CUDAExecutionProvider")
        opts = ort.SessionOptions()
        opts.log_severity_level = 3
        d = Path(model_dir)
        self.enc, self.erb_dec, self.df_dec = (
            ort.InferenceSession(str(d / name), sess_options=opts, providers=providers) for name in MODEL_FILES
        )
        self.widths = erb_widths()
        self.alpha = norm_alpha()

    def enhance_spec(self, spec: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        """Espectro mejorado (T, F) y SNR local estimada por trama (dB)."""
        feat_erb, feat_spec = features(spec, self.widths, self.alpha)
        feat_erb = _lookahead(feat_erb, CONV_LOOKAHEAD)
        feat_spec = _lookahead(feat_spec, CONV_LOOKAHEAD)
        e0, e1, e2, e3, emb, c0, lsnr = self.enc.run(None, {"feat_erb": feat_erb, "feat_spec": feat_spec})
        (mask,) = self.erb_dec.run(None, {"emb": emb, "e3": e3, "e2": e2, "e1": e1, "e0": e0})
        coefs = self.df_dec.run(None, {"emb": emb, "c0": c0})[0]
        # Máscara ERB → ganancia por bin para todo el espectro.
        gains = np.repeat(mask[0, 0], self.widths, axis=1)
        enhanced = spec * gains
        # (1, T, F', orden·2) → (orden, T, F') complejo, como DfOutputReshapeMF.
        c = coefs[0].reshape(coefs.shape[1], NB_DF, DF_ORDER, 2)
        c = (c[..., 0] + 1j * c[..., 1]).transpose(2, 0, 1).astype(np.complex64)
        enhanced[:, :NB_DF] = deep_filter(spec, c)
        return enhanced, lsnr[0, :, 0]

    def enhance(self, audio: np.ndarray, atten_lim_db: float | None = None) -> np.ndarray:
        """Audio mono a 48 kHz → audio mejorado del mismo largo (sin retardo).
        `atten_lim_db` limita cuánto se atenúa el ruido (p. ej. 12 dB)."""
        x = np.asarray(audio, dtype=np.float32)
        orig = len(x)
        x = np.concatenate([x, np.zeros(FFT_SIZE, dtype=np.float32)])
        spec = analysis(x)
        enhanced, _ = self.enhance_spec(spec)
        if atten_lim_db:
            lim = 10 ** (-abs(atten_lim_db) / 20)
            enhanced = spec * lim + enhanced * (1 - lim)
        y = synthesis(enhanced)
        d = FFT_SIZE - HOP
        return y[d : orig + d]
