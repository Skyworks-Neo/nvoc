from __future__ import annotations

from pathlib import Path


def test_production_gui_has_no_periodic_memory_sampler() -> None:
    gui_root = Path(__file__).resolve().parents[1]
    app_source = (gui_root / "src" / "app.py").read_text(encoding="utf-8")

    assert not (gui_root / "src" / "memory_debug.py").exists()
    assert "MemoryDebugSampler" not in app_source
    assert "NVOC_GUI_MEMORY_DEBUG" not in app_source
