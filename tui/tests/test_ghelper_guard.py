from __future__ import annotations

import json
import os
import stat
import sys
from pathlib import Path

from nvoc_tui import ghelper_guard
from nvoc_tui.ghelper_guard import (
    active_mode_summary,
    find_oc_keys,
    run_guard,
    strip_oc_keys,
)

# Mirrors a real G-Helper config: GPU-side writers are stored per performance
# mode ("<key>_<mode>"), everything else is UI state or another device plane.
SAMPLE_CONFIG = {
    "performance_mode": 1,
    "mode_name_3": "Custom",
    "gpu_core_0": -50,
    "gpu_core_1": 200,
    "gpu_memory_1": "1500",
    "gpu_power_1": 115,
    "gpu_temp_2": 87,
    "gpu_boost_1": 15,
    "gpu_clock_limit_1": 2100,
    "fan_profile_gpu_1": "1E-3F-44-48",
    "gpu_mode": 1,
    "max_gpu_core": 400,
    "gpu_core_20": 999,
    "gpu_core": 5,
    "cpu_uv_1": "-80.5",
    "language": "zh-CN",
}

OC_KEYS_IN_SAMPLE = {
    "gpu_core_0": -50,
    "gpu_core_1": 200,
    "gpu_memory_1": "1500",
    "gpu_power_1": 115,
    "gpu_temp_2": 87,
    "gpu_boost_1": 15,
    "gpu_clock_limit_1": 2100,
}


def _write_config(path: Path, payload: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload), encoding="utf-8")


def test_find_oc_keys_matches_per_mode_gpu_writers_only() -> None:
    assert find_oc_keys(SAMPLE_CONFIG) == OC_KEYS_IN_SAMPLE


def test_strip_removes_oc_keys_keeps_rest_and_writes_backup(tmp_path: Path) -> None:
    config = tmp_path / "GHelper" / "config.json"
    backup = tmp_path / "GHelper" / "config.json.bak.nvoc"
    _write_config(config, SAMPLE_CONFIG)

    report = strip_oc_keys(config)

    assert report.status == "removed"
    assert report.backup_path == str(backup)
    assert report.removed == OC_KEYS_IN_SAMPLE
    assert backup.is_file()
    assert json.loads(backup.read_text(encoding="utf-8")) == SAMPLE_CONFIG

    remaining = json.loads(config.read_text(encoding="utf-8"))
    assert not OC_KEYS_IN_SAMPLE.keys() & remaining.keys()
    assert remaining == {
        key: value
        for key, value in SAMPLE_CONFIG.items()
        if key not in OC_KEYS_IN_SAMPLE
    }
    # Fans are EC-owned (nvoc never writes fan curves), so the GPU fan profile
    # must survive the strip by design.
    assert remaining["fan_profile_gpu_1"] == "1E-3F-44-48"

    joined = "\n".join(report.lines)
    assert "gpu_core_1=200" in joined
    assert "gpu_memory_1='1500'" in joined
    # Active-mode check: SAMPLE's current mode is 1 (Turbo) carrying 5 keys.
    assert any(
        "active mode: 'Turbo' (1)" in line and "5 GPU OC key(s) live here" in line
        for line in report.lines
    )
    for line in report.lines:
        assert line.startswith("[GHelper]")


def test_active_mode_summary_modes_and_unknown() -> None:
    stock = {"performance_mode": 2, "gpu_temp_2": 87, "gpu_core_1": 10}
    summary = active_mode_summary(stock)
    assert "'Silent' (2)" in summary
    assert "gpu_temp_2=87" in summary
    assert "gpu_core_1" not in summary  # other modes are not the active one

    custom = {"performance_mode": 5, "mode_name_5": "Ultimate Perf"}
    assert "'Ultimate Perf' (5)" in active_mode_summary(custom)
    assert "0 GPU OC key(s)" in active_mode_summary(custom)

    assert "unknown" in active_mode_summary({"gpu_core_1": 5})
    assert "unknown" in active_mode_summary({"performance_mode": -1})


def test_strip_never_overwrites_existing_backup(tmp_path: Path) -> None:
    config = tmp_path / "GHelper" / "config.json"
    backup = tmp_path / "GHelper" / "config.json.bak.nvoc"
    _write_config(config, SAMPLE_CONFIG)
    backup.write_text(json.dumps({"sentinel": 1}), encoding="utf-8")

    report = strip_oc_keys(config)

    assert report.status == "removed"
    assert json.loads(backup.read_text(encoding="utf-8")) == {"sentinel": 1}
    assert any("backup kept" in line for line in report.lines)
    # With the first backup frozen, the echoed values are the only record of
    # what was stripped this round.
    assert "gpu_core_1=200" in "\n".join(report.lines)


def test_strip_leaves_broken_config_untouched(tmp_path: Path) -> None:
    config = tmp_path / "GHelper" / "config.json"
    config.parent.mkdir(parents=True)
    config.write_text('{"performance_mode": 1,,', encoding="utf-8")

    report = strip_oc_keys(config)

    assert report.status == "error"
    assert report.backup_path is None
    assert not (tmp_path / "GHelper" / "config.json.bak.nvoc").exists()
    assert config.read_text(encoding="utf-8") == '{"performance_mode": 1,,'


