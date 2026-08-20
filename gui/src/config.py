"""
Configuration management - persists user settings to JSON.
"""

import json
import os
import threading
import time
from typing import Any, Dict, List, Optional, Union

JSONPrimitive = Optional[Union[str, int, float, bool]]
JSONValue = Union[JSONPrimitive, Dict[str, Any], List[Any]]

DEFAULT_CONFIG: Dict[str, JSONValue] = {
    "cli_exe_path": "",  # Auto-detected or user-specified
    "last_gpu_id": "",
    "last_gpu_idx": "",
    "autoscan": {
        "mode": "standard",  # standard / ultrafast / legacy
        "output_csv": "./ws/vfp-tem.csv",
        "init_csv": "./ws/vfp-init.csv",
        "bsod_recovery": "",
    },
}

CONFIG_FILE = "nvoc_gui_config.json"


class Config:
    """Simple JSON-based config store with debounced writes.

    set() only marks the data dirty; the disk write (mkstemp + replace, a
    synchronous stall on slow disks / AV-hooked file ops) is deferred to a
    background thread and coalesced, so bursts of set() (e.g. GPU switch
    writing two keys) cost one write instead of two UI-thread stalls.
    """

    _FLUSH_DELAY_S = 0.5

    def __init__(self, config_dir: str) -> None:
        self.path = os.path.join(config_dir, CONFIG_FILE)
        self.data: Dict[str, JSONValue] = {}
        self._dirty = False
        self._closed = False
        self._lock = threading.Lock()
        self._flush_requested = threading.Event()
        self._flusher = threading.Thread(
            target=self._flush_loop, name="config-flush", daemon=True
        )
        self._flusher.start()
        self.load()

    def load(self) -> None:
        if os.path.exists(self.path):
            try:
                with open(self.path, "r", encoding="utf-8") as f:
                    self.data = json.load(f)
            except (json.JSONDecodeError, IOError):
                self.data = {}
        # Merge defaults for any missing keys
        self._merge_defaults(self.data, DEFAULT_CONFIG)

    def save(self) -> None:
        import sys
        import tempfile

        dir_ = os.path.dirname(self.path) or "."
        try:
            fd, tmp = tempfile.mkstemp(dir=dir_, prefix=".nvoc_cfg-", suffix=".tmp")
            try:
                with os.fdopen(fd, "w", encoding="utf-8") as f:
                    json.dump(self.data, f, indent=2, ensure_ascii=False)
                # Restrict config to owner-only before it lands at the final path.
                # Windows uses ACLs; os.chmod is a no-op there for 0o600, skip it
                # to avoid triggering antivirus hooks on the temp file.
                if sys.platform != "win32":
                    os.chmod(tmp, 0o600)
                os.replace(tmp, self.path)
            except Exception:
                try:
                    os.unlink(tmp)
                except OSError:
                    pass
                raise
        except IOError:
            pass

    def get(self, key: str, default: Any = None) -> Any:
        return self.data.get(key, default)

    def set(self, key: str, value: JSONValue) -> None:
        with self._lock:
            self.data[key] = value
            self._dirty = True
        self._flush_requested.set()

    def _flush_loop(self) -> None:
        while True:
            self._flush_requested.wait()
            time.sleep(self._FLUSH_DELAY_S)  # coalesce bursts into one write
            self._flush_requested.clear()
            with self._lock:
                if not self._dirty or self._closed:
                    continue
                snapshot = json.loads(json.dumps(self.data))
                self._dirty = False
            self._write_snapshot(snapshot)
            if self._closed:
                return

    def _write_snapshot(self, snapshot: Dict[str, JSONValue]) -> None:
        import sys
        import tempfile

        dir_ = os.path.dirname(self.path) or "."
        try:
            fd, tmp = tempfile.mkstemp(dir=dir_, prefix=".nvoc_cfg-", suffix=".tmp")
            try:
                with os.fdopen(fd, "w", encoding="utf-8") as f:
                    json.dump(snapshot, f, indent=2, ensure_ascii=False)
                # Restrict config to owner-only before it lands at the final path.
                # Windows uses ACLs; os.chmod is a no-op there for 0o600, skip it
                # to avoid triggering antivirus hooks on the temp file.
                if sys.platform != "win32":
                    os.chmod(tmp, 0o600)
                os.replace(tmp, self.path)
            except Exception:
                try:
                    os.unlink(tmp)
                except OSError:
                    pass
                raise
        except IOError:
            pass

    def flush(self) -> None:
        """Force any pending dirty data to disk synchronously (shutdown path)."""
        with self._lock:
            if not self._dirty or self._closed:
                return
            snapshot = json.loads(json.dumps(self.data))
            self._dirty = False
        self._write_snapshot(snapshot)

    def close(self) -> None:
        """Flush pending writes and stop the background flusher."""
        self.flush()
        self._closed = True
        self._flush_requested.set()

    def _merge_defaults(self, target: Dict[str, Any], defaults: Dict[str, Any]) -> None:
        for k, v in defaults.items():
            if k not in target:
                target[k] = v
            elif isinstance(v, dict) and isinstance(target.get(k), dict):
                self._merge_defaults(target[k], v)
