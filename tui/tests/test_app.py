# Copyright (C) 2026 Ajax Dong
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     https://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
from pathlib import Path
import asyncio

import nvoc_tui.app as app_module
from nvoc_tui.app import NVOCApp
from nvoc_tui.models import CliLocation


def test_app_uses_textual_default_theme(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(app_module, "repo_root", lambda: tmp_path)
    monkeypatch.setattr(
        app_module.CliService, "discover_cli", lambda saved_path="": CliLocation()
    )

    app = NVOCApp()

    assert app.theme == "textual-dark"
    assert app.ansi_color is False
    assert app.animation_level == "none"


def test_app_css_path_points_to_split_styles(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(app_module, "repo_root", lambda: tmp_path)
    monkeypatch.setattr(
        app_module.CliService, "discover_cli", lambda saved_path="": CliLocation()
    )

    app = NVOCApp()

    assert list(app.CSS_PATH) == [
        "styles/base.tcss",
        "styles/header.tcss",
        "styles/dashboard.tcss",
        "styles/autoscan.tcss",
        "styles/overclock.tcss",
        "styles/vfcurve.tcss",
        "styles/console.tcss",
    ]
    for css_path in app.css_path:
        assert css_path.exists()


def test_app_hides_dashboard_alt_shortcuts_from_footer(
    monkeypatch, tmp_path: Path
) -> None:
    monkeypatch.setattr(app_module, "repo_root", lambda: tmp_path)
    monkeypatch.setattr(
        app_module.CliService, "discover_cli", lambda saved_path="": CliLocation()
    )

    app = NVOCApp()
    bindings = {
        binding.key: binding
        for binding in app.BINDINGS
        if hasattr(binding, "key")
    }

    for key in [
        "alt+a",
        "alt+p",
        "alt+n",
        "alt+i",
        "alt+s",
        "alt+g",
        "alt+c",
        "alt+q",
        "alt+r",
        "alt+e",
        "alt+v",
        "alt+l",
        "alt+u",
        "alt+m",
    ]:
        assert bindings[key].show is False


def test_app_split_layout_smoke(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(app_module, "repo_root", lambda: tmp_path)
    monkeypatch.setattr(
        app_module.CliService, "discover_cli", lambda saved_path="": CliLocation()
    )
    monkeypatch.setattr(
        app_module.CliService,
        "list_gpus",
        lambda self, cli: (-1, "CLI executable not configured.", []),
    )

    async def run() -> None:
        app = NVOCApp()
        async with app.run_test() as pilot:
            await pilot.pause()
            for selector in [
                "#gpu-select",
                "#main-tabs",
                "#dashboard",
                "#autoscan",
                "#overclock",
                "#vfcurve",
                "#metrics",
                "#vf-plot",
                "#output-log",
            ]:
                assert app.query_one(selector) is not None

    asyncio.run(run())


def test_function_key_bindings_switch_tabs(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(app_module, "repo_root", lambda: tmp_path)
    monkeypatch.setattr(
        app_module.CliService, "discover_cli", lambda saved_path="": CliLocation()
    )
    monkeypatch.setattr(
        app_module.CliService,
        "list_gpus",
        lambda self, cli: (-1, "CLI executable not configured.", []),
    )

    async def run() -> None:
        app = NVOCApp()
        async with app.run_test() as pilot:
            await pilot.press("f1")
            assert app.config_data.ui.active_tab == "dashboard"
            assert app.focused is app.query_one("#dashboard-interval")
            await pilot.press("f2")
            assert app.config_data.ui.active_tab == "autoscan"
            assert app.focused is app.query_one("#autoscan-mode")
            await pilot.press("f3")
            assert app.config_data.ui.active_tab == "overclock"
            assert app.focused is app.query_one("#oc-api")
            await pilot.press("f4")
            assert app.config_data.ui.active_tab == "vfcurve"
            assert app.focused is app.query_one("#vf-path")

    asyncio.run(run())


def test_global_focus_shortcuts(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(app_module, "repo_root", lambda: tmp_path)
    monkeypatch.setattr(
        app_module.CliService, "discover_cli", lambda saved_path="": CliLocation()
    )
    monkeypatch.setattr(
        app_module.CliService,
        "list_gpus",
        lambda self, cli: (-1, "CLI executable not configured.", []),
    )

    async def run() -> None:
        app = NVOCApp()
        async with app.run_test() as pilot:
            await pilot.press("ctrl+g")
            assert app.focused is app.query_one("#gpu-select")

            panel = app.query_one("#log-panel")
            panel.add_class("hidden")
            app.config_data.ui.log_expanded = False
            await pilot.press("ctrl+o")
            assert not panel.has_class("hidden")
            assert app.config_data.ui.log_expanded is True
            assert app.focused is app.query_one("#output-log")

    asyncio.run(run())


def test_dashboard_shortcuts_are_scoped_and_labels_are_underlined(
    monkeypatch, tmp_path: Path
) -> None:
    monkeypatch.setattr(app_module, "repo_root", lambda: tmp_path)
    monkeypatch.setattr(
        app_module.CliService, "discover_cli", lambda saved_path="": CliLocation()
    )
    monkeypatch.setattr(
        app_module.CliService,
        "list_gpus",
        lambda self, cli: (-1, "CLI executable not configured.", []),
    )

    async def run() -> None:
        app = NVOCApp()
        async with app.run_test() as pilot:
            for selector, plain in [
                ("#dashboard-interval-apply", "Apply"),
                ("#dashboard-pause", "Pause"),
                ("#dashboard-now", "Now"),
                ("#dashboard-info", "Info"),
                ("#dashboard-status", "Status"),
                ("#dashboard-get", "Get"),
            ]:
                label = app.query_one(selector).label
                assert label.plain == plain
                assert any(span.start == 0 and span.end == 1 for span in label.spans)

            calls: list[str] = []

            app.dashboard_controller.set_poll_timer = lambda value: calls.append(
                f"apply:{value}"
            )
            app.dashboard_controller.tick = lambda: calls.append("now")
            app.run_cli_action = lambda args: calls.append(" ".join(args))

            await pilot.press("f1")
            await pilot.press("alt+a")
            await pilot.press("alt+n")
            await pilot.press("alt+i")
            await pilot.press("alt+s")
            await pilot.press("alt+g")

            assert calls == [
                "apply:1.0",
                "now",
                "-O json info",
                "-O json status -a",
                "-O json get",
            ]

            await pilot.press("f2")
            await pilot.press("alt+n")
            assert calls == [
                "apply:1.0",
                "now",
                "-O json info",
                "-O json status -a",
                "-O json get",
            ]

    asyncio.run(run())
