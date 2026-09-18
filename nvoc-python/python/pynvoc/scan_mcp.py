"""MCP server for srv-owned optimizer scans, using the automation credential.

Run by file path (``python scan_mcp.py``), never as ``pynvoc.scan_mcp``:
importing the ``pynvoc`` package would pull in its native extension. The
scan client is loaded from the sibling file instead. This server exposes
scan tools only; takeover/release/recover stay manual-credential exclusive
and are deliberately absent, so automation can never seize a GPU.
"""

from __future__ import annotations

import importlib.util
import json
import os
import sys
from pathlib import Path

from mcp.server.fastmcp import FastMCP
from mcp.types import ToolAnnotations

_spec = importlib.util.spec_from_file_location(
    "scan_client_for_mcp", Path(__file__).with_name("scan_client.py")
)
_client_module = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_client_module)
ScanClient = _client_module.ScanClient
ScanServiceError = _client_module.ScanServiceError

mcp = FastMCP("nvoc-scans")


def _client_factory() -> ScanClient:
    token = os.environ.get("NVOC_SCAN_AUTOMATION_TOKEN", "")
    if not token:
        raise RuntimeError(
            "Set NVOC_SCAN_AUTOMATION_TOKEN to the automation credential "
            "configured for nvoc-srv"
        )
    return ScanClient(token=token, url=os.environ.get("NVOC_SCAN_URL") or None)


def _client() -> ScanClient:
    return _client_factory()


def _run(action):
    """Surface srv arbitration verbatim: [HTTP status] SCAN_BUSY and friends."""
    try:
        return json.dumps(action())
    except ScanServiceError as error:
        raise RuntimeError(f"[HTTP {error.status}] {error}") from error


@mcp.tool(annotations=ToolAnnotations(readOnlyHint=False, destructiveHint=True))
def submit_scan(
    gpu_id: int, mode: str = "standard", request_id: str | None = None
) -> str:
    """Submit one hosted optimizer scan for a GPU.

    gpu_id is the stable GPU ID shown by `nvoc-cli get-gpu-list` (PCI bus
    times 256), not a device index. Modes: standard (thorough), ultrafast
    (quick), legacy. Scans are globally serialized: another running task
    fails with SCAN_BUSY, a manual takeover on the GPU with MANUAL_CONTROL,
    pending driver recovery with RECOVERY_REQUIRED — all reported as
    [HTTP 409] tool errors. If the response is lost, retry with the SAME
    request_id; srv treats that idempotently. Returns the task JSON.
    """
    return _run(lambda: _client().start(gpu_id, mode, request_id))


@mcp.tool(annotations=ToolAnnotations(readOnlyHint=True, idempotentHint=True))
def scan_status(task_id: int) -> str:
    """Fetch one task: id, request, origin, state, error, exit_code.

    State is queued/running/cancelling/recovering while active, then
    succeeded/cancelled/failed/interrupted. Failed tasks carry an error
    string; recovery is required before the GPU accepts new scans.
    """
    return _run(lambda: _client().task(task_id))


@mcp.tool(annotations=ToolAnnotations(readOnlyHint=True))
def scan_list() -> str:
    """List every hosted optimizer task known to srv, all states."""
    return _run(lambda: {"tasks": _client().tasks()})


@mcp.tool(annotations=ToolAnnotations(readOnlyHint=True))
def scan_log(task_id: int, offset: int = 0) -> str:
    """Read optimizer stdout as {"text", "next_offset"}.

    Offsets are byte offsets: pass a previous next_offset back unchanged
    to stream the log. Text may be truncated at 64 KiB per call.
    """
    return _run(lambda: _client().log(task_id, offset))


@mcp.tool(annotations=ToolAnnotations(readOnlyHint=True))
def scan_result(task_id: int) -> str:
    """Fetch {"task", "final_csv", "directory"} for a finished task.

    final_csv exists only for succeeded scans and can be large (srv caps
    it at 4 MiB); directory is the on-disk task workspace for artifacts
    the API does not expose.
    """
    return _run(lambda: _client().result(task_id))


@mcp.tool(annotations=ToolAnnotations(readOnlyHint=True, idempotentHint=True))
def scan_control() -> str:
    """Read arbitration state: {"manual_hold", "recovery_required"}.

    manual_hold lists GPU IDs manually taken over; automation submissions
    for those GPUs fail with MANUAL_CONTROL. recovery_required lists GPUs
    awaiting an explicit driver recovery; submissions fail with
    RECOVERY_REQUIRED. Both recoveries are manual-credential actions not
    available to this server.
    """
    return _run(lambda: _client().control())


@mcp.tool(
    annotations=ToolAnnotations(
        readOnlyHint=False, destructiveHint=True, idempotentHint=True
    )
)
def scan_cancel(task_id: int) -> str:
    """Cancel a task; srv terminates the whole optimizer process tree.

    Automation cannot cancel manual-origin tasks (FORBIDDEN). Returns the
    task JSON with state cancelling, then cancelled.
    """
    return _run(lambda: _client().cancel(task_id))


def _require_automation_token() -> None:
    token = os.environ.get("NVOC_SCAN_AUTOMATION_TOKEN", "")
    if len(token) < 32:
        print(
            "nvoc-scan-mcp: set NVOC_SCAN_AUTOMATION_TOKEN to the automation "
            "credential configured for nvoc-srv (at least 32 characters).",
            file=sys.stderr,
        )
        raise SystemExit(2)


def main() -> None:
    _require_automation_token()
    mcp.run()  # stdio: stdout is the protocol channel; diagnostics go to stderr.


if __name__ == "__main__":
    main()
