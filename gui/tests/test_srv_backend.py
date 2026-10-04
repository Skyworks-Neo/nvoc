from __future__ import annotations

from src.backend.base import FanSettings
from src.backend.srv import SrvBackend


class FakeConsole:
    def __init__(self) -> None:
        self.lines: list[str] = []

    def append(self, text: str) -> None:
        self.lines.append(text)


class FakeNative:
    def __init__(self) -> None:
        self.calls: list[str] = []

    def apply_fan_settings(self, settings: FanSettings) -> None:
        self.calls.append("apply_fan_settings")

    def reset_fan_settings(self, settings: FanSettings) -> None:
        self.calls.append("reset_fan_settings")

    def activate_fan_curve(self) -> None:
        self.calls.append("activate_fan_curve")

    def query_fan_info(self, gpu: str):
        return {"gpu": gpu}

    def shutdown(self) -> None:
        self.calls.append("shutdown")


class FakeSession:
    def __init__(self, owner: bool = True) -> None:
        self.claims: list[tuple[str, dict]] = []
        self.releases = 0
        self.closed = False
        self._owner = owner

    def claim(self, mode: str, **kwargs) -> bool:
        self.claims.append((mode, kwargs))
        return self._owner

    def release(self) -> None:
        self.releases += 1

    def close(self) -> None:
        self.closed = True


class FakeApp:
    def __init__(self) -> None:
        self.console = FakeConsole()


def _backend(owner: bool = True):
    app = FakeApp()
    session = FakeSession(owner)
    backend = SrvBackend(app, session)
    native = FakeNative()
    backend._native = native
    return app, backend, native, session


def test_manual_fan_claims_manual_duty():
    _app, backend, native, session = _backend()
    backend.apply_fan_settings(FanSettings(backend="nvapi", policy="manual", level=55))
    assert session.claims == [("manual", {"manual_percent": 55})]
    assert native.calls == []  # the srv owns the duty; no native write


def test_curve_policy_defers_to_native_then_yields():
    _app, backend, native, session = _backend()
    backend.apply_fan_settings(FanSettings(backend="nvapi", policy="curve", level=0))
    assert native.calls == ["apply_fan_settings"]
    assert session.claims == []  # no pinned claim — the band is handed back
    assert session.releases == 1


def test_reset_and_curve_activate_yield():
    _app, backend, native, session = _backend()
    backend.reset_fan_settings(FanSettings(backend="nvapi", policy="auto", level=0))
    backend.activate_fan_curve()
    assert native.calls == ["reset_fan_settings", "activate_fan_curve"]
    assert session.claims == []
    assert session.releases == 2


def test_lower_priority_claim_is_reported():
    app, backend, _native, session = _backend(owner=False)
    backend.apply_fan_settings(FanSettings(backend="nvapi", policy="manual", level=40))
    assert any("higher-priority" in line for line in app.console.lines)


def test_reads_delegate_to_native():
    _app, backend, _native, _session = _backend()
    assert backend.query_fan_info("gpu0") == {"gpu": "gpu0"}


def test_shutdown_closes_session_then_native():
    _app, backend, native, session = _backend()
    backend.shutdown()
    assert session.closed is True
    assert native.calls == ["shutdown"]
