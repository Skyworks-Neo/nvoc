from __future__ import annotations

import sys
from pathlib import Path

import pytest

GUI_ROOT = Path(__file__).resolve().parents[1]
if str(GUI_ROOT) not in sys.path:
    sys.path.insert(0, str(GUI_ROOT))


@pytest.fixture(autouse=True)
def _close_matplotlib_figures():
    """Chart tests build real Agg figures through pyplot and never revisit
    them; without an explicit close they accumulate across the session until
    matplotlib's 20-open-figures RuntimeWarning fires on every later test."""
    yield
    import matplotlib.pyplot as plt

    plt.close("all")
