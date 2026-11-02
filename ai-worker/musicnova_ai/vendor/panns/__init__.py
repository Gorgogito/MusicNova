"""PANNs: redes Cnn10 y Cnn14 para etiquetar audio con las 527 clases de AudioSet.

Arquitectura de «PANNs: Large-Scale Pretrained Audio Neural Networks for Audio
Pattern Recognition» (Kong et al., 2020), según `pytorch/models.py` de
qiuqiangkong/audioset_tagging_cnn (licencia MIT, Copyright (c) 2019 Qiuqiang
Kong). Los pesos (Zenodo 3987831) tienen licencia CC BY 4.0.

Cambios de MusicNova: el espectrograma log-mel se calcula con `torch.stft` y el
banco de filtros de librosa (equivalente a torchlibrosa, que no hace falta
instalar), y se quitan el aumento de datos y las salidas de embeddings.
`class_labels_indices.csv` es la lista de clases de AudioSet (Google, CC BY 4.0).
"""

from __future__ import annotations

from pathlib import Path

import torch
import torch.nn as nn
import torch.nn.functional as F

SAMPLE_RATE = 32_000
N_FFT = 1024
HOP = 320
N_MELS = 64
FMIN = 50
FMAX = 14_000
CLASSES = 527


class ConvBlock(nn.Module):
    def __init__(self, in_channels: int, out_channels: int):
        super().__init__()
        self.conv1 = nn.Conv2d(in_channels, out_channels, kernel_size=(3, 3), stride=(1, 1), padding=(1, 1), bias=False)
        self.conv2 = nn.Conv2d(out_channels, out_channels, kernel_size=(3, 3), stride=(1, 1), padding=(1, 1), bias=False)
        self.bn1 = nn.BatchNorm2d(out_channels)
        self.bn2 = nn.BatchNorm2d(out_channels)

    def forward(self, x, pool_size=(2, 2)):
        x = F.relu_(self.bn1(self.conv1(x)))
        x = F.relu_(self.bn2(self.conv2(x)))
        return F.avg_pool2d(x, kernel_size=pool_size)


class LogMel(nn.Module):
    """Espectrograma de potencia → banco mel de librosa (Slaney) → 10·log10."""

    def __init__(self):
        super().__init__()
        import librosa

        mel = librosa.filters.mel(sr=SAMPLE_RATE, n_fft=N_FFT, n_mels=N_MELS, fmin=FMIN, fmax=FMAX)
        self.register_buffer("mel", torch.from_numpy(mel.T).float(), persistent=False)
        self.register_buffer("window", torch.hann_window(N_FFT, periodic=True), persistent=False)

    def forward(self, x):
        spec = torch.stft(
            x, n_fft=N_FFT, hop_length=HOP, win_length=N_FFT, window=self.window, center=True, pad_mode="reflect", return_complex=True
        )
        power = spec.real**2 + spec.imag**2  # (batch, freq, time)
        mel = torch.matmul(power.transpose(1, 2), self.mel)  # (batch, time, mel)
        return (10.0 * torch.log10(torch.clamp(mel, min=1e-10))).unsqueeze(1)


class _Tagger(nn.Module):
    blocks: list[int]

    def __init__(self, channels: list[int]):
        super().__init__()
        self.logmel = LogMel()
        self.bn0 = nn.BatchNorm2d(N_MELS)
        ins = [1] + channels[:-1]
        for i, (a, b) in enumerate(zip(ins, channels), start=1):
            setattr(self, f"conv_block{i}", ConvBlock(a, b))
        self.n_blocks = len(channels)
        width = channels[-1]
        self.fc1 = nn.Linear(width, width, bias=True)
        self.fc_audioset = nn.Linear(width, CLASSES, bias=True)

    def forward(self, waveform):
        """Audio mono a 32 kHz (batch, muestras) → probabilidad de cada clase (batch, 527)."""
        x = self.logmel(waveform)
        x = x.transpose(1, 3)
        x = self.bn0(x)
        x = x.transpose(1, 3)
        for i in range(1, self.n_blocks + 1):
            pool = (1, 1) if i == 6 else (2, 2)
            x = getattr(self, f"conv_block{i}")(x, pool_size=pool)
        x = torch.mean(x, dim=3)
        x = torch.max(x, dim=2).values + torch.mean(x, dim=2)
        x = F.relu_(self.fc1(x))
        return torch.sigmoid(self.fc_audioset(x))


class Cnn14(_Tagger):
    def __init__(self):
        super().__init__([64, 128, 256, 512, 1024, 2048])


class Cnn10(_Tagger):
    def __init__(self):
        super().__init__([64, 128, 256, 512])


ARCHS = {"Cnn14": Cnn14, "Cnn10": Cnn10}

#: Claves del punto de control que este código no usa (extractores de torchlibrosa).
_IGNORED = ("spectrogram_extractor.", "logmel_extractor.")


def load(arch: str, checkpoint: Path, device: str) -> nn.Module:
    if arch not in ARCHS:
        raise ValueError(f"arquitectura PANNs desconocida: {arch}")
    model = ARCHS[arch]()
    state = torch.load(str(checkpoint), map_location="cpu", weights_only=True)
    state = state.get("model", state)
    state = {k: v for k, v in state.items() if not k.startswith(_IGNORED)}
    missing, unexpected = model.load_state_dict(state, strict=False)
    if missing or unexpected:
        raise ValueError(f"el punto de control no encaja con {arch}: faltan {missing[:3]}, sobran {unexpected[:3]}")
    return model.to(device).eval()
