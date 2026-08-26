"""Compatibility import for the refactored fan control pane."""

from src.panes.fan_control import FanControlPane

FanControlTab = FanControlPane

__all__ = ["FanControlPane", "FanControlTab"]
