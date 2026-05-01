"""
Windows single-instance guard for NVOC-GUI.
"""

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
    """Prevent multiple NVOC-GUI processes from running at once."""

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
