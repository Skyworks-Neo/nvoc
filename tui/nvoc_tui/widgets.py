from __future__ import annotations

from rich.text import Text
from textual import events
from textual.widgets import Button, Input


def mnemonic_text(letter: str, after: str, before: str = "") -> Text:
    return Text.assemble(before, (letter, "underline"), after)


class ShortcutInput(Input):
    async def _on_key(self, event: events.Key) -> None:
        if self.app.consume_alt_prefix_key(event.key):
            event.stop()
            event.prevent_default()
            return
        await super()._on_key(event)


class UnitToggle(Button):
    """3-col MHz/mV plane chip for the overclock offset rows.

    line_pad is zeroed here because TCSS rejects ``line-pad: 0`` (its
    integers must be > 0); the Button default of 1 pads the chip to 5
    columns wide / 3 rows tall and breaks the offset rows' original+2
    width budget — see the .unit-toggle rules in overclock.tcss.
    """

    def on_mount(self) -> None:
        self.styles.line_pad = 0
