"""TUI attachment to the **resident nvoc-srv control plane**.

Mirrors ``gui/src/backend/srv.py``: when a resident ``nvoc-srv`` is present the
TUI registers as a consumer in its session registry and routes the arbitrated
control operations (manual fan duty, reset, hand-back-to-driver) through a
session claim. Deep OC/VF writes stay direct-to-pynvoc; the srv arbiter only
governs the thermal/fan control loop.

Nothing here is required for the TUI to run: if no srv is reachable, or the
``nvoc_srv_client`` package is unavailable, the caller simply keeps driving
pynvoc directly.
"""

from __future__ import annotations

from typing import Optional

DEFAULT_PORT = 14514


def probe(port: int = DEFAULT_PORT) -> Optional[str]:
    """Return the srv probe state name, or ``None`` if we cannot tell.

    States: ``"ready"`` / ``"starting"`` / ``"absent"`` / ``"auth_required"``
    (see ``nvoc_srv_client.ProbeState``). ``None`` means the client package is
    missing or the port answered with something that is not a healthy srv.
    """
    try:
        from nvoc_srv_client import SrvClient
    except Exception:  # noqa: BLE001 - client not installed → native only
        return None
    try:
        return SrvClient(port=port).probe_state().value
    except Exception:  # noqa: BLE001 - unknown occupant → native only
        return None


class SrvControl:
    """A live consumer session; arbitrated control ops go through it."""

    def __init__(self, session) -> None:
        self._session = session

    @property
    def id(self) -> str:
        return self._session.id

    def claim(self, mode: str, **kwargs) -> bool:
        return self._session.claim(mode, **kwargs)

    def release(self) -> None:
        self._session.release()

    def close(self) -> None:
        self._session.close()


def attach(
    port: int = DEFAULT_PORT,
    name: str = "nvoc-tui",
    username: Optional[str] = None,
    password: Optional[str] = None,
) -> Optional[SrvControl]:
    """Register a TUI consumer session, or return ``None`` on any failure."""
    try:
        from nvoc_srv_client import (
            PRIORITY_DESKTOP,
            Session,
            basic_auth_header,
        )
    except Exception:  # noqa: BLE001
        return None
    auth_header = basic_auth_header(username, password) if username else None
    try:
        session = Session.open(
            name=name,
            priority=PRIORITY_DESKTOP,
            port=port,
            auth_header=auth_header,
        )
    except Exception:  # noqa: BLE001 - registration failed → native only
        return None
    return SrvControl(session)
