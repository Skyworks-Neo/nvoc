"""Tests for the Python srv-client against an in-process fake control plane."""

from __future__ import annotations

import http.server
import json
import threading

import pytest

from nvoc_srv_client import (
    PRIORITY_DESKTOP,
    ProbeState,
    SrvClient,
    SrvError,
    Session,
    basic_auth_header,
    percent_encode,
)


class _Recorder:
    def __init__(self):
        self.requests = []
        self.status_body = {"gpus": [{"index": 0}]}
        self.status_code = 200
        self.session_body = {"id": "abc123", "ttl_s": 30}
        self.claim_body = {"ok": True, "owner": True}
        self.auth_required = False


def _make_server(rec: _Recorder):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_a):  # silence
            pass

        def _record_and_reply(self):
            length = int(self.headers.get("Content-Length") or 0)
            if length:
                self.rfile.read(length)
            rec.requests.append({
                "method": self.command,
                "path": self.path,
                "csrf": self.headers.get("X-Requested-With"),
                "auth": self.headers.get("Authorization"),
            })
            if self.path.startswith("/api/status") or self.path.startswith("/status"):
                body = json.dumps(rec.status_body)
                code = 401 if rec.auth_required else rec.status_code
            elif self.path.startswith("/api/session/open"):
                body = json.dumps(rec.session_body)
                code = 200
            elif self.path.startswith("/api/session/claim"):
                body = json.dumps(rec.claim_body)
                code = 200
            else:
                body = json.dumps({"ok": True})
                code = 200
            payload = body.encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        do_GET = _record_and_reply
        do_POST = _record_and_reply

    server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server


@pytest.fixture()
def fake():
    rec = _Recorder()
    server = _make_server(rec)
    yield rec, server.server_address[1]
    server.shutdown()


def test_percent_encode_matches_rust():
    assert percent_encode("nvoc-gui") == "nvoc-gui"
    assert percent_encode("a b&c") == "a%20b%26c"


def test_basic_auth_header():
    # base64("user:pass") == dXNlcjpwYXNz
    assert basic_auth_header("user", "pass") == "Basic dXNlcjpwYXNz"


def test_post_carries_csrf_and_basic(fake):
    rec, port = fake
    SrvClient(port=port, username="user", password="pass").post(
        "/api/session/heartbeat", {"id": "abc"}
    )
    req = rec.requests[-1]
    assert req["method"] == "POST"
    assert req["path"] == "/api/session/heartbeat?id=abc"
    assert req["csrf"] == "XMLHttpRequest"
    assert req["auth"] == "Basic dXNlcjpwYXNz"


def test_get_has_no_csrf(fake):
    rec, port = fake
    SrvClient(port=port).get("/api/status")
    assert rec.requests[-1]["csrf"] is None


def test_probe_state_tristate(fake):
    rec, port = fake
    client = SrvClient(port=port)
    assert client.probe_state() is ProbeState.READY
    rec.status_body = {"gpus": []}
    assert client.probe_state() is ProbeState.STARTING
    rec.auth_required = True
    assert client.probe_state() is ProbeState.AUTH_REQUIRED
    assert SrvClient(port=1).probe_state() is ProbeState.ABSENT


def test_error_on_4xx(fake):
    rec, port = fake
    rec.status_code = 401
    with pytest.raises(SrvError):
        SrvClient(port=port).get("/api/status")


def test_session_release_posts_id(fake):
    rec, port = fake
    session = Session.open(name="nvoc-gui", priority=PRIORITY_DESKTOP, port=port)
    session.release()
    req = rec.requests[-1]
    assert req["method"] == "POST"
    assert req["path"] == "/api/session/release?id=abc123"
    assert req["csrf"] == "XMLHttpRequest"
    session.close()


def test_session_open_claim_close(fake):
    rec, port = fake
    session = Session.open(name="nvoc-gui", priority=PRIORITY_DESKTOP, port=port)
    assert session.id == "abc123"
    open_req = rec.requests[0]
    assert open_req["path"] == "/api/session/open?name=nvoc-gui&priority=40"
    assert session.claim("manual", manual_percent=60) is True
    claim_req = rec.requests[-1]
    assert (
        claim_req["path"]
        == "/api/session/claim?id=abc123&mode=manual&manual_percent=60"
    )
    session.close()
    assert any(r["path"] == "/api/session/close?id=abc123" for r in rec.requests)
    # Idempotent.
    session.close()
