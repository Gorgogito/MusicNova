"""Pruebas del análisis musical con una canción sintética de estructura conocida.

Forma A-B-A a 120 BPM en 4/4 (un acorde por compás de 2 s):
  A: C  Am F  G   (×4, 32 s)
  B: F  G  Em Am  (×4, 32 s, otro timbre y sin charles)
"""

import importlib.util
import json

import numpy as np
import pytest

pytestmark = pytest.mark.skipif(
    any(importlib.util.find_spec(m) is None for m in ("librosa", "scipy", "soundfile")),
    reason="dependencias de análisis no instaladas",
)

SR = 22_050
BPM = 120.0
BEAT = 60.0 / BPM
BAR = 4 * BEAT
CHORDS = {"C": (0, ""), "Am": (9, "m"), "F": (5, ""), "G": (7, ""), "Em": (4, "m")}
PART_A = ["C", "Am", "F", "G"] * 4
PART_B = ["F", "G", "Em", "Am"] * 4
FORM = PART_A + PART_B + PART_A


def _tone(freq, n, harmonics, decay=None):
    t = np.arange(n) / SR
    x = sum(a * np.sin(2 * np.pi * freq * k * t) for k, a in harmonics)
    env = np.minimum(1, t / 0.01) * (np.exp(-t / decay) if decay else 1.0)
    return x * env


