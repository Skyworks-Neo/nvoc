"""Pure helpers behind the VF Curve tab's stressor tool panel.

Deliberately free of Tk imports so argument construction, the ``--help``
capability probe, executable discovery and output summarising stay unit
testable (see ``gui/tests/test_stressor_args.py``).

Two stressor generations are expected in the field: the main-lineage build
(``--enable-vulkan-stress`` / ``--vulkan-image-*``) and the enhanced build
that renamed the Vulkan family and added the verify/SDC family
(``--verify-continue-on-error``, ``--json-out``, ...). Every option the panel
can emit therefore carries candidate spellings in preference order and the
probe decides which one the local executable actually accepts.
"""

import json
import os
import re
import shutil
import subprocess
import sys
from typing import List, Mapping, Optional, Sequence, Set, Tuple

# (state key, candidate long flags in preference order, takes a value).
# The tuple order is the emit order on the command line.
CONTROL_SPECS: Tuple[Tuple[str, Tuple[str, ...], bool], ...] = (
    ("gpu_index", ("--gpu-index",), True),
    ("cuda_path", ("--cuda-path",), True),
    ("profile", ("--profile",), True),
    ("duration", ("--duration",), True),
    ("matrix_sizes", ("--matrix-sizes",), True),
    ("precisions", ("--precisions",), True),
    ("kernel_types", ("--kernel-types",), True),
    ("stream_mode", ("--stream-mode",), True),
    ("validate_interval", ("--validate-interval",), True),
    ("validate_size", ("--validate-size",), True),
    ("seed", ("--seed",), True),
    ("gpu_generate", ("--gpu-generate",), False),
    ("no_verify", ("--no-verify",), False),
    ("verify_continue_on_error", ("--verify-continue-on-error",), False),
    ("verify_resident_interval", ("--verify-resident-interval",), True),
    ("verify_slab", ("--verify-slab",), False),
    ("gemm_full_check_max_size", ("--gemm-full-check-max-size",), True),
    ("skip_self_test", ("--skip-self-test",), False),
    ("json_out", ("--json-out",), True),
    ("vulkan", ("--vulkan", "--enable-vulkan-stress"), False),
    ("vulkan_only", ("--vulkan-only",), False),
    ("vulkan_width", ("--vulkan-width", "--vulkan-image-width"), True),
    ("vulkan_height", ("--vulkan-height", "--vulkan-image-height"), True),
    ("vulkan_msaa", ("--vulkan-msaa", "--vulkan-image-msaa"), True),
    ("vulkan_iters", ("--vulkan-iters",), True),
    ("vulkan_shells", ("--vulkan-shells",), True),
    ("vulkan_window", ("--vulkan-window",), False),
    # The CLI declares these two as value-taking (bool / u32) even though the
    # panel renders them as a checkbox and a text field.
    ("vulkan_rotate", ("--vulkan-rotate",), True),
    ("vulkan_particles", ("--vulkan-particles",), True),
    ("vulkan_offscreen", ("--vulkan-heavy-offscreen",), False),
)

_SPEC_LOOKUP = {key: (flags, takes) for key, flags, takes in CONTROL_SPECS}


def control_specs() -> Tuple[Tuple[str, Tuple[str, ...], bool], ...]:
    """The declarative control table, for the panel's capability gating."""
    return CONTROL_SPECS


def resolve_flag(
    candidates: Sequence[str], capabilities: Optional[Set[str]]
) -> Optional[str]:
    """First candidate the local executable advertises.

    ``None`` means the build does not support this option (skip it), except
    when ``capabilities`` is ``None``: that is "probe unavailable", which is
    treated as "do not gate" so an unprobeable executable still gets a full
    command line (clap reports the offending flag if the guess was wrong).
    """
    for flag in candidates:
        if capabilities is None or flag in capabilities:
            return flag
    return None


def flag_for(key: str, capabilities: Optional[Set[str]]) -> Optional[str]:
    """Resolved flag spelling for a control key, or None when unsupported."""
    spec = _SPEC_LOOKUP.get(key)
    if spec is None:
        return None
    return resolve_flag(spec[0], capabilities)


