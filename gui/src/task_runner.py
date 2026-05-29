"""Small background task runner for GUI work.

All callbacks that touch CustomTkinter widgets must still be marshalled back
through ``App.after`` by the caller.
"""

from __future__ import annotations

from collections.abc import Callable
from concurrent.futures import Future, ThreadPoolExecutor
from typing import TypeVar


T = TypeVar("T")


class GuiTaskRunner:
    """Central executor for GUI background work."""

    def __init__(self, max_workers: int = 8) -> None:
        self._executor = ThreadPoolExecutor(
            max_workers=max_workers,
            thread_name_prefix="nvoc-gui",
        )

    def submit(self, name: str, task: Callable[[], T]) -> Future[T]:
        return self._executor.submit(task)

    def shutdown(self) -> None:
        self._executor.shutdown(wait=False, cancel_futures=True)
