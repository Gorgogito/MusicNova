"""Separa la muestra de IRMAS con htdemucs y guarda las probabilidades de PANNs por stem."""
import os, pickle, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DATA  # noqa: E402
import numpy as np, torch, soundfile as sf
import musicnova_ai.tasks.transcription as T
from musicnova_ai.jobs import Job
from musicnova_ai.models import ModelManager
from musicnova_ai.tasks.separation import SeparateTask
from musicnova_ai.vendor import panns
ROOT = os.path.join(DATA, "irmas")
arch = sys.argv[1] if len(sys.argv) > 1 else "cnn14"
sep = SeparateTask(ModelManager())
md = os.path.join(os.environ["LOCALAPPDATA"], "MusicNova", "models", f"panns_{arch}")
model = panns.load("Cnn14" if arch == "cnn14" else "Cnn10", os.path.join(md, [f for f in os.listdir(md) if f.endswith(".pth")][0]), "cuda")
def probs(y):
    win, hop = int(T.INSTRUMENT_WINDOW * T.PANNS_SR), int(T.INSTRUMENT_HOP * T.PANNS_SR)
    starts = list(range(0, max(1, len(y) - win + hop), hop))
    chunk = [np.pad(y[s:s+win], (0, max(0, win - len(y[s:s+win])))) for s in starts]
    with torch.inference_mode():
        return model(torch.from_numpy(np.stack(chunk).astype(np.float32)).cuda()).float().cpu().numpy()
out = []
for txt in open(os.path.join(ROOT, "sample.txt"), encoding="utf-8").read().split("\n"):
    wav = os.path.join(ROOT, txt[:-4] + ".wav")
    if not os.path.exists(wav): continue
    labels = [l.strip() for l in open(os.path.join(ROOT, txt), encoding="utf-8", errors="replace") if l.strip()]
    d = os.path.join(ROOT, "sep", os.path.basename(txt)[:-4])
    if not os.path.exists(os.path.join(d, "other.flac")):
        sep.run({"model_id": "htdemucs", "input": wav, "output_dir": d, "device": "cuda"}, Job("s", "s", lambda *a, **k: None))
    stems = {s: probs(T._load_mono(os.path.join(d, s + ".flac"), T.PANNS_SR)) for s in ("vocals", "drums", "bass", "other")}
    mix = T._load_mono(wav, T.PANNS_SR)
    out.append((txt, labels, len(mix) / T.PANNS_SR, probs(mix), stems))
pickle.dump(out, open(os.path.join(ROOT, f"stems_{arch}.pkl"), "wb"))
print("ok", len(out))
