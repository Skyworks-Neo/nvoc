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
"""Neutralize G-Helper's persisted NVIDIA GPU OC before NVOC touches the GPU.

G-Helper (https://github.com/seerge/g-helper) keeps every GPU-side setting in
``%APPDATA%\\GHelper\\config.json`` as one flat ``<key>_<mode>`` entry per
ASUS performance mode (0=Balanced, 1=Turbo, 2=Silent, plus custom modes up to
19), and re-applies them at its own startup, performance-mode switch, AC/DC
transition and resume: core/memory offsets go out through NVAPI
SetPerformanceStates20 (P0 delta) — the same clock plane NVOC drives — the
clock lock through ``nvidia-smi -lgc``, power/temp/boost through ACPI/NVML.
Two tools writing one clock plane means last writer wins and the user's NVOC
profile silently decays.

``run_guard`` runs once at NVOC TUI/GUI startup on Windows:

1. Copy the config to ``config.json.bak.nvoc`` (never overwritten once it
   exists, so the user's original stays recoverable).
2. Delete every GPU OC key from the live config, echoing each removal into
   the returned log lines — after an earlier backup already exists, that
   echo is the only record of the stripped values.
3. Detect a running GHelper.exe. A live process holds its config in memory:
   file edits only stick until its next settings change, and it re-applies
   OC on its own events, so the only real fix is exiting it — the guard can
   only say so, loudly.
4. Report the active mode (``performance_mode`` + its keys): which mode the
   next event would actually replay, and whether it still carries anything.
5. After a successful strip, freeze the config read-only. G-Helper's writer
   (File.Replace/Move) fails on a READONLY file and swallows the error, so
   the keys stay out even while G-Helper runs; everything else keeps
   working in-session, and clearing the attribute re-enables editing.
6. When a config exists, append a standing tip: stock modes write no limits
   themselves, so the caps are ASUS mode power budgets (ACPI) and stale
   driver-side `nvidia-smi -lgc` locks — a G-Helper custom mode plus
   clearing stale locks is the workaround.

Nothing here may block or crash startup: every failure path returns a report
instead of raising. The strip/backup core is platform-neutral and unit-tested
everywhere; only ``run_guard`` gates on Windows and resolves the real APPDATA
path. Standalone nvoc-cli runs self-guard through the Rust twin in
``cli-common/src/ghelper_guard.rs`` — keep the key set and semantics in sync.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import stat
import sys
from contextlib import suppress
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

CONFIG_DIR_NAME = "GHelper"
CONFIG_FILE_NAME = "config.json"
BACKUP_SUFFIX = ".bak.nvoc"
LOG_PREFIX = "[GHelper]"

# G-Helper's Modes.cs registers exactly these settings as per-mode GPU-side
# writers: gpu_core/gpu_memory (NVAPI P0 delta offsets), gpu_clock_limit
# (nvidia-smi -lgc lock), and gpu_power/gpu_temp/gpu_boost (ACPI/NVML).
# fan_profile_gpu is deliberately absent: it drives the EC fan controller,
# which NVOC does not touch (laptop fans are EC-owned; G-Helper has no GPU
# fan surface on desktops). Non-mode keys (gpu_mode, max_gpu_core, ...) are
# UI state, not writers, and are left alone.
GPU_OC_KEYS: frozenset = frozenset({
    "gpu_core",
    "gpu_memory",
    "gpu_clock_limit",
    "gpu_power",
    "gpu_temp",
    "gpu_boost",
})

# Modes.cs: performance_mode is 0/1/2 plus custom modes 3..maxModes-1 (20).
MAX_PERF_MODE = 19

# Code-verified mechanism (g-helper source): stock modes write NO limits —
# ModeControl.SetGPUClocks early-returns without per-mode keys and
# SetGPUPower range-guards every key. What actually caps frequencies: ASUS
# mode-switch power budgets (ACPI/firmware), and any clock lock G-Helper ever
# set via `nvidia-smi -lgc`, which persists in the driver until `-rgc` (its
# slider max is the const 3000 "Default" sentinel, so a stale sub-3000 lock
# survives app removal and config strips alike).
MODE_TIP_LINES = (
    f"{LOG_PREFIX} TIP: G-Helper writes no limits on stock modes itself — caps come "
    "from ASUS mode power budgets (ACPI) and any clock lock G-Helper ever set, which "
    "persists in the driver (nvidia-smi -lgc until -rgc).",
    f"{LOG_PREFIX} TIP: prefer a custom G-Helper mode, clear stale clock locks (slider "
    "to far right = Default, or `nvidia-smi -rgc`), then let NVOC own the GPU clocks.",
)

_OC_KEY_RE = re.compile(r"^(?P<base>[a-z_]+)_(?P<mode>\d{1,2})$")
_LOG_VALUE_LIMIT = 12


@dataclass
class GuardReport:
    """Outcome of one guard run; `lines` are ready-to-log English strings."""

    status: str  # "removed" | "clean" | "no_config" | "error" | "unsupported"
    lines: list = field(default_factory=list)
    removed: dict = field(default_factory=dict)
    backup_path: Any = None
    ghelper_running: Any = None


def default_config_path():
    """%APPDATA%\\GHelper\\config.json, or None when APPDATA is unset."""
    appdata = os.environ.get("APPDATA")
    if not appdata:
        return None
    return Path(appdata) / CONFIG_DIR_NAME / CONFIG_FILE_NAME


def find_oc_keys(data):
    """Pick the per-mode GPU OC keys (``<key>_<mode>``, mode <= 19) from a
    loaded config dict. Values pass through untouched — G-Helper stores ints
    and strings alike and re-parses them on read."""
    found = {}
    for key, value in data.items():
        match = _OC_KEY_RE.match(key)
        if match is None:
            continue
        if match.group("base") not in GPU_OC_KEYS:
            continue
        if int(match.group("mode")) > MAX_PERF_MODE:
            continue
        found[key] = value
    return found


_STOCK_MODE_NAMES = {0: "Balanced", 1: "Turbo", 2: "Silent"}


def active_mode_summary(data):
    """One line naming the mode G-Helper's next event would replay, and the
    GPU OC keys that mode currently carries — the guard reports it so the
    user knows which mode the interference actually applies to."""
    try:
        mode = int(data.get("performance_mode"))
    except (TypeError, ValueError):
        return "active mode: unknown (no usable performance_mode key)"
    if mode < 0:
        return "active mode: unknown (no usable performance_mode key)"
    name = data.get(f"mode_name_{mode}")
    if not isinstance(name, str) or not name:
        name = _STOCK_MODE_NAMES.get(mode, "Custom")
    suffix = f"_{mode}"
    keys = {k: v for k, v in find_oc_keys(data).items() if k.endswith(suffix)}
    if keys:
        listing = ", ".join(f"{k}={v!r}" for k, v in sorted(keys.items()))
        return (
            f"active mode: '{name}' ({mode}) — "
            f"{len(keys)} GPU OC key(s) live here: {listing}"
        )
    return f"active mode: '{name}' ({mode}) — 0 GPU OC key(s) (replay is a no-op)"


def strip_oc_keys(config_path, backup_path=None):
    """Back up `config_path`, then rewrite it without its GPU OC keys.

    The backup is a byte-for-byte copy created only when a modification is
    about to happen; an existing backup is never overwritten. A config that
    fails to parse is left completely untouched — G-Helper has its own
    recovery pass for broken configs, and a truncated file would destroy
    that chance.
    """
    config_path = Path(config_path)
    if backup_path is None:
        backup_path = config_path.with_name(config_path.name + BACKUP_SUFFIX)
    backup_path = Path(backup_path)

    if not config_path.is_file():
        return GuardReport(status="no_config")
    try:
        data = json.loads(config_path.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            raise ValueError("config root is not a JSON object")
    except (OSError, UnicodeDecodeError, ValueError) as exc:
        return GuardReport(
            status="error",
            lines=[f"{LOG_PREFIX} config parse failed ({exc}) — file left untouched"],
        )

    removed = find_oc_keys(data)
    active = active_mode_summary(data)
    if not removed:
        return GuardReport(
            status="clean",
            lines=[
                f"{LOG_PREFIX} G-Helper config carries no GPU OC keys — nothing stripped",
                f"{LOG_PREFIX} {active}",
            ],
        )
    for key in removed:
        del data[key]

    entries = ", ".join(f"{key}={removed[key]!r}" for key in sorted(removed))
    if len(removed) > _LOG_VALUE_LIMIT:
        entries += f" … (+{len(removed) - _LOG_VALUE_LIMIT} more, see backup)"
    lines = [
        f"{LOG_PREFIX} removed {len(removed)} GPU OC key(s) from {config_path}:",
        f"{LOG_PREFIX}   {entries}",
        f"{LOG_PREFIX} {active}",
    ]

    try:
        if backup_path.exists():
            lines.append(f"{LOG_PREFIX} backup kept (exists): {backup_path}")
        else:
            shutil.copy2(config_path, backup_path)
            lines.append(f"{LOG_PREFIX} backup written: {backup_path}")
    except OSError as exc:
        # Without a backup the strip is destructive with no undo — abort.
        return GuardReport(
            status="error",
            removed=removed,
            backup_path=str(backup_path),
            lines=[
                f"{LOG_PREFIX} backup failed ({exc}) — file left untouched",
            ],
        )

    tmp_path = config_path.with_name(config_path.name + ".nvoc-tmp")
    unfroze = False
    try:
        if not os.access(config_path, os.W_OK):
            # A user-frozen (READONLY) config: clear the attribute just long
            # enough to write; run_guard re-freezes on success.
            os.chmod(config_path, stat.S_IWRITE)
            unfroze = True
        tmp_path.write_text(json.dumps(data, indent=2), encoding="utf-8")
        os.replace(tmp_path, config_path)
    except OSError as exc:
        if unfroze:
            with suppress(OSError):
                os.chmod(config_path, stat.S_IREAD)
        with suppress(OSError):
            tmp_path.unlink()
        lines.append(f"{LOG_PREFIX} strip failed ({exc}) — config may be unchanged")
        return GuardReport(
            status="error",
            removed=removed,
            backup_path=str(backup_path),
            lines=lines,
        )

    return GuardReport(
        status="removed",
        removed=removed,
        backup_path=str(backup_path),
        lines=lines,
    )


def ghelper_running():
    """Whether GHelper.exe is running (Windows); None when detection fails.

    Reads the Toolhelp32 snapshot directly — no third-party dependency, no
    tasklist child process in the startup path.
    """
    if sys.platform != "win32":
        return None
    try:
        import ctypes.wintypes

        kernel32 = ctypes.WinDLL("kernel32.dll", use_last_error=True)

        class _ProcessEntry32W(ctypes.Structure):
            _fields_ = [
                ("dwSize", ctypes.wintypes.DWORD),
                ("cntUsage", ctypes.wintypes.DWORD),
                ("th32ProcessID", ctypes.wintypes.DWORD),
                ("th32DefaultHeapID", ctypes.c_size_t),
                ("th32ModuleID", ctypes.wintypes.DWORD),
                ("cntThreads", ctypes.wintypes.DWORD),
                ("th32ParentProcessID", ctypes.wintypes.DWORD),
                # LONG, not a pointer: the Win32 header keeps this field
                # 4 bytes even on 64-bit, and a wider type here shifts
                # szExeFile off its real offset.
                ("pcPriClassBase", ctypes.wintypes.LONG),
                ("dwFlags", ctypes.wintypes.DWORD),
                ("szExeFile", ctypes.wintypes.WCHAR * 260),
            ]

        kernel32.CreateToolhelp32Snapshot.restype = ctypes.wintypes.HANDLE
        kernel32.CreateToolhelp32Snapshot.argtypes = [
            ctypes.wintypes.DWORD,
            ctypes.wintypes.DWORD,
        ]
        for name in ("Process32FirstW", "Process32NextW"):
            func = getattr(kernel32, name)
            func.restype = ctypes.wintypes.BOOL
            func.argtypes = [
                ctypes.wintypes.HANDLE,
                ctypes.POINTER(_ProcessEntry32W),
            ]
        kernel32.CloseHandle.restype = ctypes.wintypes.BOOL
        kernel32.CloseHandle.argtypes = [ctypes.wintypes.HANDLE]

        snapshot = kernel32.CreateToolhelp32Snapshot(0x2, 0)  # TH32CS_SNAPPROCESS
        if snapshot == ctypes.wintypes.HANDLE(-1).value:  # INVALID_HANDLE_VALUE
            return None
        entry = _ProcessEntry32W()
        entry.dwSize = ctypes.sizeof(_ProcessEntry32W)
        try:
            found = kernel32.Process32FirstW(snapshot, ctypes.byref(entry))
            while found:
                if entry.szExeFile.lower() == "ghelper.exe":
                    return True
                found = kernel32.Process32NextW(snapshot, ctypes.byref(entry))
        finally:
            kernel32.CloseHandle(snapshot)
        return False
    except Exception:
        return None


def run_guard(config_path=None, backup_path=None, process_check=True):
    """One startup pass: backup + strip + live-process warning.

    Windows-only; anywhere else it reports "unsupported" without touching
    anything, so callers can invoke it unconditionally. Never raises.
    """
    if sys.platform != "win32":
        return GuardReport(status="unsupported")

    if config_path is None:
        config_path = default_config_path()
        if config_path is None:
            return GuardReport(
                status="error",
                lines=[
                    f"{LOG_PREFIX} APPDATA is not set — cannot locate G-Helper config"
                ],
            )

    running = ghelper_running() if process_check else None
    report = strip_oc_keys(config_path, backup_path)
    report.ghelper_running = running

    if report.status == "no_config":
        report.lines = [
            f"{LOG_PREFIX} no G-Helper config at {config_path} — nothing to do"
        ]

    if report.status == "removed":
        # Freeze the stripped config: G-Helper's debounced writer (WriteAtomic:
        # File.Replace / File.Move) fails on a READONLY file and it swallows
        # the error, so the stripped keys stay out even when a running
        # G-Helper tries to persist its in-memory copy. Empirically harmless
        # to G-Helper itself (everything keeps working in-session; only
        # settings persistence stops until the attribute is cleared).
        try:
            os.chmod(config_path, stat.S_IREAD)  # FILE_ATTRIBUTE_READONLY
            report.lines.append(
                f"{LOG_PREFIX} config frozen read-only — G-Helper cannot write the "
                "stripped keys back (clear the read-only attribute to edit "
                "G-Helper settings again)"
            )
        except OSError as exc:
            report.lines.append(
                f"{LOG_PREFIX} could not freeze config read-only ({exc})"
            )

    if running:
        report.lines.append(
            f"{LOG_PREFIX} WARNING: GHelper.exe is running — it holds its config in "
            "memory, re-applies stored OC on its own events and can rewrite the "
            "file. Exit G-Helper (tray icon > Exit) before relying on NVOC writes."
        )
    if report.status in ("removed", "clean", "error"):
        report.lines.extend(MODE_TIP_LINES)
    return report
