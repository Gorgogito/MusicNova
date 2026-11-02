"""Descarga los datos de evaluación en MN_EVAL_DATA (o ai-worker/eval-data/).

Ninguno se redistribuye con MusicNova: solo se usan para medir.
- Vocadito (CC BY 4.0): 40 voces solistas con f0 y notas anotadas. 58 MB.
- MDB Drums (CC BY-NC-SA 4.0): 23 canciones de MedleyDB con batería sola,
  mezcla y golpes anotados. 346 MB.
- IRMAS, partición de prueba (CC BY-NC-SA 3.0): se bajan las etiquetas y una
  muestra de ~160 fragmentos (hasta 30 por instrumento) con peticiones por
  rangos, sin descargar los 7,8 GB de los zip.

uso: python fetch.py [vocadito] [mdb] [irmas]
"""
import collections
import io
import json
import os
import random
import sys
import urllib.parse
import urllib.request
import zipfile

from common import DATA

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import remotezip  # noqa: E402

IRMAS = "https://zenodo.org/records/1290750/files/IRMAS-TestingData-Part{}.zip?download=1"
IRMAS_LABELS = {"cel", "cla", "flu", "gac", "gel", "org", "pia", "sax", "tru", "vio", "voi"}


def vocadito():
    dest = os.path.join(DATA, "vocadito")
    if os.path.exists(os.path.join(dest, "x", "Audio")):
        return
    os.makedirs(dest, exist_ok=True)
    data = urllib.request.urlopen("https://zenodo.org/records/5578807/files/vocadito.zip?download=1").read()
    zipfile.ZipFile(io.BytesIO(data)).extractall(os.path.join(dest, "x"))


def mdb():
    dest = os.path.join(DATA, "mdb")
    os.makedirs(dest, exist_ok=True)
    tree = json.load(urllib.request.urlopen("https://api.github.com/repos/CarlSouthall/MDBDrums/git/trees/master?recursive=1"))
    wanted = ("MDB Drums/audio/drum_only/", "MDB Drums/audio/full_mix/", "MDB Drums/annotations/class/")
    for item in tree["tree"]:
        path = item["path"]
        if item["type"] != "blob" or not path.startswith(wanted):
            continue
        out = os.path.join(dest, path.split("/", 1)[1].replace("/", "__"))
        if os.path.exists(out):
            continue
        url = "https://raw.githubusercontent.com/CarlSouthall/MDBDrums/master/" + urllib.parse.quote(path)
        with urllib.request.urlopen(url) as r, open(out + ".part", "wb") as f:
            f.write(r.read())
        os.replace(out + ".part", out)
        print("ok", path, flush=True)


def irmas():
    dest = os.path.join(DATA, "irmas")
    os.makedirs(dest, exist_ok=True)
    labels = {}
    for part in "123":
        z = zipfile.ZipFile(io.BufferedReader(remotezip.RangeFile(IRMAS.format(part)), buffer_size=1 << 14))
        txts = [i.filename for i in z.infolist() if i.filename.endswith(".txt")]
        for name in txts:
            out = os.path.join(dest, name)
            if not os.path.exists(out):
                os.makedirs(os.path.dirname(out), exist_ok=True)
                with open(out, "wb") as f:
                    f.write(z.read(name))
            ls = [l.strip() for l in open(out, encoding="utf-8", errors="replace") if l.strip()]
            if ls and all(l in IRMAS_LABELS for l in ls):
                labels[name] = (part, ls)
    random.seed(1)
    by = collections.defaultdict(list)
    for name, (_, ls) in labels.items():
        for l in ls:
            by[l].append(name)
    pick = set()
    for l in sorted(by, key=lambda k: len(by[k])):
        cands = sorted(by[l])
        random.shuffle(cands)
        n = sum(1 for f in pick if l in labels[f][1])
        for f in cands:
            if n >= 30:
                break
            if f not in pick:
                pick.add(f)
                n += 1
    with open(os.path.join(dest, "sample.txt"), "w", encoding="utf-8") as f:
        f.write("\n".join(sorted(pick)))
    for part in "123":
        z = zipfile.ZipFile(io.BufferedReader(remotezip.RangeFile(IRMAS.format(part)), buffer_size=1 << 14))
        for name in sorted(f for f in pick if labels[f][0] == part):
            wav = name[:-4] + ".wav"
            out = os.path.join(dest, wav)
            if not os.path.exists(out):
                with open(out + ".part", "wb") as f:
                    f.write(z.read(wav))
                os.replace(out + ".part", out)
                print("ok", wav, flush=True)


if __name__ == "__main__":
    which = sys.argv[1:] or ["vocadito", "mdb", "irmas"]
    for w in which:
        {"vocadito": vocadito, "mdb": mdb, "irmas": irmas}[w]()
