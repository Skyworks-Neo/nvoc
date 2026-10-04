"""Client for the resident nvoc-srv control plane.

nvoc-srv runs as a service and owns the GPU control loop. Consumers connect to
it, register a session, and declare a control claim instead of spawning their
own srv. This is the Python counterpart of the Rust ``srv-client`` crate, used
by the desktop GUI and TUI so the human journey flows through the same
arbitration/lease rules as every other client.

The srv endpoints are plain loopback GET/POST with query strings; a small
``http.client`` wrapper keeps this dependency-free. Every mutating call carries
the ``X-Requested-With: XMLHttpRequest`` header (srv's CSRF guard). Where auth
is enabled (Windows ``auth = "auto"``), the caller supplies OS credentials and
this client sends the same ``Authorization: Basic`` header the web console's
browser prompt would.
"""

from __future__ import annotations

import base64
import http.client
import json
import threading
from enum import Enum
from typing import Any, Optional

import urllib.parse

#: Default port of the resident control plane (must match ``srv::config``).
DEFAULT_PORT = 14514

#: Renew well inside the srv lease window (30 s) so a single missed beat is
#: harmless.
LEASE_TTL_S = 30.0
HEARTBEAT_INTERVAL_S = 10.0

#: Consumer priority bands (mirror ``srv-client/src/lib.rs`` and
#: ``srv/src/session.rs``).
PRIORITY_STRESSOR = 10
#: The scan holds a temperature-critical exclusive session for its whole
#: duration, so it outranks the MCP agent; a person still preempts it.
PRIORITY_OPTIMIZER = 35
PRIORITY_MCP = 30
#: The desktop GUI/TUI band: above the MCP agent, below the reserved console.
PRIORITY_DESKTOP = 40

_IO_TIMEOUT_S = 3.0


class SrvError(Exception):
    """A srv request failed (transport error, non-2xx, or bad response)."""


class AuthRequired(SrvError):
    """The srv requires credentials (HTTP 401/407)."""


class ProbeState(Enum):
    """Outcome of :meth:`SrvClient.probe_state`."""

    #: Healthy srv with its control loop live.
    READY = "ready"
    #: srv answers but discovery is still running.
    STARTING = "starting"
    #: Nothing is listening.
    ABSENT = "absent"
    #: srv is present but refused the request without credentials.
    AUTH_REQUIRED = "auth_required"


def percent_encode(value: str) -> str:
    """RFC 3986 encode a query value (unreserved set left alone)."""
    return urllib.parse.quote(str(value), safe="")


def basic_auth_header(username: str, password: str) -> str:
    """Build an ``Authorization: Basic`` header value."""
    token = base64.b64encode(f"{username}:{password}".encode()).decode("ascii")
    return f"Basic {token}"


class SrvClient:
    """Loopback transport for the srv control plane."""

    def __init__(
        self,
        port: int = DEFAULT_PORT,
        host: str = "127.0.0.1",
        username: Optional[str] = None,
        password: Optional[str] = None,
        auth_header: Optional[str] = None,
        timeout: float = _IO_TIMEOUT_S,
    ) -> None:
        self.port = port
        self.host = host
        if auth_header is not None:
            self.auth_header = auth_header
        elif username is not None:
            self.auth_header = basic_auth_header(username, password or "")
        else:
            self.auth_header = None
        self.timeout = timeout

    def _request(self, method: str, path: str, params=None) -> tuple[int, str]:
        query = "&".join(
            f"{percent_encode(k)}={percent_encode(v)}"
            for k, v in (params or {}).items()
        )
        target = path if not query else f"{path}?{query}"
        headers = {"Connection": "close"}
        if self.auth_header:
            headers["Authorization"] = self.auth_header
        if method == "POST":
            # CSRF guard: mutations require the non-simple header.
            headers["X-Requested-With"] = "XMLHttpRequest"
        conn = http.client.HTTPConnection(self.host, self.port, timeout=self.timeout)
        try:
            conn.request(method, target, headers=headers)
            resp = conn.getresponse()
            body = resp.read().decode("utf-8", "replace")
            return resp.status, body
        finally:
            conn.close()

    def get(self, path: str, params=None) -> str:
        return self._ok(*self._request("GET", path, params))

    def post(self, path: str, params=None) -> str:
        return self._ok(*self._request("POST", path, params))

    @staticmethod
    def _ok(code: int, body: str) -> str:
        if code in (401, 407):
            raise AuthRequired(f"srv requires credentials ({code})")
        if code < 400:
            return body
        raise SrvError(f"srv returned {code}: {body.strip()}")

    def probe_state(self) -> ProbeState:
        """Probe the control plane (see :class:`ProbeState`)."""
        try:
            code, body = self._request("GET", "/status")
        except OSError:
            return ProbeState.ABSENT
        if code in (401, 407):
            return ProbeState.AUTH_REQUIRED
        if code >= 400:
            raise SrvError(f"srv returned {code}: {body.strip()}")
        try:
            data = json.loads(body)
        except ValueError as e:
            raise SrvError(f"srv /status is not JSON: {e}") from e
        gpus = data.get("gpus")
        if not isinstance(gpus, list):
            raise SrvError("srv /status missing 'gpus'")
        return ProbeState.READY if gpus else ProbeState.STARTING

    def status(self) -> Any:
        """Parsed ``/api/status`` snapshot."""
        return json.loads(self.get("/api/status"))

    def sessions(self) -> Any:
        """Parsed ``/api/sessions`` registry snapshot."""
        return json.loads(self.get("/api/sessions"))


