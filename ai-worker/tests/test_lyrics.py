"""Pruebas de letras: reparto en líneas y transcripción real con voz sintetizada.

La prueba de extremo a extremo genera voz con el sintetizador de Windows (SAPI)
y la transcribe con el modelo Whisper instalado más ligero.
"""

import difflib
import importlib.util
import json
import re
import shutil
import subprocess
import sys

import pytest

from musicnova_ai.tasks.lyrics import Word, split_lines


def words_at(text: str, start=0.0, step=0.4, gaps: dict[int, float] | None = None) -> list[Word]:
    out, t = [], start
    for i, w in enumerate(text.split()):
        t += (gaps or {}).get(i, 0.0)
        out.append(Word(t, t + step * 0.8, " " + w))
        t += step
    return out


def test_split_on_long_pause():
    words = words_at("hello there my friend how are you", gaps={4: 1.5})
    lines = split_lines(words)
    assert [" ".join(w.text.strip() for w in line) for line in lines] == ["hello there my friend", "how are you"]


def test_split_long_line_at_punctuation():
    words = words_at("I walked down the road, and then I saw the light shining over the town tonight")
    lines = split_lines(words)
    assert len(lines) == 2
    assert lines[0][-1].text.strip() == "road,"
    assert all(len(line) <= 12 for line in lines)


def test_short_line_untouched_and_order_kept():
    words = words_at("la la la")
    assert split_lines(words) == [words]
    assert split_lines([]) == []


# ---------------------------------------------------------------- extremo a extremo

SENTENCES = {
    "en": ("Microsoft Zira Desktop", ["The big wheel keeps on turning tonight.", "Come and take a ride with me."]),
    "es": ("Microsoft Helena Desktop", ["La rueda grande sigue girando esta noche.", "Ven conmigo a dar un paseo."]),
}


def synth(tmp_path, voice: str, sentences: list[str]):
    """Genera un WAV con SAPI: frase, 2 s de silencio, frase."""
    wav = tmp_path / "voz.wav"
    parts = "; ".join(f"$p.AppendText('{s}'); $p.AppendBreak([TimeSpan]::FromSeconds(2))" for s in sentences)
    script = (
        "Add-Type -AssemblyName System.Speech; "
        "$s = New-Object System.Speech.Synthesis.SpeechSynthesizer; "
        f"$s.SelectVoice('{voice}'); "
        "$p = New-Object System.Speech.Synthesis.PromptBuilder; "
        f"$p.AppendBreak([TimeSpan]::FromSeconds(1)); {parts}; "
        f"$s.SetOutputToWaveFile('{wav}'); $s.Speak($p); $s.Dispose()"
    )
    subprocess.run(["powershell.exe", "-NoProfile", "-Command", script], check=True, timeout=60)
    return wav


def norm(text: str) -> str:
    return " ".join(re.sub(r"[^\w\s]", "", text.lower()).split())


@pytest.mark.skipif(sys.platform != "win32" or shutil.which("powershell.exe") is None, reason="requiere SAPI de Windows")
@pytest.mark.skipif(importlib.util.find_spec("faster_whisper") is None, reason="faster-whisper no instalado")
# Inglés con idioma explícito; español con detección automática. (La voz sintética
# inglesa de SAPI confunde la detección de Whisper: ~50 % «es»; con voz real no.)
@pytest.mark.parametrize("lang,explicit", [("en", True), ("es", False)])
def test_transcribe_synthetic_voice(tmp_path, lang, explicit):
    import soundfile as sf

    from musicnova_ai.jobs import Job
    from musicnova_ai.models import ModelManager
    from musicnova_ai.tasks.lyrics import TranscribeTask

    mm = ModelManager()
    model = next((m for m in ("whisper_small", "whisper_large_v3_turbo") if mm.is_installed(m)), None)
    if model is None:
        pytest.skip("no hay ningún modelo Whisper instalado")
    voice, sentences = SENTENCES[lang]
    wav = synth(tmp_path, voice, sentences)
    duration = sf.info(str(wav)).duration
    out = tmp_path / "lyrics.json"
    events = []
    res = TranscribeTask(mm).run(
        {"input": str(wav), "model_id": model, "output": str(out), "source": "vocals", "language": lang if explicit else "auto"},
        Job("t", "transcribe", lambda e, d: events.append(d)),
    )
    data = json.loads(out.read_text(encoding="utf-8"))
    text = " ".join(line["text"] for line in data["lines"])
    print(f"\n{lang} {model} {res} -> {text!r}")
    assert data["language"] == lang
    expected = " ".join(sentences)
    ratio = difflib.SequenceMatcher(None, norm(text), norm(expected)).ratio()
    assert ratio > 0.85, (ratio, text)
    # La pausa de 2 s separa las dos frases en líneas distintas.
    assert len(data["lines"]) == 2
    words = [w for line in data["lines"] for w in line["words"]]
    starts = [w["start"] for w in words]
    assert starts == sorted(starts)
    assert all(0 <= w["start"] <= w["end"] <= duration + 0.5 for w in words)
    # La segunda frase empieza después de la pausa (1 s inicial + frase 1 + 2 s).
    assert data["lines"][1]["start"] - data["lines"][0]["end"] > 1.0
    assert events[-1]["progress"] == 1.0


def test_capitalized_word_starts_line_and_orphans_rejoin():
    # Segmentos de Whisper desalineados: "...ride with me," | "girl On the tunnel of love"
    words = words_at("Come on and take a little ride with me, girl On the tunnel of love Yeah I said")
    lines = [" ".join(w.text.strip() for w in line) for line in split_lines(words)]
    assert lines == ["Come on and take a little ride with me, girl", "On the tunnel of love", "Yeah I said"]


def test_dangling_word_moves_to_next_line():
    # "She" quedó al final de la frase anterior, 2 s antes de "said, baby".
    words = words_at("you are the perfect stranger She said baby let's keep it", gaps={6: 2.0})
    lines = split_lines(words)
    assert [" ".join(w.text.strip() for w in line) for line in lines] == [
        "you are the perfect stranger",
        "She said baby let's keep it",
    ]
    she, said = lines[1][0], lines[1][1]
    assert 0 < said.start - she.end <= 0.1, "se recoloca justo antes del verso"
