from __future__ import annotations

from pathlib import Path

from nvoc_tui.native import NativeService


class FakeControl:
    def __init__(self) -> None:
        self.claims: list[tuple[str, dict]] = []
        self.releases = 0
        self.closed = False

    def claim(self, mode: str, **kwargs) -> bool:
        self.claims.append((mode, kwargs))
        return True

    def release(self) -> None:
        self.releases += 1

    def close(self) -> None:
        self.closed = True


def test_claim_control_noop_without_session():
    service = NativeService(Path("."))
    # No session attached → must not raise.
    service.claim_control("manual", manual_percent=50)


def test_claim_control_routes_to_session():
    service = NativeService(Path("."))
    control = FakeControl()
    service.srv = control
    service.claim_control("manual", manual_percent=42)
    assert control.claims == [("manual", {"manual_percent": 42})]


def test_claim_control_swallows_errors():
    service = NativeService(Path("."))

    class Boom:
        def claim(self, *_a, **_k):
            raise RuntimeError("srv down")

    service.srv = Boom()
    service.claim_control("auto")  # must not raise


def test_release_control_noop_without_session():
    service = NativeService(Path("."))
    service.release_control()  # no session → must not raise


def test_release_control_routes_to_session():
    service = NativeService(Path("."))
    control = FakeControl()
    service.srv = control
    service.release_control()
    assert control.releases == 1
    assert control.claims == []


def test_release_control_swallows_errors():
    service = NativeService(Path("."))

    class Boom:
        def release(self):
            raise RuntimeError("srv down")

    service.srv = Boom()
    service.release_control()  # must not raise


def test_close_srv_releases_session():
    service = NativeService(Path("."))
    control = FakeControl()
    service.srv = control
    service.close_srv()
    assert control.closed is True
    assert service.srv is None