def build_stressor_args(
    state: Mapping[str, object], capabilities: Optional[Set[str]] = None
) -> List[str]:
    """Turn panel state into stressor argv (no executable, no GPU selector)."""
    args: List[str] = []
    for key, candidates, takes_value in CONTROL_SPECS:
        flag = resolve_flag(candidates, capabilities)
        if flag is None:
            continue
        if takes_value:
            raw = state.get(key, "")
            if isinstance(raw, bool):
                # Checkbox feeding a value-taking option (--vulkan-rotate):
                # forward the state explicitly instead of a bare flag.
                value = "true" if raw else "false"
            else:
                value = str(raw or "").strip()
            if value:
                args += [flag, value]
        elif state.get(key):
            args.append(flag)
    args += split_extra_args(str(state.get("extra_args", "") or ""))
    return args


def parse_help_flags(help_text: str) -> Set[str]:
    """Long flags advertised by a clap ``--help`` dump."""
    found = set()
    for match in re.finditer(r"--[A-Za-z][A-Za-z0-9-]*", help_text or ""):
        found.add(match.group(0).rstrip("-"))
    return found


def _no_window_kwargs() -> dict:
    if sys.platform == "win32" and hasattr(subprocess, "CREATE_NO_WINDOW"):
        return {"creationflags": subprocess.CREATE_NO_WINDOW}
    return {}


def probe_stressor_flags(exe_path: str, timeout: float = 20.0) -> Optional[Set[str]]:
    """Run ``<exe> --help`` and return its long-flag set.

    ``None`` (missing executable, non-zero exit, launch failure) means
    "capabilities unknown" — see :func:`resolve_flag`.
    """
    if not exe_path or not os.path.isfile(exe_path):
        return None
    try:
        result = subprocess.run(
            [exe_path, "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
            **_no_window_kwargs(),
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    return parse_help_flags(result.stdout)


def discover_stressor_exe(config_value: str = "", cli_exe_path: str = "") -> str:
    """Locate cli-stressor-cuda-rs: configured path, optimizer dir, PATH."""
    configured = (config_value or "").strip()
    if configured and os.path.isfile(configured):
        return configured

    names = ("cli-stressor-cuda-rs.exe", "cli-stressor-cuda-rs")
    candidates: List[str] = []
    cli_dir = os.path.dirname((cli_exe_path or "").strip())
    if cli_dir:
        candidates += [os.path.join(cli_dir, name) for name in names]
    for name in names:
        found = shutil.which(name)
        if found:
            candidates.append(found)
    for path in candidates:
        if os.path.isfile(path):
            return path
    return ""


def split_extra_args(text: str) -> List[str]:
    """Quote-aware split that keeps Windows backslash paths intact.

    ``shlex`` is unusable here: posix mode eats backslashes, non-posix mode
    keeps the quote characters. Backslashes are always literal.
    """
    args: List[str] = []
    buf: List[str] = []
    quote: Optional[str] = None
    for ch in text or "":
        if quote is not None:
            if ch == quote:
                quote = None
            else:
                buf.append(ch)
        elif ch in "\"'":
            quote = ch
        elif ch.isspace():
            if buf:
                args.append("".join(buf))
                buf = []
        else:
            buf.append(ch)
    if buf:
        args.append("".join(buf))
    return args


VERDICT_PREFIX = "VERDICT_JSON:"


def summarize_line(line: str) -> Optional[str]:
    """Compact status text for a stressor output line, else None.

    The enhanced stressor prints one ``VERDICT_JSON: {...}`` line when a run
    completes; the panel mirrors it into its result label.
    """
    index = line.find(VERDICT_PREFIX)
    if index == -1:
        return None
    payload = line[index + len(VERDICT_PREFIX) :].strip()
    try:
        data = json.loads(payload)
    except ValueError:
        return f"verdict: {payload[:120]}"
    if not isinstance(data, dict):
        return f"verdict: {payload[:120]}"
    parts = [f"verdict: {data.get('result') or '?'}"]
    classification = str(data.get("classification") or "").strip()
    if classification and classification.lower() != "none":
        parts.append(f"classification={classification}")
    confidence = data.get("confidence_pct")
    if isinstance(confidence, int):
        parts.append(f"confidence={confidence}%")
    device = str(data.get("device") or "").strip()
    if device:
        parts.append(f"device={device}")
    return " ".join(parts)


def format_exit_status(code: int) -> str:
    """Human status for a finished stressor process."""
    if code == 0:
        return "finished (exit 0)"
    if code == -1:
        return "cancelled"
    if code == 2:
        return f"error (exit {code})"
    return f"failed (exit {code})"
