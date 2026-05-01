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
from __future__ import annotations

import json
from dataclasses import asdict
from pathlib import Path
from typing import Any

from .models import AppConfig, AutoscanSettings, CliLocation, DashboardSettings, UiSettings, VFCurveSettings


TUI_CONFIG_FILE = "nvoc_tui_config.json"
GUI_CONFIG_FILE = "nvoc_gui_config.json"


class ConfigStore:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.path = root / TUI_CONFIG_FILE
        self.gui_path = root / GUI_CONFIG_FILE
        self.data = AppConfig()

    def load(self) -> AppConfig:
        if self.path.exists():
            raw = self._read_json(self.path)
            self.data = self._decode(raw)
            return self.data

        if self.gui_path.exists():
            gui_raw = self._read_json(self.gui_path)
            self.data = self._decode_from_gui(gui_raw)
            self.save()
            return self.data

        self.data = AppConfig()
        return self.data

    def save(self) -> None:
        payload = {
            "cli": asdict(self.data.cli),
            "last_gpu_idx": self.data.last_gpu_idx,
            "autoscan": self.data.autoscan.to_dict(),
            "dashboard": asdict(self.data.dashboard),
            "vfcurve": asdict(self.data.vfcurve),
            "ui": asdict(self.data.ui),
        }
        self.path.write_text(json.dumps(payload, indent=2, ensure_ascii=False), encoding="utf-8")

    @staticmethod
    def _read_json(path: Path) -> dict[str, Any]:
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except Exception:
            return {}

    def _decode(self, data: dict[str, Any]) -> AppConfig:
        cli = CliLocation(**{k: data.get("cli", {}).get(k, v) for k, v in asdict(CliLocation()).items()})
        dashboard = DashboardSettings(refresh_interval=float(data.get("dashboard", {}).get("refresh_interval", 1.0)))
        vfcurve = VFCurveSettings(
            default_path=str(data.get("vfcurve", {}).get("default_path", "")),
            quick_export=bool(data.get("vfcurve", {}).get("quick_export", True)),
            auto_refresh=bool(data.get("vfcurve", {}).get("auto_refresh", False)),
        )
        ui = UiSettings(
            log_expanded=bool(data.get("ui", {}).get("log_expanded", True)),
            active_tab=str(data.get("ui", {}).get("active_tab", "dashboard")),
        )
        last_gpu_idx = data.get("last_gpu_idx")
        if not isinstance(last_gpu_idx, int):
            last_gpu_idx = None
        return AppConfig(
            cli=cli,
            last_gpu_idx=last_gpu_idx,
            autoscan=AutoscanSettings.from_mapping(data.get("autoscan")),
            dashboard=dashboard,
            vfcurve=vfcurve,
            ui=ui,
        )

    def _decode_from_gui(self, data: dict[str, Any]) -> AppConfig:
        cli_path = str(data.get("cli_exe_path", ""))
        cli = CliLocation(exe_path=cli_path)
        last_gpu_idx_raw = data.get("last_gpu_idx")
        last_gpu_idx = int(last_gpu_idx_raw) if str(last_gpu_idx_raw).isdigit() else None
        return AppConfig(
            cli=cli,
            last_gpu_idx=last_gpu_idx,
            autoscan=AutoscanSettings.from_mapping(data.get("autoscan")),
            dashboard=DashboardSettings(refresh_interval=1.0),
            vfcurve=VFCurveSettings(default_path="", quick_export=True, auto_refresh=False),
            ui=UiSettings(log_expanded=True, active_tab="dashboard"),
        )
