"""
Configuration management - persists user settings to JSON.
"""

import json
import os
import threading
from typing import Any, Dict, List, Optional, Union

JSONPrimitive = Optional[Union[str, int, float, bool]]
JSONValue = Union[JSONPrimitive, Dict[str, Any], List[Any]]

DEFAULT_CONFIG: Dict[str, JSONValue] = {
    "cli_exe_path": "",  # Auto-detected or user-specified
    # Native library path overrides (env NVOC_NVAPI_PATH / NVOC_NVML_PATH).
    # 命令行参数 --nvapi-path/--nvml-path 优先于这里的持久化配置；两者都
    # 必须发生在首次 GPU 调用前（App 启动早期应用）。
    "nvapi_lib_path": "",
    "nvml_lib_path": "",
    "last_gpu_id": "",
    "last_gpu_idx": "",
    "stressor": {
        # Standalone cli-stressor-cuda-rs executable driven by the VF Curve
        # tab's stress-tool panel. Empty = auto-discovered (next to the
        # configured optimizer CLI, then PATH).
        "exe_path": "",
        # Every option below is forwarded only when non-empty/true, so the
        # tool's own defaults apply to blank fields.
        "profile": "(default)",  # (default) / standard / low-vram / 40-50 / dynamic-export
        "duration": "90",
        "precisions": "",
        "matrix_sizes": "",
        "kernel_types": "",
        "validate_interval": "",
        "validate_size": "",
        "seed": "",
        "stream_mode": "(default)",  # (default) / single / dual / triple
        "gpu_generate": False,
        "extra_args": "",
        # Verify/SDC family (enhanced stressor builds only; the panel greys
        # these out when the local executable does not advertise them).
        "no_verify": False,
        "verify_continue_on_error": False,
        "verify_resident_interval": "",
        "verify_slab": False,
        "gemm_full_check_max_size": "",
        "skip_self_test": False,
        "json_out": "",
        # Vulkan family (spellings differ between the two stressor
        # generations; the capability probe picks the right one).
        "vulkan": False,
        "vulkan_only": False,
        "vulkan_width": "",
        "vulkan_height": "",
        "vulkan_msaa": "",
        "vulkan_iters": "",
        "vulkan_shells": "",
        "vulkan_window": False,
        "vulkan_rotate": False,
        "vulkan_particles": False,
        "vulkan_offscreen": False,
    },
}

CONFIG_FILE = "nvoc_gui_config.json"


class Config:
    """JSON config store with coalesced background writes."""

    _FLUSH_DELAY_S = 0.5

    def __init__(self, config_dir: str) -> None:
        self.path = os.path.join(config_dir, CONFIG_FILE)
        self.data: Dict[str, JSONValue] = {}
        self._dirty = False
        self._closed = False
        self._lock = threading.Lock()
        self._write_lock = threading.Lock()
        self._flush_requested = threading.Event()
        self._stop_requested = threading.Event()
        self.load()
        self._flusher = threading.Thread(
            target=self._flush_loop,
            name="nvoc-gui-config-flush",
            daemon=True,
        )
        self._flusher.start()

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
        """Synchronously persist the current config."""
        with self._lock:
            snapshot = json.loads(json.dumps(self.data))
            self._dirty = False
            self._write_lock.acquire()
        try:
            self._write_snapshot(snapshot)
        finally:
            self._write_lock.release()

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

    def get(self, key: str, default: Any = None) -> Any:
        return self.data.get(key, default)

    def set(self, key: str, value: JSONValue) -> None:
        with self._lock:
            if self._closed:
                return
            self.data[key] = value
            self._dirty = True
        self._flush_requested.set()

    def _flush_loop(self) -> None:
        while True:
            self._flush_requested.wait()
            self._flush_requested.clear()
            if self._stop_requested.wait(self._FLUSH_DELAY_S):
                return
            with self._lock:
                if self._closed:
                    return
                if not self._dirty:
                    continue
                snapshot = json.loads(json.dumps(self.data))
                self._dirty = False
                self._write_lock.acquire()
            try:
                self._write_snapshot(snapshot)
            finally:
                self._write_lock.release()

    def flush(self) -> None:
        """Synchronously persist pending changes without closing the store."""
        with self._lock:
            if self._closed or not self._dirty:
                return
            snapshot = json.loads(json.dumps(self.data))
            self._dirty = False
            self._write_lock.acquire()
        try:
            self._write_snapshot(snapshot)
        finally:
            self._write_lock.release()

    def close(self) -> None:
        """Persist the last update and wait for the flusher to stop."""
        with self._lock:
            if self._closed:
                return
            snapshot = json.loads(json.dumps(self.data)) if self._dirty else None
            self._dirty = False
            self._closed = True
            if snapshot is not None:
                self._write_lock.acquire()
        if snapshot is not None:
            try:
                self._write_snapshot(snapshot)
            finally:
                self._write_lock.release()
        self._stop_requested.set()
        self._flush_requested.set()
        self._flusher.join()

    def _merge_defaults(self, target: Dict[str, Any], defaults: Dict[str, Any]) -> None:
        for k, v in defaults.items():
            if k not in target:
                target[k] = v
            elif isinstance(v, dict) and isinstance(target.get(k), dict):
                self._merge_defaults(target[k], v)
