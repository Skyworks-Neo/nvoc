"""HTTP client for srv-owned optimizer tasks; no GPU calls or child processes."""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.parse
import urllib.request
import uuid


TERMINAL_STATES = frozenset({"succeeded", "cancelled", "failed", "interrupted"})


class ScanServiceError(RuntimeError):
    def __init__(self, message: str, status: int = 0) -> None:
        super().__init__(message)
        self.status = status


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class ScanClient:
    """A credential's role is assigned by srv, never by a request parameter.

    Closing a client has no effect on its tasks. Reuse ``request_id`` when
    retrying a submission whose response was lost; do not blindly start again.
    """

    def __init__(
        self, token: str | None = None, url: str | None = None, timeout: float = 5
    ):
        self.token = (
            token if token is not None else os.environ.get("NVOC_SCAN_MANUAL_TOKEN", "")
        )
        self.url = (
            url or os.environ.get("NVOC_SCAN_URL", "http://127.0.0.1:14515")
        ).rstrip("/")
        parsed = urllib.parse.urlsplit(self.url)
        if (
            parsed.scheme != "http"
            or parsed.hostname not in {"127.0.0.1", "localhost", "::1"}
            or parsed.username
            or parsed.password
            or parsed.query
            or parsed.fragment
            or parsed.path
        ):
            raise ScanServiceError("Scan service URL must be a loopback HTTP address")
        if not self.token:
            raise ScanServiceError(
                "Set NVOC_SCAN_MANUAL_TOKEN to the manual credential configured for nvoc-srv"
            )
        self.timeout = timeout
        # Do not send local credentials through user-configured HTTP proxies.
        self._opener = urllib.request.build_opener(
            urllib.request.ProxyHandler({}), _NoRedirect()
        )

    def _request(self, method: str, path: str, body=None):
        data = (
            json.dumps(body).encode("utf-8")
            if body is not None
            else (b"" if method == "POST" else None)
        )
        request = urllib.request.Request(
            self.url + path,
            data=data,
            method=method,
            headers={
                "Authorization": "Bearer " + self.token,
                "Content-Type": "application/json",
            },
        )
        try:
            with self._opener.open(request, timeout=self.timeout) as response:
                payload = response.read(5 * 1024 * 1024 + 1)
                if len(payload) > 5 * 1024 * 1024:
                    raise ScanServiceError("Scan service response exceeds 5 MiB")
                return json.loads(payload)
        except urllib.error.HTTPError as exc:
            try:
                message = json.loads(exc.read(4096)).get("error", str(exc))
            except (ValueError, UnicodeError):
                message = str(exc)
            raise ScanServiceError(message, exc.code) from exc
        except (OSError, urllib.error.URLError) as exc:
            raise ScanServiceError(
                "Scan service unavailable; task may still be running. Refresh or retry with the same request ID."
            ) from exc

    def start(self, gpu_id: int, mode: str = "standard", request_id: str | None = None):
        if mode not in {"standard", "ultrafast", "legacy"}:
            raise ValueError("Unknown optimizer mode")
        if (
            isinstance(gpu_id, bool)
            or not isinstance(gpu_id, int)
            or not 0 <= gpu_id <= 0xFFFFFFFF
        ):
            raise ValueError(
                "gpu_id must be an unsigned 32-bit GPU ID, not a device index"
            )
        return self._request(
            "POST",
            "/v1/scans",
            {
                "gpu_id": gpu_id,
                "mode": mode,
                "request_id": request_id or uuid.uuid4().hex,
            },
        )

    def tasks(self):
        return self._request("GET", "/v1/scans")["tasks"]

    def task(self, task_id: int):
        return self._request("GET", f"/v1/scans/{int(task_id)}")

    def log(self, task_id: int, offset: int = 0):
        return self._request(
            "GET", f"/v1/scans/{int(task_id)}/log?offset={int(offset)}"
        )

    def result(self, task_id: int):
        return self._request("GET", f"/v1/scans/{int(task_id)}/result")

    def cancel(self, task_id: int):
        return self._request("POST", f"/v1/scans/{int(task_id)}/cancel")

    def control(self):
        return self._request("GET", "/v1/control")

    def takeover(self, gpu_id: int):
        return self._request("POST", f"/v1/control/{int(gpu_id)}/takeover")

    def release(self, gpu_id: int):
        return self._request("POST", f"/v1/control/{int(gpu_id)}/release")

    def recover(self, gpu_id: int):
        return self._request("POST", f"/v1/control/{int(gpu_id)}/recover")
