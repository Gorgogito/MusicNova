"""Fase 6: audio a notas (Basic Pitch), tono de la voz (pYIN) e instrumentos (PANNs).

Con audio sintético de altura y tiempos conocidos. La prueba de instrumentos con
PANNs necesita el modelo instalado (se salta si no) y usa voz de SAPI en Windows.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys

import numpy as np
import pytest
import soundfile as sf

from musicnova_ai.jobs import Job
from musicnova_ai.models import ModelManager
from musicnova_ai.tasks.transcription import (
    INSTRUMENTS,
    InstrumentsTask,
    NotesTask,
    PitchTask,
    crepe_available,
    monophonic,
    segments_from,
    summarize_instruments,
    track_pitch,
)

SR = 22_050


def job() -> Job:
    return Job("test", "test", lambda *a, **k: None)


def midi_hz(m: float) -> float:
    return 440.0 * 2 ** ((m - 69) / 12)


def tone(midi: float, seconds: float, sr: int = SR, harmonics=(1.0, 0.5, 0.25, 0.12)) -> np.ndarray:
    t = np.arange(int(sr * seconds)) / sr
    f = midi_hz(midi)
    env = np.minimum(1, t / 0.01) * np.exp(-t * 1.2)
    return (sum(a * np.sin(2 * np.pi * f * (h + 1) * t) for h, a in enumerate(harmonics)) * 0.25 * env).astype(np.float32)


# (inicio s, duración s, nota MIDI)
MELODY = [(0.5, 0.5, 60), (1.0, 0.5, 64), (1.5, 0.5, 67), (2.0, 0.5, 65), (2.5, 1.0, 62)]


def melody_audio(sr: int = SR) -> np.ndarray:
    y = np.zeros(int(sr * 4.5), dtype=np.float32)
    for start, dur, pitch in MELODY:
        a = int(start * sr)
        x = tone(pitch, dur, sr)
        y[a : a + len(x)] += x
    return y


def test_basic_pitch_transcribes_melody(tmp_path):
    src = tmp_path / "melodia.wav"
    sf.write(src, melody_audio(), SR, subtype="FLOAT")
    out = tmp_path / "notes.json"
    r = NotesTask(ModelManager(tmp_path / "models")).run({"input": str(src), "output": str(out), "mode": "mono", "device": "cpu"}, job())
    assert r["notes"] >= len(MELODY)
    data = json.loads(out.read_text(encoding="utf-8"))
    assert data["mode"] == "mono"
    notes = data["notes"]
    # Cada nota de la melodía aparece con su altura y su inicio (±60 ms).
    for start, dur, pitch in MELODY:
        found = [n for n in notes if n["pitch"] == pitch and abs(n["start"] - start) < 0.06]
        assert found, f"falta la nota {pitch} en {start} s: {notes}"
        assert abs(found[0]["end"] - (start + dur)) < 0.15
    # Monofónico: nunca dos notas a la vez.
    for a, b in zip(notes, notes[1:]):
        assert b["start"] >= a["end"] - 1e-6


def test_polyphonic_chord(tmp_path):
    y = np.zeros(int(SR * 3), dtype=np.float32)
    for p in (60, 64, 67):
        x = tone(p, 1.5)
        y[int(0.5 * SR) : int(0.5 * SR) + len(x)] += x
    src = tmp_path / "acorde.wav"
    sf.write(src, y, SR, subtype="FLOAT")
    r = NotesTask(ModelManager(tmp_path / "models")).run({"input": str(src), "mode": "poly", "device": "cpu"}, job())
    pitches = {n["pitch"] for n in r["notes"] if abs(n["start"] - 0.5) < 0.06}
    assert {60, 64, 67} <= pitches, r["notes"]


def test_monophonic_keeps_the_loudest():
    notes = [
        {"start": 0.0, "end": 1.0, "pitch": 60, "velocity": 100},
        {"start": 0.5, "end": 1.5, "pitch": 72, "velocity": 40},  # más débil: empieza al acabar la otra
        {"start": 1.2, "end": 2.0, "pitch": 64, "velocity": 110},  # más fuerte: recorta a la anterior
        {"start": 1.25, "end": 1.28, "pitch": 50, "velocity": 10},  # queda demasiado corta
    ]
    out = monophonic(notes, 0.05)
    assert [(n["pitch"], n["start"], n["end"]) for n in out] == [(60, 0.0, 1.0), (72, 1.0, 1.2), (64, 1.2, 2.0)]


ENGINES = [
    "pyin",
    pytest.param("crepe", marks=pytest.mark.skipif(not crepe_available(), reason="torchcrepe no instalado")),
]


@pytest.mark.parametrize("engine", ENGINES)
def test_pitch_tracking_follows_steps_and_silence(tmp_path, engine):
    sr = 16_000
    # La4 2 s, silencio 1 s, Do5 + 30 cents 2 s.
    t = np.arange(sr * 2) / sr
    a = 0.3 * np.sin(2 * np.pi * 440 * t)
    c = 0.3 * np.sin(2 * np.pi * midi_hz(72.3) * t)
    y = np.concatenate([a, np.zeros(sr), c]).astype(np.float32)
    src = tmp_path / "voz.wav"
    sf.write(src, y, sr, subtype="FLOAT")
    out = tmp_path / "pitch.json"
    r = PitchTask(ModelManager(tmp_path / "models")).run(
        {"input": str(src), "output": str(out), "engine": engine, "device": "cpu"}, job()
    )
    data = json.loads(out.read_text(encoding="utf-8"))
    assert data["source"] == r["source"] == ("pyin" if engine == "pyin" else "crepe-full")
    f0 = np.array(data["f0"])
    hop = data["hop"]
    assert hop == 0.01 and abs(len(f0) - 500) <= 2

    def cents(i0, i1, ref):
        seg = f0[int(i0 / hop) : int(i1 / hop)]
        assert (seg > 0).all(), "debería haber voz en todo el tramo"
        return np.median(1200 * np.log2(seg / ref))

    assert abs(cents(0.3, 1.7, 440.0)) < 5
    assert abs(cents(3.3, 4.7, midi_hz(72))) - 30 < 6
    assert (f0[int(2.2 / hop) : int(2.8 / hop)] == 0).all(), "silencio sin tono"


@pytest.mark.parametrize("engine", ENGINES)
def test_pitch_chunks_join_without_gaps(engine):
    sr = 16_000
    t = np.arange(sr * 45) / sr  # más de dos trozos de 20 s
    y = (0.3 * np.sin(2 * np.pi * 220 * t)).astype(np.float32)
    f0, prob = track_pitch(y, engine=engine, model="tiny")
    voiced = f0[50:-50]
    assert (voiced > 0).all(), np.where(voiced == 0)[0][:10]
    assert np.abs(1200 * np.log2(voiced / 220)).max() < 15


def test_segments_and_summary():
    assert segments_from([1, 1, 0, 1, 0, 0, 0, 0, 1], hop=1.0, window=3.0, min_gap=1.0) == [[0.0, 6.0], [8.0, 11.0]]
    assert segments_from([1, 1, 0, 1, 0, 0, 0, 0, 1], hop=1.0, window=3.0) == [[0.0, 11.0]], "huecos de ≤2 s se unen"
    labels = ["Speech", "Piano", "Guitar", "Electric guitar", "Drum kit"]
    probs = np.zeros((10, len(labels)))
    probs[:, 1] = 0.9  # piano en toda la canción
    probs[:5, 2] = 0.5  # guitarra genérica…
    probs[:5, 3] = 0.6  # …y eléctrica: solo se lista la eléctrica
    probs[0, 4] = 0.3  # batería en una sola ventana (10 %): sí aparece
    result = summarize_instruments(probs, labels, duration=12.0)
    ids = [i["id"] for i in result]
    assert ids[0] == "piano"
    assert "electric_guitar" in ids and "guitar" not in ids
    piano = result[0]
    assert piano["coverage"] == 1.0 and piano["segments"] == [[0.0, 12.0]]
    assert all(i["id"] != "speech" for i in result)
    assert all(key and label for key, label, _ in INSTRUMENTS)


def _panns_installed() -> bool:
    m = ModelManager()
    return "panns_cnn10" in m.manifests and m.is_installed("panns_cnn10")


@pytest.mark.skipif(not _panns_installed(), reason="requiere el modelo panns_cnn10 instalado")
def test_panns_recognizes_noise_and_speech(tmp_path):
    import torch

    from musicnova_ai.tasks.transcription import _labels
    from musicnova_ai.vendor import panns

    models = ModelManager()
    m = models.get("panns_cnn10")
    model = panns.load("Cnn10", models.model_dir("panns_cnn10") / m.files[0].name, "cpu")
    labels = _labels()
    assert len(labels) == 527
    t = np.arange(panns.SAMPLE_RATE * 5) / panns.SAMPLE_RATE
    # (Un seno puro deja el espectro casi vacío y el modelo lo toma por silencio.)
    noise = (0.2 * np.random.default_rng(0).standard_normal(len(t))).astype(np.float32)
    with torch.inference_mode():
        p = model(torch.from_numpy(noise[None]))[0].numpy()
    top = [labels[i] for i in np.argsort(p)[::-1][:3]]
    assert "White noise" in top and p[labels.index("Noise")] > 0.5, top

    if sys.platform == "win32" and shutil.which("powershell.exe"):
        wav = tmp_path / "voz.wav"
        script = (
            "Add-Type -AssemblyName System.Speech; "
            "$s = New-Object System.Speech.Synthesis.SpeechSynthesizer; "
            f"$s.SetOutputToWaveFile('{wav}'); "
            "$s.Speak('Esta es una prueba de reconocimiento de voz hablada para comprobar el modelo de instrumentos. "
            "Seguimos hablando durante unos segundos más para tener suficiente audio.'); $s.Dispose()"
        )
        subprocess.run(["powershell.exe", "-NoProfile", "-Command", script], check=True, timeout=60)
        out = tmp_path / "instr.json"
        task = InstrumentsTask(models)
        task.run({"input": str(wav), "output": str(out), "model_id": "panns_cnn10", "device": "cpu"}, job())
        data = json.loads(out.read_text(encoding="utf-8"))
        ids = [i["id"] for i in data["instruments"]]
        assert ids and ids[0] == "speech", data["instruments"]


# ---- Curvas de tono de las notas (pitch bends) ------------------------------------------


def test_simplify_and_note_bends():
    from musicnova_ai.tasks.transcription import note_bends, simplify_curve

    line = [(i * 0.01, i * 1.0) for i in range(50)]
    assert simplify_curve(line, 1.0) == [line[0], line[-1]]
    vee = [(i * 0.01, abs(i - 25) * 4.0) for i in range(51)]
    s = simplify_curve(vee, 1.0)
    assert len(s) == 3 and s[1] == (0.25, 0.0)
    # Casi plana: sin curva. Fuera de rango: limitada a ±200 cents.
    assert note_bends(1.0, 2.0, [(1.0 + i * 0.01, 3.0) for i in range(100)]) == []
    b = note_bends(1.0, 2.0, [(1.0 + i * 0.01, 500.0 if i > 50 else 0.0) for i in range(100)])
    assert b[0][0] == 0.0 and max(v for _, v in b) == 200.0


@pytest.mark.parametrize("bends", ["contour", pytest.param("crepe", marks=pytest.mark.skipif(not crepe_available(), reason="torchcrepe no instalado"))])
def test_notes_carry_pitch_bends(tmp_path, bends):
    # La4 sostenido 2 s con vibrato de ±30 cents a 5 Hz y +15 cents de desvío medio.
    sr = SR
    t = np.arange(int(sr * 2.0)) / sr
    cents = 15 + 30 * np.sin(2 * np.pi * 5 * t)
    f = 440 * 2 ** (cents / 1200)
    phase = 2 * np.pi * np.cumsum(f) / sr
    y = np.zeros(int(sr * 3), dtype=np.float32)
    y[int(0.5 * sr) : int(0.5 * sr) + len(t)] = (0.3 * np.sin(phase) + 0.1 * np.sin(2 * phase)) * np.minimum(1, t / 0.01)
    src = tmp_path / "vibrato.wav"
    sf.write(src, y, sr, subtype="FLOAT")
    out = tmp_path / "notes.json"
    NotesTask(ModelManager(tmp_path / "models")).run(
        {"input": str(src), "output": str(out), "mode": "mono", "bends": bends, "device": "cpu"}, job()
    )
    data = json.loads(out.read_text(encoding="utf-8"))
    assert data["bends"] == bends
    note = max(data["notes"], key=lambda n: n["end"] - n["start"])
    assert note["pitch"] == 69 and note["end"] - note["start"] > 1.5
    # La curva reconstruida sigue el vibrato real (muestras en el tramo central).
    pts = np.array(note["bends"])
    tt = np.arange(0.3, 1.5, 0.01)
    est = np.interp(tt, pts[:, 0], pts[:, 1])
    true = 15 + 30 * np.sin(2 * np.pi * 5 * (note["start"] + tt - 0.5))
    err = np.abs(est - true)
    assert np.median(err) < (8 if bends == "crepe" else 15), np.median(err)
    assert abs(np.mean(est) - 15) < 8


# ---- Batería -------------------------------------------------------------------------------


def drum_pattern(sr: int = 44_100, bars: int = 4, bpm: float = 100.0):
    """Ritmo de rock sintético: bombo en 1 y 3, caja en 2 y 4, charles en corcheas."""
    rng = np.random.default_rng(3)
    beat = 60.0 / bpm
    y = np.zeros(int(sr * (bars * 4 * beat + 1)), dtype=np.float32)
    truth = {36: [], 38: [], 42: []}

    def add(t0, x):
        a = int(t0 * sr)
        y[a : a + len(x)] += x[: len(y) - a]

    for b in range(bars * 4):
        t0 = 0.5 + b * beat
        n = int(0.3 * sr)
        tt = np.arange(n) / sr
        if b % 2 == 0:
            f = 50 + 70 * np.exp(-tt * 30)
            add(t0, 0.8 * np.sin(2 * np.pi * np.cumsum(f) / sr) * np.exp(-tt * 12))
            truth[36].append(t0)
        else:
            body = 0.4 * np.sin(2 * np.pi * 190 * tt) * np.exp(-tt * 25)
            noise = 0.5 * rng.standard_normal(n) * np.exp(-tt * 18)
            add(t0, body + noise)
            truth[38].append(t0)
        for half in (0.0, 0.5):
            m = int(0.06 * sr)
            # Ruido filtrado entre 6 y 14 kHz, como un charles cerrado.
            spec = np.fft.rfft(rng.standard_normal(m))
            fr = np.fft.rfftfreq(m, 1 / sr)
            spec[(fr < 6000) | (fr > 14000)] = 0
            hh = np.fft.irfft(spec, m) * 0.6 * np.exp(-np.arange(m) / sr * 70)
            add(t0 + half * beat, hh)
            truth[42].append(t0 + half * beat)
    return y, truth


def test_drums_transcribes_kick_snare_hihat(tmp_path):
    from musicnova_ai.tasks.drums import DrumsTask

    sr = 44_100
    y, truth = drum_pattern(sr)
    src = tmp_path / "bateria.wav"
    sf.write(src, y, sr, subtype="FLOAT")
    out = tmp_path / "notes.json"
    DrumsTask(ModelManager(tmp_path / "models")).run({"input": str(src), "output": str(out)}, job())
    data = json.loads(out.read_text(encoding="utf-8"))
    assert data["mode"] == "drums"
    notes = data["notes"]
    assert {n["pitch"] for n in notes} <= {36, 38, 42}
    for pitch, times in truth.items():
        got = [n["start"] for n in notes if n["pitch"] == pitch]
        hits = sum(1 for t in times if any(abs(g - t) <= 0.03 for g in got))
        assert hits >= 0.9 * len(times), (pitch, hits, len(times), got[:10])
        assert len(got) <= 1.15 * len(times) + 1, (pitch, len(got), len(times))


def test_stem_probs_take_each_group_from_its_stem():
    from musicnova_ai.tasks.transcription import _labels, combine_stem_probs

    labels = _labels()
    idx = {n: i for i, n in enumerate(labels)}
    mix = np.full((3, len(labels)), 0.01, dtype=np.float32)
    vocals = mix.copy()
    vocals[:, idx["Singing"]] = 0.9
    vocals[:, idx["Piano"]] = 0.8  # la voz no debe aportar piano
    other = mix.copy()
    other[:, idx["Piano"]] = 0.5
    out = combine_stem_probs(mix, {"vocals": vocals, "other": other}, labels)
    assert out[0, idx["Singing"]] == pytest.approx(0.9)
    assert out[0, idx["Piano"]] == pytest.approx(0.5)
    # Sin stem de batería, la batería sale de la mezcla; las clases ajenas a los grupos, también.
    assert out[0, idx["Drum kit"]] == pytest.approx(0.01)
    assert out[0, idx["Music"]] == pytest.approx(0.01)
    summary = {i["id"] for i in summarize_instruments(out, labels, 9.0)}
    assert {"vocals", "piano"} <= summary
