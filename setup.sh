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

# tkinter: uv-managed pythons bundle Tcl/Tk, but a SYSTEM python3 does not
# until the distro's split package is installed. The gap only bites when uv
# resolves to the system interpreter (UV_PYTHON_PREFERENCE=only-system,
# python-downloads=false, or a pre-existing system venv), so this stays an
# informational probe; `cargo xtask setup` re-checks tkinter inside the real
# gui environment and gives the same distro-aware guidance there.
if command -v python3 >/dev/null 2>&1 &&
    ! python3 -c 'import tkinter' >/dev/null 2>&1; then
    package=''
    # shellcheck disable=SC1091
    . /etc/os-release 2>/dev/null || true
    case "${ID:-}${ID_LIKE:-}" in
        *debian* | *ubuntu*) package='sudo apt install python3-tk' ;;
        *fedora* | *rhel* | *centos*) package='sudo dnf install python3-tkinter' ;;
        *arch* | *manjaro*) package='sudo pacman -S --needed tk' ;;
        *suse*) package='sudo zypper install python3-tk' ;;
        *alpine*) package='apk add python3-tkinter' ;;
    esac
    echo "[bootstrap] system python3 lacks tkinter (the GUI needs it)."
    if [ -n "$package" ]; then
        echo "[bootstrap]   uv-managed pythons bundle Tcl/Tk by default; if you keep uv on"
        echo "[bootstrap]   the system interpreter, run: $package"
    else
        echo "[bootstrap]   install your distro's tkinter package (e.g. python3-tk)."
    fi
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
