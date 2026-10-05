"""Real-Tk construction smoke tests for the VF Curve stressor panel.

``test_stressor_args`` covers the pure argv/probe helpers; these tests build
the actual panel with real widgets against a stub app so name-resolution and
wiring bugs (the class of failure that killed the old ``_make_slider_row``)
die here instead of on the user's screen. Tk-in-pytest needs a display;
skips cleanly when there is none.
"""

from __future__ import annotations

import pytest

try:
    import tkinter as tk

    tk.Tk()
    tk_available = True
except Exception:
    tk_available = False

pytestmark = pytest.mark.skipif(not tk_available, reason="no display for real Tk")

from src.tabs.vfcurve.sections import stressor as stressor_module  # noqa: E402
from src.tabs.vfcurve.sections.stressor import StressorPanel  # noqa: E402

MAIN_CAPS = {
    "--help",
    "--config",
    "--list-gpus",
    "--gpu-index",
    "--profile",
    "--duration",
    "--matrix-sizes",
    "--precisions",
    "--kernel-types",
    "--stream-mode",
    "--validate-interval",
    "--validate-size",
    "--seed",
    "--gpu-generate",
    "--enable-vulkan-stress",
    "--vulkan-only",
    "--vulkan-image-width",
    "--vulkan-image-height",
    "--vulkan-image-msaa",
}

ENHANCED_CAPS = MAIN_CAPS - {
    "--enable-vulkan-stress",
    "--vulkan-image-width",
    "--vulkan-image-height",
    "--vulkan-image-msaa",
} | {
    "--vulkan",
    "--vulkan-width",
    "--vulkan-height",
    "--vulkan-msaa",
    "--vulkan-iters",
    "--vulkan-shells",
    "--vulkan-window",
    "--vulkan-rotate",
    "--vulkan-particles",
    "--vulkan-heavy-offscreen",
    "--no-verify",
    "--verify-continue-on-error",
    "--verify-resident-interval",
    "--verify-slab",
    "--gemm-full-check-max-size",
    "--skip-self-test",
    "--json-out",
    "--cuda-path",
}


class _FakeConfig:
    def __init__(self) -> None:
        self.data = {"cli_exe_path": "", "stressor": {}}

    def get(self, key, default=None):
        return self.data.get(key, default)

    def set(self, key, value) -> None:
        self.data[key] = value


class _FakeConsole:
    def __init__(self) -> None:
        self.lines: list[str] = []

    def append(self, text: str) -> None:
        self.lines.append(text)

    def open(self) -> None:
        return

    def toggle(self) -> None:
        return


class _FakeRunner:
    is_running = False


class _App:
    """Minimal app surface the stressor panel touches."""

    def __init__(self, root: tk.Misc, gpu_label: str = "GPU 0: Test GPU") -> None:
        self.config = _FakeConfig()
        self.console = _FakeConsole()
        self.runner = _FakeRunner()
        self.cli_cwd = None
        self.gpu_var = tk.StringVar(master=root, value=gpu_label)
        self.gpu_names = {0: "Test GPU"}
        self.gpu_map = {gpu_label: 0}
        self.calls: list[tuple[list[str], str | None]] = []
        self.last_on_output = None
        self.last_on_finished = None
        self.resize_targets: list[object] = []
        self.background: list[str] = []

    def get_current_gpu_index(self):
        return 0

    def run_background(self, name, task):
        self.background.append(name)
        return task()

    def after(self, _ms, func=None, *args):
        return func(*args) if func is not None else None

    def run_cli(self, args, on_finished=None, exe=None, on_output=None):
        self.calls.append((list(args), exe))
        self.last_on_finished = on_finished
        self.last_on_output = on_output

    def run_cli_display(self, args, on_finished=None, exe=None, on_output=None):
        self.calls.append((list(args), exe))

    def cancel_cli(self) -> None:
        return

    def register_resize_target(self, target) -> None:
        self.resize_targets.append(target)


@pytest.fixture()
def panel(monkeypatch):
    root = tk.Tk()
    root.withdraw()
    app = _App(root)
    monkeypatch.setattr(stressor_module, "probe_stressor_flags", lambda _exe: MAIN_CAPS)
    frame = tk.Frame(root, bg="#2b2b2b")
    built = StressorPanel(frame, app)
    yield root, app, built
    root.destroy()


def _gated_widgets(panel_obj):
    return {key: widget for key, widget, _label in panel_obj._gated}


