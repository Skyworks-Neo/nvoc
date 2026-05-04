from __future__ import annotations

from textual import events
from textual.widgets import Input


class ShortcutInput(Input):
    async def _on_key(self, event: events.Key) -> None:
        if self.app.consume_alt_prefix_key(event.key):
            event.stop()
            event.prevent_default()
            return
        await super()._on_key(event)
