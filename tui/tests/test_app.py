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

import nvoc_tui.app as app_module
from nvoc_tui.app import NVOCApp
from nvoc_tui.models import CliLocation


def test_app_defaults_to_terminal_theme(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setattr(app_module, "repo_root", lambda: tmp_path)
    monkeypatch.setattr(
        app_module.CliService, "discover_cli", lambda saved_path="": CliLocation()
    )

    app = NVOCApp()

    assert app.theme == "textual-ansi"
    assert app.ansi_color is True
    assert app.get_css_variables()["background"] == "ansi_default"
    assert app.get_css_variables()["surface"] == "ansi_default"