class Session:
    """A registered consumer session with a renewable lease.

    :meth:`open` registers with the srv and starts a background heartbeat so
    the lease never lapses while the client is alive. :meth:`close` (or
    dropping the object) deregisters, which is how the srv learns to hand
    control back. A crashed client simply stops heartbeating and its claim
    lapses on the srv side.
    """

    def __init__(self, client: SrvClient, session_id: str) -> None:
        self._client = client
        self._id = session_id
        self._stop = threading.Event()
        self._hb: Optional[threading.Thread] = None
        self._closed = False

    @classmethod
    def open(
        cls,
        name: str,
        priority: int = PRIORITY_DESKTOP,
        port: int = DEFAULT_PORT,
        username: Optional[str] = None,
        password: Optional[str] = None,
        auth_header: Optional[str] = None,
    ) -> "Session":
        client = SrvClient(
            port=port,
            username=username,
            password=password,
            auth_header=auth_header,
        )
        body = client.post("/api/session/open", {"name": name, "priority": priority})
        try:
            session_id = json.loads(body)["id"]
        except (ValueError, KeyError) as e:
            raise SrvError(f"srv session/open response missing 'id': {body!r}") from e
        session = cls(client, session_id)
        session._start_heartbeat()
        return session

    @property
    def id(self) -> str:
        return self._id

    def _start_heartbeat(self) -> None:
        def beat() -> None:
            while not self._stop.wait(HEARTBEAT_INTERVAL_S):
                try:
                    self._client.post("/api/session/heartbeat", {"id": self._id})
                except (SrvError, OSError) as e:  # transient — keep trying
                    print(f"nvoc-srv-client: heartbeat failed: {e}")

        self._hb = threading.Thread(target=beat, name="nvoc-srv-heartbeat", daemon=True)
        self._hb.start()

    def claim(
        self,
        mode: str,
        target_c: Optional[float] = None,
        manual_percent: Optional[int] = None,
    ) -> bool:
        """Declare (or replace) this consumer's control claim.

        Returns whether this consumer is now the control owner — the srv
        arbitrates by priority, so a lower-priority claim may not win.
        """
        params = {"id": self._id, "mode": mode}
        if target_c is not None:
            params["target_c"] = target_c
        if manual_percent is not None:
            params["manual_percent"] = manual_percent
        body = self._client.post("/api/session/claim", params)
        try:
            return bool(json.loads(body).get("owner", False))
        except ValueError as e:
            raise SrvError(f"srv claim returned non-JSON: {e}") from e

    def release(self) -> None:
        """Hand the control band back without deregistering.

        The session's claim is dropped — control falls to the next claim, or
        ``auto`` if none remain — but the session stays registered and
        heartbeating, so a later :meth:`claim` re-takes the band without a
        reconnect. This is how the GUI/TUI yield when the user hands control
        back to the driver, so a waiting optimizer/MCP consumer can operate.
        """
        self._client.post("/api/session/release", {"id": self._id})

    def close(self) -> None:
        """Deregister and stop the heartbeat. Idempotent."""
        if self._closed:
            return
        self._closed = True
        self._stop.set()
        if self._hb is not None:
            self._hb.join(timeout=HEARTBEAT_INTERVAL_S + 1.0)
            self._hb = None
        try:
            self._client.post("/api/session/close", {"id": self._id})
        except (SrvError, OSError) as e:
            print(f"nvoc-srv-client: session close failed: {e}")

    def __enter__(self) -> "Session":
        return self

    def __exit__(self, *_exc) -> None:
        self.close()

    def __del__(self) -> None:  # best-effort
        try:
            self.close()
        except Exception:  # noqa: BLE001 - interpreter shutdown
            pass
