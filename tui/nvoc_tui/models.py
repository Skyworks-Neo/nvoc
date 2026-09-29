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
    # Generation discriminator from the discover payload (core
    # detect_gpu_type). Pascal needs a curve-source downgrade — its private
    # ClockClient frequency terms read with a residual scale error, so the
    # public read stays the sole GPC authority there.
    arch: str | None = None
    is_legacy_voltage: bool | None = None

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
class NativeSettings:
    """Native library path overrides (NVOC_NVAPI_PATH / NVOC_NVML_PATH).

    命令行参数优先于这里的持久化配置；两者都必须发生在首次 GPU 调用前。
    """

    nvapi_path: str = ""
    nvml_path: str = ""


@dataclass(slots=True)
class AppConfig:
    last_gpu_idx: int | None = None
    dashboard: DashboardSettings = field(default_factory=DashboardSettings)
    vfcurve: VFCurveSettings = field(default_factory=VFCurveSettings)
    ui: UiSettings = field(default_factory=UiSettings)
    native: NativeSettings = field(default_factory=NativeSettings)


@dataclass(slots=True)
class EffectiveCurve:
    """Forward-synthesized effective series for a curve (display only).

    Composed from the readable offset planes instead of inverting the
    (underdetermined) observation: the private base curve + the ClkDomains
    slot-1 µV voltage addend (voltage-axis shift) + the slot-0 kHz
    frequency offset. Display-only: never written back into
    CurveData.frequencies — the editing paths operate on the base axes.
    """

    curve_id: str
    voltages: list[float] = field(default_factory=list)  # mV, shifted grid
    freqs: list[float] = field(default_factory=list)  # MHz, offset-added
    offset_mv: float = 0.0
    offset_mhz: float = 0.0
    applicable: bool = False


@dataclass(slots=True)
class CurveData:
    """One plotted V/F curve (GPC public / XBAR / MSD private)."""

    curve_id: str
    source: str = "public"  # "public" | "private" | "hybrid"
    write_mode: str = "public"  # "public" | "private"
    bank: int = 0
    seg_start: int = 0
    seg_end: int = 0
    voltages: list[float] = field(default_factory=list)  # mV
    frequencies: list[float] = field(default_factory=list)  # MHz (current)
    defaults: list[float] = field(default_factory=list)  # MHz (default)
    has_fixed: bool = False
    # Per-point public editability, index-aligned with ``voltages``: True =
    # the open VFP interface moves that point, False = it reads Fixed on the
    # public table and only the private one can. None = no trustworthy public
    # read (private-only segment, corrupt read), in which case the curve-level
    # ``write_mode`` decides as it always did. ``write_mode`` is the AGGREGATE
    # of this list — "public" when any point is publicly writable. Only
    # parsing.route_for_index may be used for a per-point decision.
    public_writable: list[bool] | None = None
    # Synthesized effective series (positive-slot1 display), see
    # parsing.synthesize_effective. None when no offset data was read.
    effective: EffectiveCurve | None = None

    def mark_public_read(self, public_writable: list[bool] | None) -> None:
        """Adopt the public table's per-point editability for this curve.

        ``None`` (or an empty read — no points, nothing to decide) = no
        trustworthy public read: the curve keeps its curve-level ``write_mode``
        and every point routes the old way.
        """
        if not public_writable:
            self.public_writable = None
            return
        self.public_writable = list(public_writable)
        self.has_fixed = any(not w for w in self.public_writable)
        self.write_mode = "public" if any(self.public_writable) else "private"

    def public_read_note(self) -> str:
        """Log suffix naming the per-point class mix, "" when unknown."""
        writable = self.public_writable
        if not writable:
            return ""
        fixed = sum(1 for w in writable if not w)
        return f", {fixed}/{len(writable)} Fixed" if fixed else ""


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
    # ClkDomains controllable mask from query_private_freq_domain_info
    # (refined by generation: Pascal MSD greyed regardless). None until the
    # overclock tab's first mask poll lands. Drives the Sys/Msd/Host row
    # enabled state on the Overclock pane.
    clk_domain_mask: int | None = None


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
