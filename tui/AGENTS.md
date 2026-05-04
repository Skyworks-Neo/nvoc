# AGENTS.md

This repository contains `nvoc-tui`, a Textual-based terminal UI for the external `nvoc-auto-optimizer` CLI.

## Project Map

- `nvoc_tui/app.py`: Main `Textual` application shell. Owns shared runtime state, starts background queries/actions, wires pane controllers, and delegates UI events.
- `nvoc_tui/panes/`: Textual composition modules. Each file builds one visible area: header, dashboard, autoscan, overclock, VF curve, or console.
- `nvoc_tui/controllers/`: Pane behavior modules. Controllers handle pane-specific button events, UI-to-config sync, command argument construction, and rendering updates.
- `nvoc_tui/styles/`: Split Textual CSS (`*.tcss`) loaded through `NVOCApp.CSS_PATH`. Keep pane-specific selectors in the matching style file and shared selectors in `base.tcss`.
- `nvoc_tui/cli.py`: Wrapper around the external CLI. Handles executable discovery, synchronous queries, long-running streamed actions, and cancellation.
- `nvoc_tui/config.py`: Loads and saves `nvoc_tui_config.json`, and can import older `nvoc_gui_config.json` data on first run.
- `nvoc_tui/models.py`: Dataclasses for persisted config, GPU metadata, cached state, and small shared models.
- `nvoc_tui/parsing.py`: Parses CLI output from `list`, `info`, `status`, and `get`, plus ASCII VF-curve rendering.
- `nvoc_tui/__main__.py`: Script entry point for `nvoc-tui`.
- `tests/`: Focused unit tests for config migration/persistence, parsing behavior, controller command construction, and app layout smoke coverage.

## How The App Works

- `NVOCApp` owns shared app state, a `CliService` instance, and controller instances for each pane.
- Pane composition is intentionally separate from behavior:
  - `nvoc_tui/panes/*.py` creates widgets and preserves widget IDs.
  - `nvoc_tui/controllers/*.py` reads widgets, persists settings, builds CLI args, and updates pane output.
- Startup flow in `on_mount()`:
  - write a startup log line
  - update the metrics panel with placeholder/cache data
  - detect GPUs
  - start the dashboard polling timer
- GPU state refresh uses three background queries:
  - `info`
  - `status -a`
  - `get`
- Long-running operations use `CliService.run_action()` so output can stream into the log widget while the UI remains responsive.
- Some workflows are chained in order with `NVOCApp.run_action_chain()`, especially autoscan prep/reset operations.
- Cross-pane updates should stay explicit through controller calls. For example, dashboard query results update metrics, prime overclock inputs, and may trigger VF plot rendering.

## Important State Boundaries

- Persisted config lives in `self.config_data` and should be saved through `self.config_store` after UI-driven changes.
- Use `NVOCApp.save_config()` rather than repeating `config_store.data = ...` and `config_store.save()` in controllers.
- Live device state is cached in `self.cache`:
  - `info`
  - `status`
  - `settings`
  - `vf_curve_path`
- GPU selection is derived from `#gpu-select`; command helpers should go through `gpu_args()` instead of re-implementing `--gpu=...`.
- VF curve exports are cached under `vfp_cache/` in the repo root, keyed by GPU UUID when available.

## Concurrency Notes

- Query commands run in background threads and marshal results back with `call_from_thread(...)`.
- Streaming actions use a worker thread plus `subprocess.Popen`.
- `CliService.action_state.running` prevents overlapping mutating actions.
- When changing threaded code, preserve the current pattern of doing subprocess work off the UI thread and updating widgets on the app thread.

## CLI Integration Assumptions

- The UI depends on an external executable and does not implement GPU logic itself.
- CLI discovery currently checks:
  - saved path
  - `nvoc-autooptimizer` on `PATH`
  - `nvoc-auto-optimizer` on `PATH`
  - a sibling `../auto-optimizer/target/release/...` build
- Query parsing is intentionally tolerant because CLI output may be plain text or JSON.

## Editing Guidance

- CHECK ../gui for GUI counterparts; CHECK ../auto-optimizer for CLI that you will call.
- Keep `app.py` as the shell. Do not move pane composition or pane-specific button logic back into it.
- Add widgets in the appropriate `nvoc_tui/panes/<pane>.py` file, handle their behavior in `nvoc_tui/controllers/<pane>.py`, and place pane-specific CSS in `nvoc_tui/styles/<pane>.tcss`.
- Preserve widget IDs unless you update every controller, test, and saved-setting sync path that depends on them.
- Prefer extending controller helper methods like `AutoscanController.autoscan_args()`, `OverclockController.oc_args()`, `OverclockController.limit_args()`, and `OverclockController.fan_args()` instead of embedding command construction directly in button handlers.
- Shared UI/runtime operations should remain on `NVOCApp` only when multiple controllers need them, such as `gpu_args()`, `run_action()`, `run_action_chain()`, `run_query()`, `refresh_all_state()`, and `write_log()`.
- If you add new persisted fields, update:
  - the dataclass in `models.py`
  - `ConfigStore.save()`
  - `ConfigStore._decode()`
  - GUI migration logic in `ConfigStore._decode_from_gui()` when relevant
- If you add new CLI query modes, keep parsing logic in `parsing.py` and normalization in `normalize_query_output()`.
- Avoid blocking calls on the Textual UI thread.

## Testing Guidance

- Use uv to run the program or any auxiliary scripts.
- Existing tests are lightweight unit tests, not integration tests.
- Prefer adding parsing/config tests when behavior changes in `parsing.py` or `config.py`.
- Prefer adding controller tests when command argument construction or button behavior changes.
- Keep the app layout smoke test updated when adding or removing important widget IDs.
- Run `uv run pytest` before finishing changes.

## Common Safe Entry Points For Changes

- Add a new button/action:
  - create the widget in the matching `nvoc_tui/panes/` compose function
  - dispatch it from the matching controller `handle_button()`
  - build arguments in a controller helper method if the command is non-trivial
  - add or update focused controller tests for the argument shape
- Add a new persisted preference:
  - update models/config decode/save
  - initialize the widget from config
  - sync widget state back into config in the matching controller before saving
- Add a new dashboard metric:
  - parse it in `parsing.py`
  - store it in `self.cache`
  - render it in `DashboardController.update_metrics()`
- Add or change CSS:
  - put shared layout rules in `styles/base.tcss`
  - put pane-specific rules in the matching `styles/<pane>.tcss`
  - update `pyproject.toml` package data if adding a new style directory or pattern

## Known Risks

- Cross-pane behavior can become coupled if controllers reach into each other too casually; keep shared operations on `NVOCApp` and make cross-pane updates explicit.
- Widget IDs are effectively part of the internal contract; renaming them will break event handlers and sync helpers.
- TCSS files must remain package data, otherwise installed builds can fail to load styles.
- The app assumes the external CLI is trustworthy and available; many failures surface as logged output rather than structured errors.
