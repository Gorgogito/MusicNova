"""Dependencias bajo demanda: elección de PyTorch y cobertura de las versiones fijadas."""

from musicnova_ai import deps
from musicnova_ai.tasks import REGISTRY, requires_for_model_task


def _with_driver(monkeypatch, driver):
    gpus = [] if driver is None else [{"name": "NVIDIA", "driver": driver, "vendor": "nvidia"}]
    monkeypatch.setattr(deps.system, "info", lambda: {"gpus": gpus})
    monkeypatch.delenv("MUSICNOVA_TORCH", raising=False)


def test_torch_variant_follows_the_nvidia_driver(monkeypatch):
    for driver, want in [("591.44", "cu130"), ("580.88", "cu130"), ("572.16", "cu128"), ("566.03", "cu126"), ("552.22", "cpu"), (None, "cpu")]:
        _with_driver(monkeypatch, driver)
        assert deps.torch_variant() == want, driver
    monkeypatch.setenv("MUSICNOVA_TORCH", "cpu")
    assert deps.torch_variant() == "cpu"


def test_every_task_dependency_has_a_pinned_requirement():
    for tid, cls in REGISTRY.items():
        for m in cls.requires:
            assert m in deps.REQUIREMENTS or m in deps.TORCH_MODULES, f"{tid}: {m} sin versión fijada"
    for r in deps.REQUIREMENTS.values():
        assert "==" in r, r
    # Las tareas que usan modelos declaran lo que necesitan.
    assert "torch" in requires_for_model_task("separation")
    assert "faster_whisper" in requires_for_model_task("lyrics")
    assert "torch" in requires_for_model_task("chords"), "BTC lo usa el análisis"


def test_nothing_is_installed_outside_the_managed_environment(monkeypatch):
    monkeypatch.setenv("MUSICNOVA_MANAGED_ENV", "0")
    called = []
    monkeypatch.setattr(deps, "_pip", lambda *a, **k: called.append(a))
    assert deps.ensure(["paquete_que_no_existe_xyz"], lambda p, m: None) is False
    assert called == []


def test_managed_install_puts_the_chosen_torch_first(monkeypatch):
    monkeypatch.setenv("MUSICNOVA_MANAGED_ENV", "1")
    _with_driver(monkeypatch, "580.1")
    calls = []
    monkeypatch.setattr(deps, "_pip", lambda args, report, label, s, e: calls.append((label, args)))
    state = {"done": False}
    monkeypatch.setattr(deps, "missing", lambda mods: [] if state["done"] else [m for m in mods if m in ("torch", "demucs")])

    def fake_pip(args, report, label, s, e):
        calls.append((label, args))
        if label == "instalando dependencias":
            state["done"] = True

    monkeypatch.setattr(deps, "_pip", fake_pip)
    assert deps.ensure(["torch", "demucs", "numpy"], lambda p, m: None) is True
    assert calls[0][0] == "instalando PyTorch (CUDA)"
    assert "https://download.pytorch.org/whl/cu130" in calls[0][1]
    assert "torch==2.11.0+cu130" in calls[0][1]
    assert "--constraint" in calls[1][1] and "demucs==4.0.1" in calls[1][1]
