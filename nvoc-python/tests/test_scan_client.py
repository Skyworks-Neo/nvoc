"""Service-client tests run without loading native GPU bindings."""

import importlib.util
import json
from pathlib import Path
from unittest.mock import Mock
from urllib.error import HTTPError, URLError
from io import BytesIO

import pytest

spec = importlib.util.spec_from_file_location(
    "scan_client_under_test",
    Path(__file__).parents[1] / "python/pynvoc/scan_client.py",
)
client_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(client_module)
ScanClient = client_module.ScanClient
ScanServiceError = client_module.ScanServiceError


def client():
    instance = ScanClient(token="manual-secret")
    response = Mock()
    response.read.return_value = b'{"id": 1, "state": "queued"}'
    instance._opener = Mock()
    instance._opener.open.return_value.__enter__ = Mock(return_value=response)
    instance._opener.open.return_value.__exit__ = Mock(return_value=False)
    return instance


def test_start_preserves_idempotency_and_sends_no_origin_or_executable():
    c = client()
    c.start(256, "ultrafast", "request-1")
    request = c._opener.open.call_args.args[0]
    assert json.loads(request.data) == {
        "gpu_id": 256,
        "mode": "ultrafast",
        "request_id": "request-1",
    }
    assert request.get_header("Authorization") == "Bearer manual-secret"
    c.start(256, "ultrafast", "request-1")
    assert c._opener.open.call_args.args[0].data == request.data


def test_unavailable_service_never_starts_a_local_process():
    c = client()
    c._opener.open.side_effect = URLError("connection refused")
    with pytest.raises(ScanServiceError, match="may still be running"):
        c.start(256, request_id="same-on-retry")


def test_structured_rejection_retains_http_status():
    c = client()
    c._opener.open.side_effect = HTTPError(
        c.url, 409, "Conflict", {}, BytesIO(b'{"error":"MANUAL_CONTROL"}')
    )
    with pytest.raises(ScanServiceError) as caught:
        c.start(256)
    assert caught.value.status == 409
    assert str(caught.value) == "MANUAL_CONTROL"


def test_only_loopback_service_addresses_are_accepted():
    for url in [
        "https://example.com",
        "http://10.0.0.1",
        "http://localhost/path",
        "http://user@localhost",
    ]:
        with pytest.raises(ScanServiceError):
            ScanClient("token", url)


def test_mutating_methods_do_not_confuse_cancel_takeover_and_release():
    c = client()
    for method, value, suffix in [
        (c.cancel, 12, "/v1/scans/12/cancel"),
        (c.takeover, 256, "/v1/control/256/takeover"),
        (c.release, 256, "/v1/control/256/release"),
    ]:
        method(value)
        request = c._opener.open.call_args.args[0]
        assert request.full_url.endswith(suffix)
        assert request.method == "POST"
