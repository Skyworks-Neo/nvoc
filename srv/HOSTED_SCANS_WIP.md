# Hosted optimizer checkpoint — 2026-09-18

Status: implementation in progress, saved at user request. No commit, service installation, real GPU scan or GPU write was performed. Do not treat this as production-ready. All Rust edits recompiled and verified on 2026-09-18 (see "Verified 2026-09-18").

## Scope agreed with user

1. Unify ordinary GPU control APIs later.
2. Current work: srv owns optimizer launch, lifetime, logs, cancellation and process-tree cleanup; GUI submits hosted tasks. Manual takeover blocks automation until released.
3. Optimizer-only MCP mapping comes later and has not been started.

Do not change existing GUI/TUI ownership locks or ordinary pynvoc control paths. TUI currently has no scan UI. Direct driver callers still bypass hosted-task arbitration. Globally serialize hosted scans because optimizer recovery can reset the driver; manual automation holds are per GPU.

## Implemented files

- `srv/src/scans.rs`: persisted task registry, idempotent request IDs, admission lock, manual holds, cancel/recovery states, fail-closed journal errors, restart interruption handling.
- `srv/src/scan_process.rs`: Windows Job Object, suspended launch then assignment/resume, inherited-handle whitelist, full process-tree cleanup; injectable backend and worker panic handling. Cancellation/failure resets defaults, NOT a snapshot of previous settings. Successful optimization preserves results. `spawn` strips the verbatim `\\?\` cwd prefix before `CreateProcessW` (children created with a verbatim current directory hang at startup before running any code).
- `srv/src/scan_api.rs`: authenticated loopback HTTP on 127.0.0.1:14515, separate manual/automation credentials. Async recovery task. Up to four submit-body readers so incomplete submission does not block control requests.
- `srv/src/bin/nvoc_service.rs`: optional embedded host, pauses legacy srv temperature/OC writes during hosted work/recovery-required state.
- `srv/src/bin/nvoc_scan_host.rs`: foreground host for debugging and development only — a 24-line wrapper over the same `Host::start` core, not a supported deployment form. The service with embedded host is the deployment path. Enter requests shutdown and cleanup. Optimizer executable is a fixed sibling binary.
- `srv/tests/hosted_scans.rs`: 12 mock/state-machine tests.
- `srv/tests/hosted_http.rs`: live TCP/HTTP integration (port 0 + injectable fake backend): full lifecycle over real HTTP (submit → worker launches optimizer process via ProcessTree → stdout streamed through /log → cancel kills the tree and seals state) plus incomplete-POST-body reader not blocking control endpoints. `Host::start_with` is now `pub` for this. No GPU and no service involved.
- `srv/tests/process_tree.rs`: 3 harmless real Windows process-tree tests plus 3 ignored fixture entry points invoked by those tests.
- `nvoc-python/python/pynvoc/scan_client.py`: urllib client, loopback only, bearer credentials, no proxy/redirect/subprocess fallback.
- `nvoc-python/tests/test_scan_client.py`: 5 client tests, imports module directly to avoid native pynvoc dependency.
- `gui/src/tabs/vfcurve/sections/autoscan.py`: hosted full optimization panel (standard/ultrafast/legacy), task list, logs, cancellation, takeover/release/recovery, result export, reconnect and pending request ID handling. Closing GUI stops polling, not srv task.
- `gui/tests/test_hosted_autoscan.py`: 4 controller tests.
- `srv/Cargo.toml`, `srv/src/lib.rs`: required module/dependency wiring.

## Verification already completed

- Rust cargo check all srv targets passed before latest changes.
- Rust tests: 19 passed (4 API unit, 12 hosted/state, 3 Windows job tests); three fixture tests intentionally ignored in ordinary test enumeration.
- Python GUI + client suite: 107 passed.
- Ruff format/check passed before latest Rust-only edits.
- Real Job Object testing found ActiveProcesses can reach zero before root process handle signals; fixed cleanup to wait for BOTH. Tests then passed.
- Last clippy run reported collapsible_if in UTF-8 log handling; fixed, but clippy has not been rerun.

## Verified 2026-09-18 (checkpoint follow-up)

All three previously unverified edits compile and pass: expanded GPU default recovery, Host `start_with` refactor, and the log/process-test clippy fixes. Green: cargo check/test (19 passed + 3 ignored fixtures), clippy `-D warnings`, rustfmt edition 2024, pytest 107 passed, `git diff --check`.

New fix this round: `ProcessTree::spawn` normalizes verbatim `\\?\` cwd paths (`plain_cwd`). Root cause found by controlled experiments: a child created with a verbatim current directory hangs during startup (resumed but stuck in kernel waits, never runs main, never spawns descendants), while identical spawns with a plain path work. The process-tree tests reproduce this via `Workspace::new()` `canonicalize()` and pass with the original canonicalize restored, so the verbatim path case stays permanently covered. Hardware behavior of the recovery resets remains unvalidated.

## Remaining work

- HTTP hardening: submit readers bounded to four but no socket read timeout; stuck readers can exhaust submissions. Other response writes remain synchronous and a slow reader may block dispatch. Resolve before claiming robust service behavior.
- Log paging: terminal 64 KiB pages can split UTF-8; preserve incomplete trailing codepoint whenever more file bytes remain, not only while nonterminal.
- Result API currently only final CSV and silently truncates at 4 MiB. Make size behavior explicit and expose useful legacy artifacts (`scan/vfp.jsonl`, stdout). JSONL last test entry can be rewritten in place: do not assume immutable append-only events.
- GUI pending request persistence uses config.set (async flush), despite comment promising persistence before submission; explicitly save before sending or correct durability behavior.
- GUI _action should attach returned recovery task ID; enable Stop only for cancellable Scan states, not Recovery/recovering.
- Add user-facing setup/usage docs, replacing or supplementing this checkpoint. Explain full optimize workflow replaces old raw autoscan panel, default-reset recovery, no real GPU validation, and direct control paths remaining outside arbitration. Present `nvoc_scan_host` as a debugging harness, not a supported deployment form.
- Run final fmt, clippy, relevant Rust/Python suites and git diff --check. No MCP implementation yet.

## API/configuration

Host enabled with distinct `NVOC_SCAN_MANUAL_TOKEN` and `NVOC_SCAN_AUTOMATION_TOKEN`, each >=32 characters. Optional `NVOC_SCAN_ROOT`; default scan-data beside executable. GUI client uses manual token and optional `NVOC_SCAN_URL` (default http://127.0.0.1:14515). Service-account environment/config needs deployment documentation.

- POST /v1/scans: request_id, gpu_id (stable ID, not index), mode (standard/ultrafast/legacy).
- GET /v1/scans; GET /v1/scans/{id}; GET /v1/scans/{id}/log?offset=0; GET /v1/scans/{id}/result.
- POST /v1/scans/{id}/cancel.
- GET /v1/control.
- POST /v1/control/{gpu_id}/takeover, /release, /recover (manual credential only).
- Restart marks unfinished tasks interrupted and requires explicit recovery. Successful recovery clears recovery_required but retains manual hold until released.

## Workspace and commands

Repository: D:\08-skyworks\nvoc-srv\nvoc, originally main commit 615ea92. Do not overwrite working changes or touch sibling old repositories.
All newly downloaded dependencies/caches/build output MUST remain under D:\08-skyworks\nvoc-srv.
Dot-source D:\08-skyworks\nvoc-srv\workspace-env.ps1 before commands. It sets local .deps paths and CARGO_INCREMENTAL=0 (volume lacks hardlinks).

Local rustup 1.95 install was damaged by overlapping install/check; do not use or reinstall concurrently. Use existing installed stable 1.97.1 binaries directly, keeping all cargo output/cache local:

```powershell
# Run from D:\08-skyworks\nvoc-srv
. .\workspace-env.ps1
$rustBin = 'C:\Users\Chebeilsea\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin'
$env:RUSTC = "$rustBin\rustc.exe"
$env:RUSTDOC = "$rustBin\rustdoc.exe"
$env:PATH = "$rustBin;$env:PATH"
& "$rustBin\cargo.exe" test --offline --manifest-path nvoc/Cargo.toml -p nvoc-srv --all-targets
& "$rustBin\cargo.exe" clippy --offline --manifest-path nvoc/Cargo.toml -p nvoc-srv --all-targets -- -D warnings
# rustfmt changed Rust files with --edition 2024.
& .deps/py/Scripts/python.exe -m pytest nvoc/gui/tests nvoc/nvoc-python/tests/test_scan_client.py
```

Python venv .deps/py uses existing bundled Python 3.12 with system-site-packages; pytest/ruff/customtkinter/pystray installed locally. All 232 Cargo.lock registry packages are cached locally; .deps/fetch_crates.py verified checksums and worked around old archive mtimes on this volume. nvapi-rs submodule initialized. No background build/download/test process remains from this checkpoint.
