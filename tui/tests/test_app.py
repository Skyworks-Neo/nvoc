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
