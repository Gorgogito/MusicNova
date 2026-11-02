"""Carpeta de los datos de evaluación: MN_EVAL_DATA o `eval-data/` junto a ai-worker."""
import os
import sys

DATA = os.environ.get("MN_EVAL_DATA") or os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "eval-data")
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
try:
    sys.stdout.reconfigure(encoding="utf-8")
except AttributeError:
    pass
