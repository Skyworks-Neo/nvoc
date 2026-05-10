"""
Cross-platform single-instance guard for NVOC-GUI.

On Windows  : named mutex + named event (kernel objects, zero-dependency).
On POSIX    : Unix-domain socket in XDG_RUNTIME_DIR (or $TMPDIR) — the server
              half binds the socket to claim ownership; the client half connects
              to signal the running instance.
"""

import os
import sys

# ---------------------------------------------------------------------------
# Windows implementation
# ---------------------------------------------------------------------------

if sys.platform == "win32":
    import ctypes
    from ctypes import wintypes

    ERROR_ALREADY_EXISTS = 183
    MUTEX_NAME = r"Local\NVOC-GUI-SingleInstance-{7A0B671E-89A0-4E3B-8AA9-25A9C39C4D11}"
    EVENT_NAME = r"Local\NVOC-GUI-Activate-{7A0B671E-89A0-4E3B-8AA9-25A9C39C4D11}"
    EVENT_MODIFY_STATE = 0x0002
    SYNCHRONIZE = 0x00100000
    WAIT_OBJECT_0 = 0x00000000
    WAIT_TIMEOUT = 0x00000102

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

    kernel32.CreateMutexW.argtypes = (wintypes.LPVOID, wintypes.BOOL, wintypes.LPCWSTR)
    kernel32.CreateMutexW.restype = wintypes.HANDLE
    kernel32.CreateEventW.argtypes = (
        wintypes.LPVOID,
        wintypes.BOOL,
        wintypes.BOOL,
        wintypes.LPCWSTR,
    )
    kernel32.CreateEventW.restype = wintypes.HANDLE
    kernel32.OpenEventW.argtypes = (wintypes.DWORD, wintypes.BOOL, wintypes.LPCWSTR)
    kernel32.OpenEventW.restype = wintypes.HANDLE
    kernel32.SetEvent.argtypes = (wintypes.HANDLE,)
    kernel32.SetEvent.restype = wintypes.BOOL
    kernel32.ResetEvent.argtypes = (wintypes.HANDLE,)
    kernel32.ResetEvent.restype = wintypes.BOOL
    kernel32.WaitForSingleObject.argtypes = (wintypes.HANDLE, wintypes.DWORD)
    kernel32.WaitForSingleObject.restype = wintypes.DWORD
    kernel32.ReleaseMutex.argtypes = (wintypes.HANDLE,)
    kernel32.ReleaseMutex.restype = wintypes.BOOL
    kernel32.CloseHandle.argtypes = (wintypes.HANDLE,)
    kernel32.CloseHandle.restype = wintypes.BOOL

    class SingleInstanceGuard:
        """Prevent multiple NVOC-GUI processes from running at once (Windows)."""

        def __init__(self, mutex_name: str = MUTEX_NAME):
            self.mutex_name = mutex_name
            self.event_name = EVENT_NAME
            self._handle = None  # type: Optional[int]
            self._event_handle = None  # type: Optional[int]
            self._owns_mutex = False

        def acquire(self) -> bool:
            """Return True when this process is the primary instance."""
            ctypes.set_last_error(0)
            handle = kernel32.CreateMutexW(None, True, self.mutex_name)
            if not handle:
                raise ctypes.WinError(ctypes.get_last_error())

            self._handle = handle
            last_error = ctypes.get_last_error()
            self._owns_mutex = last_error != ERROR_ALREADY_EXISTS

            if not self._owns_mutex:
                self.release()
                return False

            self._event_handle = kernel32.CreateEventW(None, True, False, self.event_name)
            if not self._event_handle:
                error = ctypes.get_last_error()
                self.release()
                raise ctypes.WinError(error)

            return True

        def signal_existing_instance(self) -> bool:
            """Ask the running instance to restore and focus its window."""
            event_handle = kernel32.OpenEventW(EVENT_MODIFY_STATE, False, self.event_name)
            if not event_handle:
                return False

            try:
                return bool(kernel32.SetEvent(event_handle))
            finally:
                kernel32.CloseHandle(event_handle)

        def consume_activation_request(self) -> bool:
            """Return True when another process has requested activation."""
            if self._event_handle is None:
                return False

            wait_result = kernel32.WaitForSingleObject(self._event_handle, 0)
            if wait_result == WAIT_OBJECT_0:
                kernel32.ResetEvent(self._event_handle)
                return True

            if wait_result == WAIT_TIMEOUT:
                return False

            raise ctypes.WinError(ctypes.get_last_error())

        def release(self):
            """Release the mutex handle if this process owns it."""
            if self._event_handle is not None:
                kernel32.CloseHandle(self._event_handle)
                self._event_handle = None

            if self._handle is None:
                return

            if self._owns_mutex:
                kernel32.ReleaseMutex(self._handle)
                self._owns_mutex = False

            kernel32.CloseHandle(self._handle)
            self._handle = None

# ---------------------------------------------------------------------------
# POSIX implementation
# ---------------------------------------------------------------------------

else:
    import errno
    import select
    import socket
    import tempfile

    def _socket_path() -> str:
        """Compute a per-user socket path inside XDG_RUNTIME_DIR or $TMPDIR."""
        runtime = os.environ.get("XDG_RUNTIME_DIR") or tempfile.gettempdir()
        return os.path.join(runtime, f"nvoc-gui-{os.getuid()}.sock")

    class SingleInstanceGuard:
        """Prevent multiple NVOC-GUI processes from running at once (POSIX).

        The primary instance binds a Unix-domain socket and listens on it.
        A duplicate launch connects to that socket, sends a one-byte signal,
        and exits.  The primary instance polls the socket non-blockingly via
        ``consume_activation_request()`` on each GUI tick.
        """

        def __init__(self) -> None:
            self._sock_path = _socket_path()
            self._server: "socket.socket | None" = None

        def acquire(self) -> bool:
            """Return True when this process is the primary instance."""
            server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                server.bind(self._sock_path)
            except OSError as exc:
                server.close()
                if exc.errno != errno.EADDRINUSE:
                    raise
                # Socket file exists: probe whether the owner is still alive.
                probe = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                try:
                    probe.settimeout(0.5)
                    probe.connect(self._sock_path)
                    probe.close()
                    return False  # live primary instance found
                except OSError:
                    probe.close()
                # Stale socket from a crashed previous run — clean up and retry.
                try:
                    os.unlink(self._sock_path)
                except OSError:
                    pass
                server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                server.bind(self._sock_path)

            server.listen(8)
            server.setblocking(False)
            self._server = server
            return True

        def signal_existing_instance(self) -> bool:
            """Send an activation byte to the running primary instance."""
            try:
                s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                s.settimeout(1.0)
                s.connect(self._sock_path)
                s.sendall(b"\x01")
                s.close()
                return True
            except OSError:
                return False

        def consume_activation_request(self) -> bool:
            """Return True if a duplicate launch has signalled us (non-blocking)."""
            if self._server is None:
                return False
            try:
                readable, _, _ = select.select([self._server], [], [], 0)
                if not readable:
                    return False
                conn, _ = self._server.accept()
                try:
                    conn.settimeout(0.1)
                    data = conn.recv(1)
                    return data == b"\x01"
                finally:
                    conn.close()
            except OSError:
                return False

        def release(self) -> None:
            """Close the server socket and remove the socket file."""
            if self._server is not None:
                self._server.close()
                self._server = None
                try:
                    os.unlink(self._sock_path)
                except OSError:
                    pass
