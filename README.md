# NVOC

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

NVOC is a monorepo for NVIDIA GPU overclocking and stability tooling. The stack centers on a Rust optimizer that controls GPUs through NVAPI/NVML, with stress modules and GUI/TUI/SRV frontends for different operating environments.

> ⚠️ Overclocking writes can crash drivers, reset GPUs, and destabilize systems. Start with read-only validation and ensure rollback/recovery paths before any write operation.

## Documentation & Wiki Policy

For this repository size and contributor model, documentation is maintained **in this monorepo first**.

- Canonical docs path: `docs/wiki/`
- Review flow: open PRs in `nvoc`, review here, then sync to GitHub Wiki (`nvoc-wiki`) if needed.
- Why: avoids split-brain edits and keeps docs changes reviewed alongside code changes.

If wiki pages differ from `docs/wiki`, treat `docs/wiki` as source of truth and sync the wiki repo.

## Components

| Component | Path |
|---|---|
| Auto Optimizer | `auto-optimizer/` |
| Core Library | `nvoc-core/` |
| GUI | `gui/` |
| TUI | `tui/` |
| Service | `srv/` |
| CUDA Stressor (Python) | `cli-stressor-cuda/` |
| CUDA Stressor (Rust) | `cli-stressor-cuda-rs/` |
| OpenCL Stressor | `cli-stressor-opencl/` |

## Quick Start

```bash
git clone https://github.com/Skyworks-Neo/nvoc.git
cd nvoc
cd auto-optimizer && cargo build --release
```

Run a frontend:

```bash
cd gui && uv sync && uv run python main.py
# or
cd tui && uv sync && uv run nvoc-tui
```

## License

Apache License 2.0. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).
