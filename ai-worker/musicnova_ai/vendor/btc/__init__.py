# Copyright (c) 2019 Jonggwon Park. MIT License (ver LICENSE).
"""BTC: «A Bi-Directional Transformer for Musical Chord Recognition» (Park et
al., ISMIR 2019), modelo de vocabulario amplio (170 clases: 14 tipos × 12 raíces,
«X» y «N»).

Adaptado de `btc_model.py`, `utils/transformer_modules.py` y
`utils/mir_eval_modules.py` del repositorio jayg996/BTC-ISMIR19 (commit
2682317b). Cambios de MusicNova: solo inferencia (sin pérdida, etiquetas ni
mapas de atención), `np.float` → `np.float64`, carga segura de los pesos
(`weights_only=True`) y salida de probabilidades por trama. Se conservan las
peculiaridades del original que afectan a los pesos (ReLU tras la última capa
de la red de posición, LSTM sin usar en la capa de salida).
"""

from __future__ import annotations

import math
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn
import torch.nn.functional as F

SR = 22_050
N_BINS = 144
BINS_PER_OCTAVE = 24
HOP = 2048
TIMESTEP = 108
INST_LEN = 10.0
FRAME_SECONDS = INST_LEN / TIMESTEP  # ≈ 0,0926 s
CONFIG = {
    "feature_size": 144,
    "timestep": TIMESTEP,
    "num_chords": 170,
    "num_layers": 8,
    "num_heads": 4,
    "hidden_size": 128,
    "total_key_depth": 128,
    "total_value_depth": 128,
    "filter_size": 128,
}
ROOTS = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
QUALITIES = ["min", "maj", "dim", "aug", "min6", "maj6", "min7", "minmaj7", "maj7", "7", "dim7", "hdim7", "sus2", "sus4"]
X_INDEX, N_INDEX = 168, 169


def index_to_chord(i: int) -> tuple[int | None, str | None]:
    """(raíz 0–11, tipo de BTC) de una clase; (None, None) para «N» y «X»."""
    if i >= 168:
        return None, None
    return i // 14, QUALITIES[i % 14]


def _gen_bias_mask(max_length: int) -> torch.Tensor:
    mask = np.triu(np.full([max_length, max_length], -np.inf), 1)
    return torch.from_numpy(mask).type(torch.FloatTensor).unsqueeze(0).unsqueeze(1)


def _gen_timing_signal(length: int, channels: int, min_timescale=1.0, max_timescale=1.0e4) -> torch.Tensor:
    position = np.arange(length)
    num_timescales = channels // 2
    log_inc = math.log(float(max_timescale) / float(min_timescale)) / (float(num_timescales) - 1)
    inv = min_timescale * np.exp(np.arange(num_timescales).astype(np.float64) * -log_inc)
    scaled = np.expand_dims(position, 1) * np.expand_dims(inv, 0)
    signal = np.concatenate([np.sin(scaled), np.cos(scaled)], axis=1)
    signal = np.pad(signal, [[0, 0], [0, channels % 2]], "constant", constant_values=[0.0, 0.0])
    return torch.from_numpy(signal.reshape([1, length, channels])).type(torch.FloatTensor)


class LayerNorm(nn.Module):
    def __init__(self, features, eps=1e-6):
        super().__init__()
        self.gamma = nn.Parameter(torch.ones(features))
        self.beta = nn.Parameter(torch.zeros(features))
        self.eps = eps

    def forward(self, x):
        mean = x.mean(-1, keepdim=True)
        std = x.std(-1, keepdim=True)
        return self.gamma * (x - mean) / (std + self.eps) + self.beta


