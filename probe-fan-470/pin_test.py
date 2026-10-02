"""Live pin/unpin test for the GUI/TUI percent fallback (472.12).

Usage: python pin_test.py <backend> <percent|auto>
Runs pynvoc.set_fan the same way gui/src/backend/native.py and
tui/nvoc_tui/controllers/overclock.py do.
"""
import sys

import pynvoc

backend = sys.argv[1]
level = sys.argv[2]
if level == "auto":
    pynvoc.set_fan("0", backend, "all", "auto", 0)
    print(f"set_fan({backend!r}, auto) ok")
else:
    pynvoc.set_fan("0", backend, "all", "manual", int(level))
    print(f"set_fan({backend!r}, manual, {level}%) ok")