def test_strip_without_oc_keys_is_clean(tmp_path: Path) -> None:
    config = tmp_path / "GHelper" / "config.json"
    clean_payload = {
        k: v for k, v in SAMPLE_CONFIG.items() if k not in OC_KEYS_IN_SAMPLE
    }
    _write_config(config, clean_payload)

    report = strip_oc_keys(config)

    assert report.status == "clean"
    assert report.removed == {}
    assert report.backup_path is None
    assert not (tmp_path / "GHelper" / "config.json.bak.nvoc").exists()
    assert config.read_text(encoding="utf-8") == json.dumps(clean_payload)
    # Active-mode line: current mode 1 (Turbo) is clean after the sample's
    # mode-1 keys were (hypothetically) removed from the payload.
    assert any(
        "active mode: 'Turbo' (1)" in line and "0 GPU OC key(s)" in line
        for line in report.lines
    )


def test_strip_handles_user_frozen_config(tmp_path: Path) -> None:
    config = tmp_path / "GHelper" / "config.json"
    _write_config(config, SAMPLE_CONFIG)
    config.chmod(stat.S_IREAD)  # user froze it manually

    report = strip_oc_keys(config)

    assert report.status == "removed"
    remaining = json.loads(config.read_text(encoding="utf-8"))
    assert not OC_KEYS_IN_SAMPLE.keys() & remaining.keys()


def test_strip_missing_config_reports_no_config(tmp_path: Path) -> None:
    report = strip_oc_keys(tmp_path / "GHelper" / "config.json")

    assert report.status == "no_config"


def test_run_guard_is_windows_gated(monkeypatch, tmp_path: Path) -> None:
    monkeypatch.setenv("APPDATA", str(tmp_path))
    monkeypatch.setattr(sys, "platform", "linux")

    report = run_guard()

    assert report.status == "unsupported"
    assert report.lines == []
    assert not (tmp_path / "GHelper").exists()


def test_run_guard_strips_default_path(monkeypatch, tmp_path: Path) -> None:
    config = tmp_path / "GHelper" / "config.json"
    _write_config(config, SAMPLE_CONFIG)
    monkeypatch.setenv("APPDATA", str(tmp_path))
    monkeypatch.setattr(sys, "platform", "win32")

    report = run_guard(process_check=False)

    assert report.status == "removed"
    assert report.backup_path == str(config) + ".bak.nvoc"
    assert report.ghelper_running is None
    assert not any("WARNING" in line for line in report.lines)
    remaining = json.loads(config.read_text(encoding="utf-8"))
    assert not OC_KEYS_IN_SAMPLE.keys() & remaining.keys()
    # The stripped config is frozen so G-Helper cannot write the keys back.
    assert any("frozen read-only" in line for line in report.lines)
    assert not os.access(config, os.W_OK)

    # A later run on the frozen, clean config stays quiet about the freeze.
    second = run_guard(process_check=False)
    assert second.status == "clean"
    assert not any("frozen read-only" in line for line in second.lines)


def test_run_guard_warns_about_running_ghelper(monkeypatch, tmp_path: Path) -> None:
    config = tmp_path / "GHelper" / "config.json"
    _write_config(config, SAMPLE_CONFIG)
    monkeypatch.setenv("APPDATA", str(tmp_path))
    monkeypatch.setattr(sys, "platform", "win32")
    monkeypatch.setattr(ghelper_guard, "ghelper_running", lambda: True)

    report = run_guard(process_check=True)

    assert report.status == "removed"
    assert report.ghelper_running is True
    assert any(
        "WARNING" in line and "GHelper.exe is running" in line for line in report.lines
    )


def test_run_guard_appends_mode_tip_when_config_exists(
    monkeypatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("APPDATA", str(tmp_path))
    monkeypatch.setattr(sys, "platform", "win32")

    # Removed: tip follows the strip report.
    _write_config(tmp_path / "GHelper" / "config.json", SAMPLE_CONFIG)
    removed = run_guard(process_check=False)
    assert removed.status == "removed"
    assert any("TIP" in line and "nvidia-smi -lgc" in line for line in removed.lines)
    assert any("custom G-Helper mode" in line for line in removed.lines)

    # Clean (already stripped once): tip still applies, stock caps remain.
    clean = run_guard(process_check=False)
    assert clean.status == "clean"
    assert any("TIP" in line and "custom G-Helper mode" in line for line in clean.lines)

    # No config: user does not run G-Helper — no tip noise. The frozen file
    # must be unfrozen before it can be removed on Windows.
    frozen = tmp_path / "GHelper" / "config.json"
    frozen.chmod(stat.S_IWRITE)
    frozen.unlink()
    absent = run_guard(process_check=False)
    assert absent.status == "no_config"
    assert not any("TIP" in line for line in absent.lines)
