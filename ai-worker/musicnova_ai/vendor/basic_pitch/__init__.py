# Copyright 2022 Spotify AB
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
"""Basic Pitch (Spotify, Apache-2.0): transcripción polifónica de audio a notas.

Adaptado de `basic_pitch/constants.py`, `inference.py` y `note_creation.py`
del commit fa5997af0a8210982619003269994a1be25eddf3. El modelo `nmp.onnx` es el
ICASSP 2022 publicado en ese mismo commit.

Cambios de MusicNova (permitidos por la licencia Apache-2.0, sección 4):
- solo se usa el modelo ONNX (onnxruntime), sin TensorFlow, CoreML ni TFLite;
- el audio llega ya cargado a 22 050 Hz y la inferencia admite avance y cancelación;
- se quitan pretty_midi, mir_eval y resampy (la exportación MIDI la hace la aplicación);
- las conversiones Hz/MIDI/tiempo se hacen con numpy en lugar de librosa.
"""

from __future__ import annotations

from pathlib import Path
from typing import Callable, Optional

import numpy as np
import scipy.signal

MODEL_PATH = Path(__file__).with_name("nmp.onnx")

# ---- constants.py -------------------------------------------------------------------
FFT_HOP = 256
CONTOURS_BINS_PER_SEMITONE = 3
ANNOTATIONS_BASE_FREQUENCY = 27.5
ANNOTATIONS_N_SEMITONES = 88
AUDIO_SAMPLE_RATE = 22050
N_FREQ_BINS_CONTOURS = ANNOTATIONS_N_SEMITONES * CONTOURS_BINS_PER_SEMITONE
AUDIO_WINDOW_LENGTH = 2
ANNOTATIONS_FPS = AUDIO_SAMPLE_RATE // FFT_HOP
ANNOT_N_FRAMES = ANNOTATIONS_FPS * AUDIO_WINDOW_LENGTH
AUDIO_N_SAMPLES = AUDIO_SAMPLE_RATE * AUDIO_WINDOW_LENGTH - FFT_HOP

# ---- inference.py ------------------------------------------------------------------
DEFAULT_ONSET_THRESHOLD = 0.5
DEFAULT_FRAME_THRESHOLD = 0.3
DEFAULT_MINIMUM_NOTE_LENGTH_MS = 127.7
DEFAULT_OVERLAPPING_FRAMES = 30

# ---- note_creation.py --------------------------------------------------------------
MIDI_OFFSET = 21
MAX_FREQ_IDX = 87
ENERGY_TOLERANCE = 11
MAGIC_ALIGNMENT_OFFSET = 0.0018


def _hz_to_midi(f: float) -> float:
    return 12.0 * (np.log2(f) - np.log2(440.0)) + 69.0


def _midi_to_hz(m: float) -> float:
    return 440.0 * 2.0 ** ((m - 69.0) / 12.0)


class Model:
    """Sesión de onnxruntime con el modelo ICASSP 2022."""

    def __init__(self, model_path: Path = MODEL_PATH, device: str = "cpu"):
        import onnxruntime as ort

        providers = ["CPUExecutionProvider"]
        if device.startswith("cuda") and "CUDAExecutionProvider" in ort.get_available_providers():
            providers.insert(0, "CUDAExecutionProvider")
        opts = ort.SessionOptions()
        opts.log_severity_level = 3
        self.session = ort.InferenceSession(str(model_path), sess_options=opts, providers=providers)

    def predict(self, x: np.ndarray) -> dict[str, np.ndarray]:
        note, onset, contour = self.session.run(
            ["StatefulPartitionedCall:1", "StatefulPartitionedCall:2", "StatefulPartitionedCall:0"],
            {"serving_default_input_2:0": x},
        )
        return {"note": note, "onset": onset, "contour": contour}


def _unwrap_output(output: np.ndarray, audio_original_length: int, n_overlapping_frames: int, hop_size: int) -> np.ndarray:
    n_olap = int(0.5 * n_overlapping_frames)
    if n_olap > 0:
        output = output[:, n_olap:-n_olap, :]
    shape = output.shape
    unwrapped = output.reshape(shape[0] * shape[1], shape[2])
    n_expected_windows = audio_original_length / hop_size
    n_frames_per_window = (AUDIO_WINDOW_LENGTH * ANNOTATIONS_FPS) - n_overlapping_frames
    return unwrapped[: int(n_expected_windows * n_frames_per_window), :]


def run_inference(
    audio: np.ndarray,
    model: Model,
    progress: Optional[Callable[[float], None]] = None,
    batch: int = 16,
) -> dict[str, np.ndarray]:
    """Ejecuta el modelo sobre audio mono a 22 050 Hz (ventanas de 2 s solapadas)."""
    n_overlapping_frames = DEFAULT_OVERLAPPING_FRAMES
    overlap_len = n_overlapping_frames * FFT_HOP
    hop_size = AUDIO_N_SAMPLES - overlap_len

    original_length = audio.shape[0]
    audio = np.concatenate([np.zeros((overlap_len // 2,), dtype=np.float32), audio.astype(np.float32)])
    starts = list(range(0, audio.shape[0], hop_size))
    output: dict[str, list[np.ndarray]] = {"note": [], "onset": [], "contour": []}
    for b in range(0, len(starts), batch):
        windows = []
        for i in starts[b : b + batch]:
            w = audio[i : i + AUDIO_N_SAMPLES]
            if len(w) < AUDIO_N_SAMPLES:
                w = np.pad(w, (0, AUDIO_N_SAMPLES - len(w)))
            windows.append(w)
        x = np.stack(windows)[:, :, None].astype(np.float32)
        for k, v in model.predict(x).items():
            output[k].append(v)
        if progress:
            progress(min(1.0, (b + batch) / len(starts)))
    return {
        k: _unwrap_output(np.concatenate(v), original_length, n_overlapping_frames, hop_size) for k, v in output.items()
    }


def model_frames_to_time(n_frames: int) -> np.ndarray:
    original_times = np.arange(n_frames) * FFT_HOP / AUDIO_SAMPLE_RATE
    window_numbers = np.floor(np.arange(n_frames) / ANNOT_N_FRAMES)
    window_offset = (FFT_HOP / AUDIO_SAMPLE_RATE) * (ANNOT_N_FRAMES - (AUDIO_N_SAMPLES / FFT_HOP)) + MAGIC_ALIGNMENT_OFFSET
    return original_times - (window_offset * window_numbers)


def get_infered_onsets(onsets: np.ndarray, frames: np.ndarray, n_diff: int = 2) -> np.ndarray:
    diffs = []
    for n in range(1, n_diff + 1):
        frames_appended = np.concatenate([np.zeros((n, frames.shape[1])), frames])
        diffs.append(frames_appended[n:, :] - frames_appended[:-n, :])
    frame_diff = np.min(diffs, axis=0)
    frame_diff[frame_diff < 0] = 0
    frame_diff[:n_diff, :] = 0
    peak = np.max(frame_diff)
    if peak > 0:
        frame_diff = np.max(onsets) * frame_diff / peak
    return np.max([onsets, frame_diff], axis=0)


def constrain_frequency(onsets: np.ndarray, frames: np.ndarray, max_freq: Optional[float], min_freq: Optional[float]):
    n_freqs = onsets.shape[1]
    min_freq_idx = 0
    max_freq_idx = n_freqs
    if min_freq is not None:
        min_freq_idx = int(np.round(_hz_to_midi(min_freq) - MIDI_OFFSET))
    if max_freq is not None:
        max_freq_idx = int(np.round(_hz_to_midi(max_freq) - MIDI_OFFSET))
    onsets[:, :min_freq_idx] = 0
    frames[:, :min_freq_idx] = 0
    onsets[:, max_freq_idx:] = 0
    frames[:, max_freq_idx:] = 0
    return onsets, frames


def output_to_notes_polyphonic(
    frames: np.ndarray,
    onsets: np.ndarray,
    onset_thresh: float,
    frame_thresh: float,
    min_note_len: int,
    infer_onsets: bool,
    max_freq: Optional[float],
    min_freq: Optional[float],
    melodia_trick: bool = True,
    energy_tol: int = ENERGY_TOLERANCE,
    check: Optional[Callable[[], None]] = None,
) -> list[tuple[int, int, int, float]]:
    n_frames = frames.shape[0]
    onsets, frames = constrain_frequency(onsets, frames, max_freq, min_freq)
    if infer_onsets:
        onsets = get_infered_onsets(onsets, frames)

    peak_thresh_mat = np.zeros(onsets.shape)
    peaks = scipy.signal.argrelmax(onsets, axis=0)
    peak_thresh_mat[peaks] = onsets[peaks]

    onset_idx = np.where(peak_thresh_mat >= onset_thresh)
    onset_time_idx = onset_idx[0][::-1]
    onset_freq_idx = onset_idx[1][::-1]

    remaining_energy = np.array(frames, dtype=np.float64, copy=True)
    note_events = []
    for note_start_idx, freq_idx in zip(onset_time_idx, onset_freq_idx):
        if note_start_idx >= n_frames - 1:
            continue
        i = note_start_idx + 1
        k = 0
        while i < n_frames - 1 and k < energy_tol:
            if remaining_energy[i, freq_idx] < frame_thresh:
                k += 1
            else:
                k = 0
            i += 1
        i -= k
        if i - note_start_idx <= min_note_len:
            continue
        remaining_energy[note_start_idx:i, freq_idx] = 0
        if freq_idx < MAX_FREQ_IDX:
            remaining_energy[note_start_idx:i, freq_idx + 1] = 0
        if freq_idx > 0:
            remaining_energy[note_start_idx:i, freq_idx - 1] = 0
        amplitude = np.mean(frames[note_start_idx:i, freq_idx])
        note_events.append((int(note_start_idx), int(i), int(freq_idx + MIDI_OFFSET), float(amplitude)))

    if melodia_trick:
        energy_shape = remaining_energy.shape
        steps = 0
        while np.max(remaining_energy) > frame_thresh:
            steps += 1
            if check and steps % 64 == 0:
                check()
            i_mid, freq_idx = np.unravel_index(np.argmax(remaining_energy), energy_shape)
            remaining_energy[i_mid, freq_idx] = 0
            i = i_mid + 1
            k = 0
            while i < n_frames - 1 and k < energy_tol:
                if remaining_energy[i, freq_idx] < frame_thresh:
                    k += 1
                else:
                    k = 0
                remaining_energy[i, freq_idx] = 0
                if freq_idx < MAX_FREQ_IDX:
                    remaining_energy[i, freq_idx + 1] = 0
                if freq_idx > 0:
                    remaining_energy[i, freq_idx - 1] = 0
                i += 1
            i_end = i - 1 - k
            i = i_mid - 1
            k = 0
            while i > 0 and k < energy_tol:
                if remaining_energy[i, freq_idx] < frame_thresh:
                    k += 1
                else:
                    k = 0
                remaining_energy[i, freq_idx] = 0
                if freq_idx < MAX_FREQ_IDX:
                    remaining_energy[i, freq_idx + 1] = 0
                if freq_idx > 0:
                    remaining_energy[i, freq_idx - 1] = 0
                i -= 1
            i_start = i + 1 + k
            if i_end - i_start <= min_note_len:
                continue
            amplitude = np.mean(frames[i_start:i_end, freq_idx])
            note_events.append((int(i_start), int(i_end), int(freq_idx + MIDI_OFFSET), float(amplitude)))
    return note_events


def midi_pitch_to_contour_bin(pitch_midi: int) -> float:
    return 12.0 * CONTOURS_BINS_PER_SEMITONE * np.log2(_midi_to_hz(pitch_midi) / ANNOTATIONS_BASE_FREQUENCY)


def get_pitch_bends(contours: np.ndarray, note_events, n_bins_tolerance: int = 25):
    """Curva de cada nota en tercios de semitono respecto a su altura nominal."""
    window_length = n_bins_tolerance * 2 + 1
    freq_gaussian = scipy.signal.windows.gaussian(window_length, std=5)
    out = []
    for start_idx, end_idx, pitch_midi, amplitude in note_events:
        freq_idx = int(np.round(midi_pitch_to_contour_bin(pitch_midi)))
        freq_start_idx = max(freq_idx - n_bins_tolerance, 0)
        freq_end_idx = min(N_FREQ_BINS_CONTOURS, freq_idx + n_bins_tolerance + 1)
        sub = contours[start_idx:end_idx, freq_start_idx:freq_end_idx] * freq_gaussian[
            max(0, n_bins_tolerance - freq_idx) : window_length - max(0, freq_idx - (N_FREQ_BINS_CONTOURS - n_bins_tolerance - 1))
        ]
        pb_shift = n_bins_tolerance - max(0, n_bins_tolerance - freq_idx)
        bends = list(np.argmax(sub, axis=1) - pb_shift)
        out.append((start_idx, end_idx, pitch_midi, amplitude, [int(b) for b in bends]))
    return out


def predict_notes(
    audio: np.ndarray,
    model: Model,
    onset_threshold: float = DEFAULT_ONSET_THRESHOLD,
    frame_threshold: float = DEFAULT_FRAME_THRESHOLD,
    minimum_note_length_ms: float = DEFAULT_MINIMUM_NOTE_LENGTH_MS,
    minimum_frequency: Optional[float] = None,
    maximum_frequency: Optional[float] = None,
    melodia_trick: bool = True,
    progress: Optional[Callable[[float], None]] = None,
    check: Optional[Callable[[], None]] = None,
) -> list[tuple[float, float, int, float, list[int]]]:
    """Notas (inicio s, fin s, nota MIDI, amplitud 0–1, curva en tercios de semitono)."""
    output = run_inference(audio, model, progress)
    if check:
        check()
    min_note_len = int(np.round(minimum_note_length_ms / 1000 * (AUDIO_SAMPLE_RATE / FFT_HOP)))
    notes = output_to_notes_polyphonic(
        output["note"],
        output["onset"],
        onset_thresh=onset_threshold,
        frame_thresh=frame_threshold,
        min_note_len=min_note_len,
        infer_onsets=True,
        max_freq=maximum_frequency,
        min_freq=minimum_frequency,
        melodia_trick=melodia_trick,
        check=check,
    )
    with_bends = get_pitch_bends(output["contour"], notes)
    times = model_frames_to_time(output["contour"].shape[0])
    return [(float(times[s]), float(times[min(e, len(times) - 1)]), p, a, b) for s, e, p, a, b in with_bends]
