#!/usr/bin/env bash
# nvoc fresh-machine bootstrap: install rustup / uv when missing, then hand off
# to `cargo xtask setup`. Extra arguments are forwarded, e.g.:  ./setup.sh --dry-run
set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
    echo "[bootstrap] cargo not found - installing rustup via rustup.rs..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    export PATH="$HOME/.cargo/bin:$PATH"
fi

if ! command -v uv >/dev/null 2>&1; then
    echo "[bootstrap] uv not found - installing via astral.sh..."
    curl -LsSf https://astral.sh/uv/install.sh | sh
    # The uv installer uses ~/.local/bin; older releases used ~/.cargo/bin.
    export PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
fi

# nvapi-rs is a path dependency of nvoc-core, so `cargo run -p xtask` cannot
# even parse the workspace manifest without it — yet the submodule bootstrap
# is itself a step inside `cargo xtask setup`. Break the chicken-and-egg by
# checking it out here, before the first cargo invocation.
if [ ! -f nvapi-rs/Cargo.toml ]; then
    echo "[bootstrap] nvapi-rs submodule missing - initializing..."
    git submodule update --init nvapi-rs
fi

exec cargo run --quiet -p xtask -- setup "$@"
