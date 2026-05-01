# Contributing to NVOC

NVOC is a mixed Rust and Python monorepo. Keep changes scoped to the component you are modifying, and update the relevant component README when behavior, commands, or setup steps change.

## Repository Areas

- `auto-optimizer/`: Rust CLI core and shared overclocking behavior.
- `cli-stressor-cuda/`: CUDA/PyTorch stress workload.
- `cli-stressor-opencl/`: OpenCL stress workload.
- `gui/`: Python GUI frontend.
- `tui/`: Python Textual terminal frontend.
- `srv/`: Windows service wrapper and localhost control endpoint.

## Development Checks

Run the checks that match the files you changed:

```bash
cd auto-optimizer && cargo build
cd srv && cargo build
cd tui && uv run pytest
```

For Python components, run `uv sync` before local testing when dependencies have changed.

## Safety

Changes that write GPU state need extra care. Document the tested GPU generation, driver, operating system, and whether the change uses NVAPI, NVML, CUDA, or OpenCL. Prefer read-only validation before write operations, and keep recovery/reset behavior visible in the docs.

## Documentation

Use monorepo-relative links for internal references. The canonical repository URL is:

```text
https://github.com/Skyworks-Neo/nvoc
```
