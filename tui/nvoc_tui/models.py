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

import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


@dataclass(slots=True)
class GpuDescriptor:
    index: int
    name: str
    uuid: str | None = None
    gpu_id_hex: str | None = None

    @property
    def short_label(self) -> str:
        return f"GPU {self.index}: {self.name}"

    @property
    def long_label(self) -> str:
        if self.uuid:
            return f"{self.short_label} [{self.uuid}]"
        if self.gpu_id_hex:
            return f"{self.short_label} [{self.gpu_id_hex}]"
        return self.short_label


@dataclass(slots=True)
class DashboardSettings:
    refresh_interval: float = 1.0


@dataclass(slots=True)
class VFCurveSettings:
    default_path: str = ""
    auto_refresh: bool = False


@dataclass(slots=True)
class UiSettings:
    log_expanded: bool = True
    active_tab: str = "dashboard"


@dataclass(slots=True)
class AppConfig:
    last_gpu_idx: int | None = None
    dashboard: DashboardSettings = field(default_factory=DashboardSettings)
    vfcurve: VFCurveSettings = field(default_factory=VFCurveSettings)
    ui: UiSettings = field(default_factory=UiSettings)


@dataclass(slots=True)
class CurveData:
    """One plotted V/F curve (GPC public / XBAR / SYS private)."""

    curve_id: str
    source: str = "public"  # "public" | "private"
    write_mode: str = "public"  # "public" | "private"
    bank: int = 0
    seg_start: int = 0
    seg_end: int = 0
    voltages: list[float] = field(default_factory=list)  # mV
    frequencies: list[float] = field(default_factory=list)  # MHz (current)
    defaults: list[float] = field(default_factory=list)  # MHz (default)
    has_fixed: bool = False


@dataclass(slots=True)
class GpuCache:
    info: dict[str, Any] = field(default_factory=dict)
    status: dict[str, Any] = field(default_factory=dict)
    settings: dict[str, Any] = field(default_factory=dict)
    vf_curve_points: list[dict[str, Any]] | None = None
    # Multi-curve state (controller-owned; cache is the cross-pane view).
    vf_curves: dict[str, CurveData] | None = None
    curve_visible: dict[str, bool] = field(default_factory=dict)
    active_curve: str = "gpc"
    # Live crosshair for the active private curve (xbar/host), set by the
    # direct-read poll path (voltage mV, frequency MHz).
    vf_live_point: tuple[float, float] | None = None
    # Per-rail live voltages [(label, mV), ...] for the dashboard VOLT line,
    # refreshed by the rail poll piggybacked on the status sweep. None on
    # single-rail / volt-rails-unsupported parts (plain VOLT form then).
    rail_volts: list[tuple[str, float]] | None = None


@dataclass(slots=True)
class ActionState:
    running: bool = False
    description: str = ""


@dataclass(slots=True)
class OutputLine:
    text: str
    level: str = "info"


def repo_root() -> Path:
    if getattr(sys, "frozen", False):
        return Path(sys.executable).resolve().parent
    return Path(__file__).resolve().parent.parent