class MultiHeadAttention(nn.Module):
    def __init__(self, input_depth, total_key_depth, total_value_depth, output_depth, num_heads, bias_mask=None):
        super().__init__()
        self.num_heads = num_heads
        self.query_scale = (total_key_depth // num_heads) ** -0.5
        self.bias_mask = bias_mask
        self.query_linear = nn.Linear(input_depth, total_key_depth, bias=False)
        self.key_linear = nn.Linear(input_depth, total_key_depth, bias=False)
        self.value_linear = nn.Linear(input_depth, total_value_depth, bias=False)
        self.output_linear = nn.Linear(total_value_depth, output_depth, bias=False)

    def _split(self, x):
        b, t, d = x.shape
        return x.view(b, t, self.num_heads, d // self.num_heads).permute(0, 2, 1, 3)

    def _merge(self, x):
        b, h, t, d = x.shape
        return x.permute(0, 2, 1, 3).contiguous().view(b, t, d * h)

    def forward(self, queries, keys, values):
        q = self._split(self.query_linear(queries)) * self.query_scale
        k = self._split(self.key_linear(keys))
        v = self._split(self.value_linear(values))
        logits = torch.matmul(q, k.permute(0, 1, 3, 2))
        if self.bias_mask is not None:
            logits = logits + self.bias_mask[:, :, : logits.shape[-2], : logits.shape[-1]].to(logits)
        weights = F.softmax(logits, dim=-1)
        return self.output_linear(self._merge(torch.matmul(weights, v)))


class Conv(nn.Module):
    def __init__(self, input_size, output_size, kernel_size, pad_type):
        super().__init__()
        padding = (kernel_size - 1, 0) if pad_type == "left" else (kernel_size // 2, (kernel_size - 1) // 2)
        self.pad = nn.ConstantPad1d(padding, 0)
        self.conv = nn.Conv1d(input_size, output_size, kernel_size=kernel_size, padding=0)

    def forward(self, inputs):
        return self.conv(self.pad(inputs.permute(0, 2, 1))).permute(0, 2, 1)


class PositionwiseFeedForward(nn.Module):
    def __init__(self, input_depth, filter_size, output_depth):
        super().__init__()
        # Configuración «cc» con relleno a ambos lados, como en BTC.
        self.layers = nn.ModuleList(
            [Conv(input_depth, filter_size, 3, "both"), Conv(filter_size, output_depth, 3, "both")]
        )
        self.relu = nn.ReLU()

    def forward(self, x):
        for layer in self.layers:
            x = self.relu(layer(x))  # el original aplica ReLU también tras la última capa
        return x


class SelfAttentionBlock(nn.Module):
    def __init__(self, hidden, key_depth, value_depth, filter_size, num_heads, bias_mask):
        super().__init__()
        self.multi_head_attention = MultiHeadAttention(hidden, key_depth, value_depth, hidden, num_heads, bias_mask)
        self.positionwise_convolution = PositionwiseFeedForward(hidden, filter_size, hidden)
        self.layer_norm_mha = LayerNorm(hidden)
        self.layer_norm_ffn = LayerNorm(hidden)

    def forward(self, x):
        n = self.layer_norm_mha(x)
        x = x + self.multi_head_attention(n, n, n)
        return x + self.positionwise_convolution(self.layer_norm_ffn(x))


class BiDirectionalSelfAttention(nn.Module):
    def __init__(self, hidden, key_depth, value_depth, filter_size, num_heads, max_length):
        super().__init__()
        self.attn_block = SelfAttentionBlock(hidden, key_depth, value_depth, filter_size, num_heads, _gen_bias_mask(max_length))
        self.backward_attn_block = SelfAttentionBlock(
            hidden, key_depth, value_depth, filter_size, num_heads, torch.transpose(_gen_bias_mask(max_length), 2, 3)
        )
        self.linear = nn.Linear(hidden * 2, hidden)

    def forward(self, x):
        return self.linear(torch.cat((self.attn_block(x), self.backward_attn_block(x)), dim=2))


class BiDirectionalLayers(nn.Module):
    def __init__(self, c):
        super().__init__()
        self.timing_signal = _gen_timing_signal(c["timestep"], c["hidden_size"])
        self.embedding_proj = nn.Linear(c["feature_size"], c["hidden_size"], bias=False)
        self.self_attn_layers = nn.Sequential(
            *[
                BiDirectionalSelfAttention(
                    c["hidden_size"], c["total_key_depth"], c["total_value_depth"], c["filter_size"], c["num_heads"], c["timestep"]
                )
                for _ in range(c["num_layers"])
            ]
        )
        self.layer_norm = LayerNorm(c["hidden_size"])

    def forward(self, inputs):
        x = self.embedding_proj(inputs)
        x = x + self.timing_signal[:, : inputs.shape[1], :].to(x)
        return self.layer_norm(self.self_attn_layers(x))


class OutputLayer(nn.Module):
    def __init__(self, hidden_size, output_size):
        super().__init__()
        self.output_projection = nn.Linear(hidden_size, output_size)
        # Presente en los pesos aunque BTC no lo usa.
        self.lstm = nn.LSTM(input_size=hidden_size, hidden_size=hidden_size // 2, batch_first=True, bidirectional=True)

    def forward(self, hidden):
        return self.output_projection(hidden)


class BTC(nn.Module):
    def __init__(self, c=CONFIG):
        super().__init__()
        self.self_attn_layers = BiDirectionalLayers(c)
        self.output_layer = OutputLayer(c["hidden_size"], c["num_chords"])

    def forward(self, x):
        return self.output_layer(self.self_attn_layers(x))


def load(checkpoint: str | Path, device: str = "cpu") -> tuple[BTC, float, float]:
    """Modelo y normalización (media, desviación) del checkpoint, cargado sin
    ejecutar código: solo tensores y escalares de numpy."""
    import codecs

    safe = [(np._core.multiarray.scalar, "numpy.core.multiarray.scalar"), np.dtype, type(np.dtype("float64")), codecs.encode]
    with torch.serialization.safe_globals(safe):
        ckpt = torch.load(str(checkpoint), map_location="cpu", weights_only=True)
    model = BTC()
    missing, unexpected = model.load_state_dict(ckpt["model"], strict=False)
    if missing or unexpected:
        raise ValueError(f"pesos de BTC incompatibles: faltan {missing[:3]}, sobran {unexpected[:3]}")
    return model.to(device).eval(), float(ckpt["mean"]), float(ckpt["std"])


def features(y: np.ndarray) -> np.ndarray:
    """CQT logarítmica (tramas × 144) como en BTC: por trozos de 10 s a 22 050 Hz."""
    import librosa

    chunk = int(SR * INST_LEN)
    parts = []
    start = 0
    while len(y) > start + chunk:
        parts.append(librosa.cqt(y[start : start + chunk], sr=SR, n_bins=N_BINS, bins_per_octave=BINS_PER_OCTAVE, hop_length=HOP))
        start += chunk
    parts.append(librosa.cqt(y[start:], sr=SR, n_bins=N_BINS, bins_per_octave=BINS_PER_OCTAVE, hop_length=HOP))
    return np.log(np.abs(np.concatenate(parts, axis=1)) + 1e-6).T


@torch.inference_mode()
def predict_probs(model: BTC, mean: float, std: float, y: np.ndarray, device: str = "cpu") -> np.ndarray:
    """Probabilidades (tramas × 170) cada FRAME_SECONDS para audio mono a 22 050 Hz."""
    feat = (features(y) - mean) / std
    n = feat.shape[0]
    pad = (-n) % TIMESTEP
    feat = np.pad(feat, ((0, pad), (0, 0)))
    x = torch.tensor(feat, dtype=torch.float32, device=device).view(-1, TIMESTEP, N_BINS)
    probs = F.softmax(model(x), dim=-1).reshape(-1, CONFIG["num_chords"]).cpu().numpy()
    return probs[:n]
