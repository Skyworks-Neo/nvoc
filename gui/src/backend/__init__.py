"""Backend adapters for GUI operations."""

from src.backend.base import FanSettings
from src.backend.cli import CliBackend
from src.backend.native import NativeBackend

__all__ = ["CliBackend", "FanSettings", "NativeBackend"]
