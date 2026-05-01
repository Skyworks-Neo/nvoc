# AGENTS.md

This repository contains `nvoc-tui`, a Textual-based terminal UI for the external `nvoc-auto-optimizer` CLI.

## Project Map

- `nvoc_tui/app.py`: Main `Textual` application. Builds the UI, persists user choices, triggers background queries/actions, and routes button events to CLI commands.
- `nvoc_tui/cli.py`: Wrapper around the external CLI. Handles executable discovery, synchronous queries, long-running streamed actions, and cancellation.
- `nvoc_tui/config.py`: Loads and saves `nvoc_tui_config.json`, and can import older `nvoc_gui_config.json` data on first run.
- `nvoc_tui/models.py`: Dataclasses for persisted config, GPU metadata, cached state, and small shared models.
- `nvoc_tui/parsing.py`: Parses CLI output from `list`, `info`, `status`, and `get`, plus ASCII VF-curve rendering.
- `nvoc_tui/__main__.py`: Script entry point for `nvoc-tui`.
- `tests/`: Focused unit tests for config migration/persistence and parsing behavior.

## How The App Works

- `NVOCApp` owns the UI state and a `CliService` instance.
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
- Some workflows are chained in order with `_run_action_chain()`, especially autoscan prep/reset operations.

## Important State Boundaries

- Persisted config lives in `self.config_data` and should be saved through `self.config_store` after UI-driven changes.
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

- CHECK ../gui for GUI conterparts; CHECK ../auto-optimizer for CLI that you will call.
- Keep changes consistent with existing Textual patterns in `app.py`; this file currently centralizes most UI behavior.
- Prefer extending helper methods like `_autoscan_args()`, `_oc_args()`, `_limit_args()`, and `_fan_args()` instead of embedding command construction directly in button handlers.
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
- UI behavior in `app.py` is currently only lightly protected; be careful with regressions around widget IDs and saved settings.

## Common Safe Entry Points For Changes

- Add a new button/action:
  - create the widget in `compose()`
  - handle it in `on_button_pressed()`
  - build arguments in a helper method if the command is non-trivial
- Add a new persisted preference:
  - update models/config decode/save
  - initialize the widget from config
  - sync widget state back into config before saving
- Add a new dashboard metric:
  - parse it in `parsing.py`
  - store it in `self.cache`
  - render it in `_update_metrics()`

## Known Risks

- `app.py` is doing a lot of orchestration already, so feature additions can easily increase coupling.
- Widget IDs are effectively part of the internal contract; renaming them will break event handlers and sync helpers.
- The app assumes the external CLI is trustworthy and available; many failures surface as logged output rather than structured errors.
