"""GUI backend that drives the **resident nvoc-srv control plane**.

When a resident `nvoc-srv` is present, the GUI registers as a consumer in its
session registry (the same arbitration/lease machinery the auto-optimizer, the
CUDA stressor, and the MCP agent use) and routes the *arbitrated* control
operations — manual fan duty, reset, and the "hand the fan back to the driver"
cases — through a session claim. Everything else (reads, deep OC/VF writes)
still goes straight to pynvoc via the wrapped :class:`NativeBackend`; srv's
arbiter only governs the thermal/fan control loop, not the OC surface.

This is a consumer, not a second control plane: the GUI never spawns its own
srv. If none is running, :func:`connect` returns ``None`` and the app keeps the
plain native backend (today's behaviour).
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Callable, Optional, Tuple

from src.backend.base import FanSettings
from src.backend.native import NativeBackend

if TYPE_CHECKING:
    from src.app import App

CredentialProvider = Callable[[], Optional[Tuple[str, str]]]


class SrvBackend:
    """Wraps a :class:`NativeBackend`, routing arbitrated control to srv.

    ``__getattr__`` forwards every other method (reads, OC/VF writes) to the
    wrapped native backend unchanged, so only the control-class operations
    defined here differ.
    """

    def __init__(self, app: "App", session) -> None:
        self.app = app
        # Assign the delegate before anything can touch a missing attribute.
        self._native = NativeBackend(app)
        self._session = session

    # ── arbitrated control-class operations (through the srv registry) ──

    def apply_fan_settings(self, settings: FanSettings) -> None:
        if settings.policy == "manual":
            # The srv control loop owns the duty; a manual percent claim is the
            # direct equivalent of the GUI's manual fan policy.
            self._claim("manual", manual_percent=int(settings.level))
        else:
            # auto / curve / anything else is a *driver-side* fan behaviour; the
            # native write produces it and the srv must not fight it, so yield
            # the band back entirely rather than pinning it at auto.
            self._native.apply_fan_settings(settings)
            self._release()

    def reset_fan_settings(self, settings: FanSettings) -> None:
        self._native.reset_fan_settings(settings)
        self._release()

    def activate_fan_curve(self) -> None:
        self._native.activate_fan_curve()
        self._release()

    def _claim(self, mode: str, **kwargs) -> None:
        try:
            owner = self._session.claim(mode, **kwargs)
        except Exception as exc:  # noqa: BLE001 - surface, never crash the UI
            self.app.console.append(f"[GUI] srv claim failed: {exc}\n")
            return
        if not owner:
            self.app.console.append(
                f"[GUI] srv: '{mode}' registered, but a higher-priority consumer "
                "currently holds control.\n"
            )

    def _release(self) -> None:
        # Hand the band back so a waiting optimizer/MCP consumer can drive the
        # fan; the srv restores driver control when no claim remains. The
        # session stays open, so a later manual op re-claims immediately.
        try:
            self._session.release()
        except Exception as exc:  # noqa: BLE001 - surface, never crash the UI
            self.app.console.append(f"[GUI] srv release failed: {exc}\n")

    def shutdown(self) -> None:
        """Release the srv session (restores driver control) and native threads."""
        try:
            self._session.close()
        finally:
            self._native.shutdown()

    def __getattr__(self, name: str):
        # Everything not overridden above is direct-to-pynvoc, unchanged.
        return getattr(self._native, name)


def connect(
    app: "App",
    credential_provider: Optional[CredentialProvider] = None,
    port: int = 14514,
) -> Optional[SrvBackend]:
    """Attach to a resident srv and register a desktop consumer.

    Returns a :class:`SrvBackend` when a srv is reachable (prompting for OS
    credentials via ``credential_provider`` if it demands auth), else ``None``
    so the caller falls back to the plain native backend.
    """
    try:
        from nvoc_srv_client import (
            PRIORITY_DESKTOP,
            ProbeState,
            SrvClient,
            Session,
            basic_auth_header,
        )
    except Exception:  # noqa: BLE001 - client not installed → native only
        return None

    client = SrvClient(port=port)
    try:
        state = client.probe_state()
    except Exception:  # noqa: BLE001 - unknown occupant → native only
        return None

    if state is ProbeState.ABSENT:
        return None

    auth_header = None
    if state is ProbeState.AUTH_REQUIRED:
        creds = credential_provider() if credential_provider is not None else None
        if not creds:
            return None
        auth_header = basic_auth_header(creds[0], creds[1])

    try:
        session = Session.open(
            name="nvoc-gui",
            priority=PRIORITY_DESKTOP,
            port=port,
            auth_header=auth_header,
        )
    except Exception as exc:  # noqa: BLE001 - registration failed → native only
        app.console.append(f"[GUI] srv registration failed, using native: {exc}\n")
        return None

    app.console.append(
        f"[GUI] attached to resident nvoc-srv as a consumer "
        f"(session {session.id}, band {PRIORITY_DESKTOP}).\n"
    )
    return SrvBackend(app, session)
