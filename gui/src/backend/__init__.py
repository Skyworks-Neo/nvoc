"""Backend adapters for GUI operations."""

from src.backend.base import FanSettings
from src.backend.native import NativeBackend, apply_native_paths

__all__ = ["FanSettings", "NativeBackend", "apply_native_paths"]
