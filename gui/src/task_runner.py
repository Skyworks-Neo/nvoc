"""Small background task runner for GUI work.

All callbacks that touch CustomTkinter widgets must still be marshalled back
through ``App.after`` by the caller.
"""

from __future__ import annotations

from collections.abc import Callable
from concurrent.futures import Future
from queue import Empty, Queue
from threading import Lock, Thread
from typing import TypeVar


T = TypeVar("T")
TaskItem = tuple[str, Callable[[], object], Future[object]]


class GuiTaskRunner:
    """Central executor for GUI background work."""

    def __init__(self, max_workers: int = 8) -> None:
        self._queue: Queue[TaskItem | None] = Queue()
        self._lock = Lock()
        self._shutdown = False
        self._workers: list[Thread] = []
        for index in range(max_workers):
            worker = Thread(target=self._work, name=f"nvoc-gui_{index}")
            worker.daemon = True
            worker.start()
            self._workers.append(worker)

    def submit(self, name: str, task: Callable[[], T]) -> Future[T]:
        future: Future[T] = Future()
        with self._lock:
            if self._shutdown:
                raise RuntimeError("cannot schedule new work after shutdown")
            self._queue.put((name, task, future))  # type: ignore[arg-type]
        return future

    def shutdown(self, wait: bool = False, cancel_futures: bool = True) -> None:
        with self._lock:
            if self._shutdown:
                return
            self._shutdown = True
            if cancel_futures:
                self._cancel_pending()
            for _worker in self._workers:
                self._queue.put(None)

        if wait:
            for worker in self._workers:
                worker.join()

    def _cancel_pending(self) -> None:
        while True:
            try:
                item = self._queue.get_nowait()
            except Empty:
                return
            try:
                if item is not None:
                    _name, _task, future = item
                    future.cancel()
            finally:
                self._queue.task_done()

    def _work(self) -> None:
        while True:
            item = self._queue.get()
            try:
                if item is None:
                    return
                _name, task, future = item
                if not future.set_running_or_notify_cancel():
                    continue
                try:
                    result = task()
                except BaseException as exc:
                    future.set_exception(exc)
                else:
                    future.set_result(result)
            finally:
                self._queue.task_done()