def synth_song(seed=0):
    rng = np.random.default_rng(seed)
    total = int(len(FORM) * BAR * SR)
    mix = np.zeros(total)
    harm = np.zeros(total)
    bass_track = np.zeros(total)
    bar_n = int(BAR * SR)
    beat_n = int(BEAT * SR)
    for i, name in enumerate(FORM):
        root, q = CHORDS[name]
        part_b = 16 <= i < 32
        start = i * bar_n
        third = 3 if q == "m" else 4
        pad_h = [(1, 1.0), (2, 0.5), (3, 0.3), (4, 0.2)] if not part_b else [(1, 1.0), (2, 0.8), (3, 0.6), (5, 0.4), (6, 0.3)]
        for interval in (0, third, 7):
            f = 261.63 * 2 ** ((root + interval) / 12)
            harm[start : start + bar_n] += 0.12 * _tone(f, bar_n, pad_h)
        for b in range(4):
            s = start + b * beat_n
            fb = 65.41 * 2 ** (root / 12)
            bass_track[s : s + beat_n] += 0.35 * _tone(fb, beat_n, [(1, 1.0), (2, 0.3)], decay=0.25)
            # Bombo en 1 y 3 (más fuerte en el 1), caja en 2 y 4.
            n = int(0.15 * SR)
            if b in (0, 2):
                amp = 0.9 if b == 0 else 0.6
                t = np.arange(n) / SR
                mix[s : s + n] += amp * np.sin(2 * np.pi * (50 + 80 * np.exp(-t / 0.03)) * t) * np.exp(-t / 0.08)
            else:
                mix[s : s + n] += 0.35 * rng.standard_normal(n) * np.exp(-np.arange(n) / (0.04 * SR))
            if not part_b:
                for h in (0, beat_n // 2):
                    m = int(0.03 * SR)
                    mix[s + h : s + h + m] += 0.08 * rng.standard_normal(m) * np.exp(-np.arange(m) / (0.008 * SR))
    mix += harm + bass_track
    peak = np.abs(mix).max()
    return (mix / peak * 0.8).astype(np.float32), (harm / peak * 0.8).astype(np.float32), (bass_track / peak * 0.8).astype(np.float32)


@pytest.fixture(scope="module")
def song():
    return synth_song()


def chord_accuracy(chords, form):
    hits = 0
    for i, name in enumerate(form):
        t = (i + 0.5) * BAR
        c = next((c for c in chords if c["start"] <= t < c["end"]), None)
        root, q = CHORDS[name]
        # Las extensiones de séptima cuentan como acierto si la tríada coincide.
        if c and c["root"] == root and (c["quality"] or "").startswith(q) and not (q == "" and (c["quality"] or "").startswith("m")):
            hits += 1
    return hits / len(form)


def test_librosa_beats_tempo_and_meter(song):
    from musicnova_ai.analysis import beats as bt

    mix, _, _ = song
    beats = bt.beats_librosa(mix)
    bpm, _, stable = bt.tempo_summary(beats)
    assert abs(bpm - BPM) / BPM < 0.02, bpm
    assert stable
    downbeats, bpb = bt.estimate_downbeats(mix, beats)
    assert bpb == 4
    # Los tiempos fuertes caen en múltiplos de un compás (±70 ms).
    phase = np.abs(((downbeats + BAR / 2) % BAR) - BAR / 2)
    assert np.median(phase) < 0.07, np.median(phase)


def test_chords_and_key_from_stems(song):
    from musicnova_ai.analysis import beats as bt
    from musicnova_ai.analysis import harmony

    mix, harm, bass = song
    beats = np.arange(0, len(mix) / SR - 0.01, BEAT)
    treble, bass_chroma, rms = harmony.chroma_features(harm + bass, bass)
    chords = harmony.recognize_chords(treble, bass_chroma, rms, beats, len(mix) / SR)
    acc = chord_accuracy(chords, FORM)
    assert acc >= 0.9, (acc, [c["label"] for c in chords][:20])
    key = harmony.estimate_key(treble, rms, chords)
    assert key["label"] == "C", key
    # Sin cambios espurios: aproximadamente un segmento por compás.
    assert len(chords) <= len(FORM) * 1.3


def test_chords_from_mix_without_stems(song):
    import librosa

    from musicnova_ai.analysis import harmony

    mix, _, _ = song
    beats = np.arange(0, len(mix) / SR - 0.01, BEAT)
    y_harm = librosa.effects.harmonic(mix, margin=2.0)
    treble, bass_chroma, rms = harmony.chroma_features(y_harm)
    chords = harmony.recognize_chords(treble, bass_chroma, rms, beats, len(mix) / SR)
    assert chord_accuracy(chords, FORM) >= 0.8


def test_sections_find_aba_form(song):
    from musicnova_ai.analysis import harmony, structure

    mix, harm, bass = song
    duration = len(mix) / SR
    beats = np.arange(0, duration - 0.01, BEAT)
    downbeats = beats[::4]
    treble, _, _ = harmony.chroma_features(harm + bass, bass)
    sections = structure.detect_sections(mix, treble, beats, downbeats, 4, duration)
    starts = [s["start"] for s in sections]
    for boundary in (32.0, 64.0):
        assert min(abs(s - boundary) for s in starts) <= 2.0, starts
    label_at = lambda t: next(s["label"] for s in sections if s["start"] <= t < s["end"])  # noqa: E731
    assert label_at(16) == label_at(80) != label_at(48), sections


@pytest.mark.parametrize("beat_model", ["none", "beat_this", "beat_this_small"])
def test_analyze_task_end_to_end(song, tmp_path, beat_model):
    import soundfile as sf

    from musicnova_ai.jobs import Job
    from musicnova_ai.models import ModelManager
    from musicnova_ai.tasks.analysis import BEAT_THIS_DEPS, AnalyzeTask, _missing

    mm = ModelManager()
    if beat_model != "none" and (_missing(BEAT_THIS_DEPS) or not mm.is_installed(beat_model)):
        pytest.skip(f"{beat_model} no está instalado")
    mix, harm, bass = song
    sf.write(tmp_path / "mix.wav", np.stack([mix, mix], axis=1), SR, subtype="FLOAT")
    sf.write(tmp_path / "other.flac", harm, SR)
    sf.write(tmp_path / "bass.flac", bass, SR)
    out = tmp_path / "analysis" / "analysis.json"
    events = []
    res = AnalyzeTask(mm).run(
        {
            "input": str(tmp_path / "mix.wav"),
            "stems": {"other": str(tmp_path / "other.flac"), "bass": str(tmp_path / "bass.flac")},
            "beat_model_id": beat_model,
            "chord_engine": "hmm",
            "output": str(out),
        },
        Job("t", "analyze", lambda e, d: events.append(d)),
    )
    data = json.loads(out.read_text(encoding="utf-8"))
    print(f"\n{beat_model}: {res} bpb={data['beats_per_bar']} secciones={[s['label'] for s in data['sections']]}")
    assert data["beat_source"] == ("librosa" if beat_model == "none" else beat_model)
    assert data["chord_source"] == "stems"
    assert abs(data["bpm"] - BPM) / BPM < 0.02
    assert data["beats_per_bar"] == 4
    assert data["key"]["label"] == "C"
    assert chord_accuracy(data["chords"], FORM) >= 0.85
    assert events[-1]["progress"] == 1.0


def test_seventh_chords_detected():
    """Las séptimas que sí suenan se reconocen (Cmaj7 Dm7 G7 C, un acorde por compás)."""
    from musicnova_ai.analysis import harmony

    voicings = [(0, (0, 4, 7, 11), "maj7"), (2, (0, 3, 7, 10), "m7"), (7, (0, 4, 7, 10), "7"), (0, (0, 4, 7), "")]
    bar_n = int(BAR * SR)
    harm = np.zeros(bar_n * 8)
    bass = np.zeros_like(harm)
    for i in range(8):
        root, notes, _ = voicings[i % 4]
        s = i * bar_n
        for iv in notes:
            harm[s : s + bar_n] += 0.1 * _tone(261.63 * 2 ** ((root + iv) / 12), bar_n, [(1, 1.0), (2, 0.4), (3, 0.2)])
        bass[s : s + bar_n] += 0.3 * _tone(65.41 * 2 ** (root / 12), bar_n, [(1, 1.0), (2, 0.3)])
    beats = np.arange(0, len(harm) / SR - 0.01, BEAT)
    treble, bass_chroma, rms = harmony.chroma_features((harm + bass).astype(np.float32), bass.astype(np.float32))
    chords = harmony.recognize_chords(treble, bass_chroma, rms, beats, len(harm) / SR)
    got = [next(c for c in chords if c["start"] <= (i + 0.5) * BAR < c["end"]) for i in range(8)]
    expected = [(v[0], v[2]) for v in voicings] * 2
    assert [(c["root"], c["quality"]) for c in got] == expected, [c["label"] for c in got]


def test_btc_decoding_follows_beats():
    """Probabilidades por trama → un acorde por pulso, con «X» sumada a «N»."""
    from musicnova_ai.analysis import harmony
    from musicnova_ai.vendor import btc

    fs = btc.FRAME_SECONDS
    n = int(8 / fs)
    probs = np.full((n, 170), 1e-4)
    t = (np.arange(n) + 0.5) * fs
    # 0–4 s: Do mayor (índice 0·14+1); 4–6 s: Si m7♭5 (11·14+11); 6–8 s: desconocido (X).
    probs[t < 4, 0 * 14 + 1] = 1.0
    probs[(t >= 4) & (t < 6), 11 * 14 + 11] = 1.0
    probs[t >= 6, btc.X_INDEX] = 1.0
    probs /= probs.sum(axis=1, keepdims=True)
    beats = np.arange(0.5, 8, 0.5)
    chords = harmony.recognize_chords_btc(probs, fs, beats, 8.0)
    assert [(c["label"], c["start"], c["end"]) for c in chords] == [("C", 0.0, 4.0), ("Bm7b5", 4.0, 6.0), ("N", 6.0, 8.0)]
    assert chords[1]["root"] == 11 and chords[1]["quality"] == "m7b5"


def test_analyze_with_btc(song, tmp_path):
    import soundfile as sf

    from musicnova_ai.jobs import Job
    from musicnova_ai.models import ModelManager
    from musicnova_ai.tasks.analysis import AnalyzeTask

    mm = ModelManager()
    if not mm.is_installed("btc_large_voca"):
        pytest.skip("BTC no está instalado")
    mix, _, _ = song
    sf.write(tmp_path / "mix.wav", np.stack([mix, mix], axis=1), SR, subtype="FLOAT")
    res = AnalyzeTask(mm).run({"input": str(tmp_path / "mix.wav"), "beat_model_id": "none", "chord_engine": "btc", "device": "cpu"}, Job("t", "analyze", lambda *a: None))
    assert res["chord_source"] == "btc"
    assert chord_accuracy(res["chords"], FORM) >= 0.85, res["chords"][:8]
    assert res["key"]["label"] == "C"
