from __future__ import annotations

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
        return self.short_label


@dataclass(slots=True)
class CliLocation:
    exe_path: str = ""
    cwd: str | None = None


@dataclass(slots=True)
class AutoscanSettings:
    mode: str = "standard"
    test_exe: str = r".\test\test.bat"
    score_threshold: str = "98.5"
    timeout_loops: str = "30"
    output_csv: str = r".\ws\vfp-tem.csv"
    init_csv: str = r".\ws\vfp-init.csv"
    log_file: str = r".\ws\vfp.log"
    score_path: str = r"..\yiji_tb\result.xml"
    bsod_recovery: str = ""

    @classmethod
    def from_mapping(cls, data: dict[str, Any] | None) -> "AutoscanSettings":
        merged = cls()
        if data:
            for key in cls.__dataclass_fields__:
                if key in data:
                    setattr(merged, key, str(data[key]))
        return merged

    def to_dict(self) -> dict[str, str]:
        return {
            "mode": self.mode,
            "test_exe": self.test_exe,
            "score_threshold": self.score_threshold,
            "timeout_loops": self.timeout_loops,
            "output_csv": self.output_csv,
            "init_csv": self.init_csv,
            "log_file": self.log_file,
            "score_path": self.score_path,
            "bsod_recovery": self.bsod_recovery,
        }


@dataclass(slots=True)
class DashboardSettings:
    refresh_interval: float = 1.0


@dataclass(slots=True)
class VFCurveSettings:
    default_path: str = ""
    quick_export: bool = True
    auto_refresh: bool = False


@dataclass(slots=True)
class UiSettings:
    log_expanded: bool = True
    active_tab: str = "dashboard"


@dataclass(slots=True)
class AppConfig:
    cli: CliLocation = field(default_factory=CliLocation)
    last_gpu_idx: int | None = None
    autoscan: AutoscanSettings = field(default_factory=AutoscanSettings)
    dashboard: DashboardSettings = field(default_factory=DashboardSettings)
    vfcurve: VFCurveSettings = field(default_factory=VFCurveSettings)
    ui: UiSettings = field(default_factory=UiSettings)


@dataclass(slots=True)
class GpuCache:
    info: dict[str, Any] = field(default_factory=dict)
    status: dict[str, Any] = field(default_factory=dict)
    settings: dict[str, Any] = field(default_factory=dict)
    vf_curve_path: str = ""


@dataclass(slots=True)
class ActionState:
    running: bool = False
    description: str = ""


@dataclass(slots=True)
class OutputLine:
    text: str
    level: str = "info"


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent
