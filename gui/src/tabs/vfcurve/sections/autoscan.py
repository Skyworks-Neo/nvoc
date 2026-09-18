"""Submit and monitor srv-owned optimization; closing the GUI only detaches."""

from __future__ import annotations

import json
import tkinter as tk
import uuid
from tkinter import filedialog

import customtkinter as ctk

from src.widgets.lightweight_controls import LiteButton, install_mousewheel_support


TERMINAL = {"succeeded", "cancelled", "failed", "interrupted"}


class AutoscanTab:
    def __init__(self, parent, app) -> None:
        self.app = app
        self.frame = parent
        self._task_id = None
        self._tasks = {}
        self._offset = 0
        self._refreshing = False
        self._acting = False
        self._is_resize_active = False
        self._pending_scan_button_state = None

        scroll = ctk.CTkScrollableFrame(parent)
        scroll.pack(fill="both", expand=True, padx=10, pady=10)
        install_mousewheel_support(scroll)
        ctk.CTkLabel(
            scroll, text="Service optimization", font=("Segoe UI", 14, "bold")
        ).pack(anchor="w")
        ctk.CTkLabel(
            scroll,
            text="The service runs the full optimization workflow and saves its files.\n"
            "Closing this window does not stop a task. Use Stop to cancel and reset scan settings.",
            justify="left",
        ).pack(anchor="w", pady=5)

        modes = ctk.CTkFrame(scroll)
        modes.pack(fill="x", pady=5)
        self.mode_var = tk.StringVar(value="Standard")
        ctk.CTkOptionMenu(
            modes, variable=self.mode_var, values=["Standard", "Ultrafast", "Legacy"]
        ).pack(side="left", padx=5)
        self.start_btn = LiteButton(
            modes, text="Start optimization", command=self._start_scan
        )
        self.start_btn.pack(side="left", padx=5)
        self.stop_btn = LiteButton(modes, text="Stop task", command=self._stop_scan)
        self.stop_btn.pack(side="left", padx=5)

        row = ctk.CTkFrame(scroll)
        row.pack(fill="x", pady=5)
        self.task_var = tk.StringVar(value="No tasks")
        self.task_menu = ctk.CTkOptionMenu(
            row,
            variable=self.task_var,
            values=["No tasks"],
            command=self._select_task,
            width=280,
        )
        self.task_menu.pack(side="left", padx=5)
        LiteButton(row, text="Refresh", command=self._refresh).pack(side="left", padx=5)
        LiteButton(row, text="Save result", command=self._save_result).pack(
            side="left", padx=5
        )

        controls = ctk.CTkFrame(scroll)
        controls.pack(fill="x", pady=5)
        LiteButton(
            controls,
            text="Stop automation & take over",
            command=lambda: self._control("takeover"),
        ).pack(side="left", padx=5)
        LiteButton(
            controls, text="Allow automation", command=lambda: self._control("release")
        ).pack(side="left", padx=5)
        LiteButton(
            controls, text="Retry recovery", command=lambda: self._control("recover")
        ).pack(side="left", padx=5)
        self.status_var = tk.StringVar(value="Connecting to scan service…")
        ctk.CTkLabel(
            scroll, textvariable=self.status_var, justify="left", wraplength=750
        ).pack(anchor="w", pady=5)
        self.frame.after(100, self._poll)

    @staticmethod
    def _client():
        from pynvoc.scan_client import ScanClient

        return ScanClient()

    def _alive(self):
        return not getattr(self.app, "_exiting", False)

    def _background(self, action, complete, failed=None):
        """Poll futures on Tk's thread; worker functions never touch widgets."""
        future = self.app.run_background("hosted-scan", action)

        def deliver():
            if not self._alive():
                return
            if not future.done():
                self.frame.after(100, deliver)
                return
            try:
                value = future.result()
            except Exception as exc:
                self.status_var.set(str(exc))
                if failed:
                    failed(exc)
            else:
                complete(value)

        self.frame.after(100, deliver)

    def _selected_gpu(self):
        # Do not use get_gpu_args' display-index fallback as a stable GPU ID.
        value = self.app.gpu_map.get(self.app.gpu_var.get())
        if value is None:
            raise ValueError("Select a detected GPU before starting a task")
        text = str(value)
        return int(text, 16 if text.lower().startswith("0x") else 10)

    def _control_gpu(self):
        task = self._tasks.get(self._task_id)
        return task["request"]["gpu_id"] if task else self._selected_gpu()

    def _start_scan(self):
        if self._acting:
            return
        try:
            pending = self.app.config.get("hosted_scan_pending")
            if not pending:
                pending = {
                    "gpu_id": self._selected_gpu(),
                    "mode": self.mode_var.get().lower(),
                    "request_id": uuid.uuid4().hex,
                }
                # Persist before sending so a lost reply/restart can reuse the ID.
                self.app.config.set("hosted_scan_pending", pending)
        except (ValueError, TypeError) as exc:
            self.status_var.set(str(exc))
            return
        self._acting = True
        self.start_btn.configure(state="disabled")
        self.status_var.set("Submitting optimization…")

        def accepted(task):
            self._acting = False
            self.app.config.set("hosted_scan_pending", None)
            self._task_id = task["id"]
            self._offset = 0
            self._tasks[self._task_id] = task
            self.status_var.set(f"Task {task['id']} accepted by the service")
            self._refresh()

        def failed(exc):
            self._acting = False
            self.start_btn.configure(state="normal")
            # A definite rejection is safe to replace. Network failures preserve
            # the pending payload; pressing Start retries exactly that submission.
            if getattr(exc, "status", 0) in {400, 401, 403, 404, 409}:
                self.app.config.set("hosted_scan_pending", None)

        self._background(lambda: self._client().start(**pending), accepted, failed)

    def _action(self, operation):
        if self._acting:
            return
        self._acting = True

        def done(_value):
            self._acting = False
            self._refresh()

        def failed(_error):
            self._acting = False

        self._background(operation, done, failed)

    def _stop_scan(self):
        if self._task_id is not None:
            task_id = self._task_id
            self._action(lambda: self._client().cancel(task_id))

    def _control(self, action):
        try:
            gpu = self._control_gpu()
        except ValueError as exc:
            self.status_var.set(str(exc))
            return
        self._action(lambda: getattr(self._client(), action)(gpu))

    def _select_task(self, label):
        try:
            task_id = int(label.split(":", 1)[0])
        except ValueError:
            return
        if task_id != self._task_id:
            self._task_id = task_id
            self._offset = 0
        self._refresh()

    def _poll(self):
        if self._alive():
            self._refresh()
            self.frame.after(1500, self._poll)

    def _refresh(self):
        if self._refreshing or not self._alive():
            return
        self._refreshing = True
        selected, offset = self._task_id, self._offset

        def read():
            client = self._client()
            tasks = client.tasks()
            controls = client.control()
            log = client.log(selected, offset) if selected is not None else None
            return tasks, controls, log

        def failed(_error):
            self._refreshing = False

        def show(snapshot):
            self._refreshing = False
            tasks, controls, log = snapshot
            self._tasks = {task["id"]: task for task in tasks}
            if self._task_id is None and tasks:
                active = [t for t in tasks if t["state"] not in TERMINAL]
                self._task_id = (active or tasks)[-1]["id"]
            pending = self.app.config.get("hosted_scan_pending")
            if pending:
                recovered = next(
                    (
                        t
                        for t in tasks
                        if t["origin"] == "manual" and t["request"] == pending
                    ),
                    None,
                )
                if recovered:
                    self.app.config.set("hosted_scan_pending", None)
                    if self._task_id != recovered["id"]:
                        self._task_id = recovered["id"]
                        self._offset = 0
            labels = {
                t[
                    "id"
                ]: f"{t['id']}: GPU {t['request']['gpu_id']} · {t['origin']} · {t['state']}"
                for t in tasks
            }
            self.task_menu.configure(
                values=list(reversed(list(labels.values()))) or ["No tasks"]
            )
            self.task_var.set(labels.get(self._task_id, "No tasks"))
            current = self._tasks.get(self._task_id)
            busy = any(t["state"] not in TERMINAL for t in tasks)
            self._set_scan_buttons(
                not busy and not self._acting,
                bool(current and current["state"] not in TERMINAL),
            )
            if current:
                gpu = current["request"]["gpu_id"]
                held = gpu in controls["manual_hold"]
                needs_recovery = gpu in controls["recovery_required"]
                self.status_var.set(
                    f"Task {current['id']}: {current['state']} · GPU {gpu}\n"
                    f"Automation: {'held by user' if held else 'allowed'}"
                    + (" · Recovery required" if needs_recovery else "")
                    + (f"\n{current['error']}" if current.get("error") else "")
                )
            else:
                self.status_var.set("Scan service connected. No tasks yet.")
            if log is not None and selected == self._task_id:
                self._offset = log["next_offset"]
                if log["text"]:
                    self.app.console.append(log["text"])

        self._background(read, show, failed)

    def _save_result(self):
        if self._task_id is None:
            return
        task_id = self._task_id

        def save(result):
            csv = result.get("final_csv")
            path = filedialog.asksaveasfilename(
                defaultextension=".csv" if csv is not None else ".json",
                initialfile=f"scan-{task_id}.csv"
                if csv is not None
                else f"scan-{task_id}.json",
            )
            if path:
                try:
                    with open(path, "w", encoding="utf-8", newline="") as output:
                        output.write(
                            csv
                            if csv is not None
                            else json.dumps(result, ensure_ascii=False, indent=2)
                        )
                except OSError as exc:
                    self.status_var.set(str(exc))

        self._background(lambda: self._client().result(task_id), save)

    def _set_scan_buttons(self, start_enabled, stop_enabled):
        self.start_btn.configure(state="normal" if start_enabled else "disabled")
        self.stop_btn.configure(state="normal" if stop_enabled else "disabled")

    def on_resize_state_changed(self, resizing, force_flush=False):
        self._is_resize_active = resizing
