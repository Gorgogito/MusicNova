"""Separa con htdemucs las mezclas de MDB Drums (canciones de prueba) y guarda el stem de batería."""
import glob, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from common import DATA  # noqa: E402
import soundfile as sf
from musicnova_ai.jobs import Job
from musicnova_ai.models import ModelManager
from musicnova_ai.tasks.separation import SeparateTask
ROOT = os.path.join(DATA, "mdb")
task = SeparateTask(ModelManager())
os.makedirs(os.path.join(ROOT, "sep"), exist_ok=True)
names = sorted(os.path.basename(f)[len("annotations__class__"):-len("_class.txt")] for f in glob.glob(os.path.join(ROOT, "annotations__class__*_class.txt")))
for name in names[1::2]:
    out = os.path.join(ROOT, "sep", name + "_drums.wav")
    if os.path.exists(out): continue
    tmp = os.path.join(ROOT, "sep", name)
    task.run({"model_id": "htdemucs", "input": os.path.join(ROOT, f"audio__full_mix__{name}_MIX.wav"), "output_dir": tmp, "device": "cuda"}, Job("s", "s", lambda *a, **k: None))
    y, sr = sf.read(os.path.join(tmp, "drums.flac"))
    sf.write(out, y, sr)
    print("ok", name, flush=True)
