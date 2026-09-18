"""MCP server tests run without loading native GPU bindings."""

import asyncio
import importlib.util
import json
from pathlib import Path
from unittest.mock import Mock

import pytest

spec = importlib.util.spec_from_file_location(
    "scan_mcp_under_test",
    Path(__file__).parents[1] / "python/pynvoc/scan_mcp.py",
)
scan_mcp = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scan_mcp)
ScanServiceError = scan_mcp.ScanServiceError

TOOLS = {
    "submit_scan",
    "scan_status",
    "scan_list",
    "scan_log",
    "scan_result",
    "scan_control",
    "scan_cancel",
}


@pytest.fixture()
def stub(monkeypatch):
    instance = Mock()
    monkeypatch.setattr(scan_mcp, "_client_factory", lambda: instance)
    return instance


def test_submit_forwards_arguments_and_serializes_payload(stub):
    stub.start.return_value = {"id": 7, "state": "queued"}
    out = scan_mcp.submit_scan(256, "ultrafast", "request-1")
    stub.start.assert_called_once_with(256, "ultrafast", "request-1")
    assert json.loads(out) == {"id": 7, "state": "queued"}
    scan_mcp.submit_scan(9)
    stub.start.assert_called_with(9, "standard", None)


def test_read_tools_pass_payloads_and_log_offset_through(stub):
    stub.task.return_value = {"id": 3, "state": "running"}
    stub.tasks.return_value = [{"id": 3}]
    stub.log.return_value = {"text": "chunk", "next_offset": 4096}
    stub.result.return_value = {"task": {}, "final_csv": None, "directory": "d"}
    stub.control.return_value = {"manual_hold": [256], "recovery_required": []}
    assert json.loads(scan_mcp.scan_status(3))["state"] == "running"
    assert json.loads(scan_mcp.scan_list())["tasks"] == [{"id": 3}]
    assert json.loads(scan_mcp.scan_log(3, 4096))["next_offset"] == 4096
    stub.log.assert_called_with(3, 4096)
    assert json.loads(scan_mcp.scan_result(3))["directory"] == "d"
    assert json.loads(scan_mcp.scan_control())["manual_hold"] == [256]


def test_cancel_returns_task_payload(stub):
    stub.cancel.return_value = {"id": 5, "state": "cancelling"}
    assert json.loads(scan_mcp.scan_cancel(5))["state"] == "cancelling"
    stub.cancel.assert_called_once_with(5)


@pytest.mark.parametrize(
    ("message", "status"),
    [
        ("SCAN_BUSY", 409),
        ("MANUAL_CONTROL", 409),
        ("RECOVERY_REQUIRED", 409),
        ("UNAUTHORIZED", 401),
        ("FORBIDDEN", 403),
    ],
)
def test_arbitration_errors_surface_with_http_status(monkeypatch, message, status):
    monkeypatch.setattr(
        scan_mcp,
        "_client_factory",
        Mock(side_effect=ScanServiceError(message, status)),
    )
    with pytest.raises(RuntimeError, match=rf"\[HTTP {status}\] {message}"):
        scan_mcp.scan_control()


def test_main_refuses_to_start_without_an_automation_token(monkeypatch, capfd):
    monkeypatch.delenv("NVOC_SCAN_AUTOMATION_TOKEN", raising=False)
    with pytest.raises(SystemExit):
        scan_mcp.main()
    captured = capfd.readouterr()
    assert "NVOC_SCAN_AUTOMATION_TOKEN" in captured.err
    assert captured.out == ""
    monkeypatch.setenv("NVOC_SCAN_AUTOMATION_TOKEN", "short")
    with pytest.raises(SystemExit):
        scan_mcp.main()


def test_manual_only_controls_are_not_exposed_as_tools():
    for absent in ("takeover", "release", "recover"):
        assert not hasattr(scan_mcp, absent)
    names = {tool.name for tool in asyncio.run(scan_mcp.mcp.list_tools())}
    assert names == TOOLS


def test_tool_call_over_a_real_mcp_session(monkeypatch):
    from mcp.shared.memory import create_connected_server_and_client_session

    instance = Mock()
    instance.control.return_value = {"manual_hold": [256], "recovery_required": []}
    monkeypatch.setattr(scan_mcp, "_client_factory", lambda: instance)

    async def exchange():
        async with create_connected_server_and_client_session(scan_mcp.mcp) as session:
            await session.initialize()
            return await session.call_tool("scan_control", {})

    result = asyncio.run(exchange())
    assert not result.isError
    assert "manual_hold" in result.content[0].text
