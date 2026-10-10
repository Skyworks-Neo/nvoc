"""Headless live TUI probe: mount NVOCApp against the real GPUs, let the
dashboard poll a few cycles, then dump the rendered text of the metrics panel
and the GPU header. Diagnostic only — not a pytest (needs live GPUs).
Run: cd tui && uv run python live_tui_probe.py
"""

import asyncio
import sys

from nvoc_tui.app import NVOCApp


def dump_screen(app) -> str:
    # textual 8.x: the compositor yields strips per visible line
    lines = []
    for strip in app.screen._compositor.render_strips():
        lines.append("".join(seg.text for seg in strip))
    return "\n".join(lines)


async def main() -> int:
    app = NVOCApp()
    async with app.run_test(size=(170, 50)) as pilot:
        await pilot.pause()
        # let the dashboard poll + the startup info query land
        for _ in range(12):
            await asyncio.sleep(1)
            await pilot.pause()
        text = dump_screen(app)
    print(text)
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
