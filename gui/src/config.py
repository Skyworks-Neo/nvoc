"""
Configuration management - persists user settings to JSON.
"""

import json
import os
from typing import Any, Dict, List, Optional, Union

JSONPrimitive = Optional[Union[str, int, float, bool]]
JSONValue = Union[JSONPrimitive, Dict[str, Any], List[Any]]

DEFAULT_CONFIG: Dict[str, JSONValue] = {
    "cli_exe_path": "",  # Auto-detected or user-specified
    "last_gpu_id": "",
    "last_gpu_idx": "",
    "autoscan": {
        "mode": "standard",  # standard / ultrafast / legacy
        "test_exe": r".\test\test.bat",
        "score_threshold": "98.5",
        "timeout_loops": "30",
        "output_csv": r".\ws\vfp-tem.csv",
        "init_csv": r".\ws\vfp-init.csv",
        "log_file": r".\ws\vfp.log",
        "score_path": r"..\yiji_tb\result.xml",
        "bsod_recovery": "",
    },
}

CONFIG_FILE = "nvoc_gui_config.json"


class Config:
    """Simple JSON-based config store."""

    def __init__(self, config_dir: str) -> None:
        self.path = os.path.join(config_dir, CONFIG_FILE)
        self.data: Dict[str, JSONValue] = {}
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
        try:
            with open(self.path, "w", encoding="utf-8") as f:
                json.dump(self.data, f, indent=2, ensure_ascii=False)
        except IOError:
            pass

    def get(self, key: str, default: Any = None) -> Any:
        return self.data.get(key, default)

    def set(self, key: str, value: JSONValue) -> None:
        self.data[key] = value
        self.save()

    def _merge_defaults(self, target: Dict[str, Any], defaults: Dict[str, Any]) -> None:
        for k, v in defaults.items():
            if k not in target:
                target[k] = v
            elif isinstance(v, dict) and isinstance(target.get(k), dict):
                self._merge_defaults(target[k], v)