def test_panel_builds_and_registers_for_resize(panel):
    root, app, built = panel
    root.update_idletasks()

    assert built in app.resize_targets
    assert built.frame is not None
    assert "0" in built._gpu_lbl.cget("text")


def test_capability_gating_follows_the_probe(panel):
    _root, _app, built = panel
    gated = _gated_widgets(built)

    built._apply_capabilities(MAIN_CAPS)
    assert gated["no_verify"].cget("state") == "disabled"
    assert gated["verify_continue_on_error"].cget("state") == "disabled"
    assert gated["json_out"].cget("state") == "disabled"
    assert gated["cuda_path"].cget("state") == "disabled"
    # Main-lineage builds still advertise the legacy Vulkan family.
    assert gated["vulkan"].cget("state") == "normal"
    assert gated["vulkan_width"].cget("state") == "normal"

    built._apply_capabilities(ENHANCED_CAPS)
    assert gated["no_verify"].cget("state") == "normal"
    assert gated["json_out"].cget("state") == "normal"
    assert gated["cuda_path"].cget("state") == "normal"
    assert gated["vulkan"].cget("state") == "normal"


def test_probe_populates_capabilities_and_status(panel, monkeypatch):
    _root, _app, built = panel
    built._exe_var.set(__file__)
    monkeypatch.setattr(
        stressor_module, "probe_stressor_flags", lambda _exe: ENHANCED_CAPS
    )

    built._schedule_probe()

    assert built._capabilities == ENHANCED_CAPS
    assert "verify/SDC available" in built._probe_lbl.cget("text")


def test_start_run_composes_args_and_persists(panel):
    _root, app, built = panel
    built._exe_var.set(__file__)
    built._apply_capabilities(MAIN_CAPS)
    built._vars["duration"].set("30")
    built._vars["vulkan"].set(True)

    built._start_run()

    args, exe = app.calls[-1]
    assert exe == __file__
    assert args[:2] == ["--gpu-index", "0"]
    assert args[args.index("--duration") + 1] == "30"
    # Main-lineage executable: the legacy Vulkan spelling is chosen.
    assert "--enable-vulkan-stress" in args
    assert "--vulkan" not in args
    assert app.config.data["stressor"]["duration"] == "30"
    assert built.start_btn.cget("state") == "disabled"
    assert built.stop_btn.cget("state") == "normal"


def test_enhanced_build_uses_renamed_vulkan_flag(panel):
    _root, app, built = panel
    built._exe_var.set(__file__)
    built._apply_capabilities(ENHANCED_CAPS)
    built._vars["vulkan"].set(True)
    built._vars["verify_continue_on_error"].set(True)
    built._vars["cuda_path"].set(r"D:\cuda\runtime")

    built._start_run()

    args, _exe = app.calls[-1]
    assert "--vulkan" in args
    assert "--enable-vulkan-stress" not in args
    assert "--verify-continue-on-error" in args
    assert ["--cuda-path", r"D:\cuda\runtime"] == args[args.index("--cuda-path") :][:2]
    # Value-taking Vulkan knobs: the rotate checkbox forwards its state, the
    # particle pool stays on the "(default)" sentinel until the user types.
    assert ["--vulkan-rotate", "true"] == args[args.index("--vulkan-rotate") :][:2]
    assert "--vulkan-particles" not in args


def test_verdict_line_and_exit_land_in_the_panel(panel):
    _root, app, built = panel
    built._exe_var.set(__file__)
    built._start_run()

    app.last_on_output(
        'VERDICT_JSON: {"result": "fail", "classification": "sdc", '
        '"confidence_pct": 87, "device": "Test GPU"}\n'
    )
    assert built._result_lbl.cget("text") == (
        "verdict: fail classification=sdc confidence=87% device=Test GPU"
    )

    app.last_on_finished(1)
    assert "failed (exit 1)" in built._status_lbl.cget("text")
    assert built.start_btn.cget("state") == "normal"
    assert built.stop_btn.cget("state") == "disabled"


def test_stop_run_resets_buttons(panel):
    _root, _app, built = panel
    built._exe_var.set(__file__)
    built._start_run()

    built._stop_run()

    assert built.start_btn.cget("state") == "normal"
    assert built.stop_btn.cget("state") == "disabled"


def test_start_run_without_executable_warns(panel):
    _root, app, built = panel
    built._exe_var.set("")

    built._start_run()

    assert app.calls == []
    assert any("not found" in line for line in app.console.lines)
