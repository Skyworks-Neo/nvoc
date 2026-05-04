from __future__ import annotations

from rich.text import Text
from textual import events
from textual.widgets import Input


def mnemonic_text(letter: str, after: str, before: str = "") -> Text:
    return Text.assemble(before, (letter, "underline"), after)


class ShortcutInput(Input):
    async def _on_key(self, event: events.Key) -> None:
        if self.app.consume_alt_prefix_key(event.key):
            event.stop()
            event.prevent_default()
            return
        await super()._on_key(event)
