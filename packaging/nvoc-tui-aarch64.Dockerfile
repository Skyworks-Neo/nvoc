# syntax=docker/dockerfile:1.7

ARG RUST_VERSION=1.95.0
ARG UV_VERSION=0.11.24

FROM --platform=$TARGETPLATFORM debian:bookworm-slim AS build

ARG RUST_VERSION
ARG TARGETARCH
ARG UV_VERSION

ENV CARGO_HOME=/root/.cargo \
    CARGO_TARGET_DIR=/tmp/nvoc-cargo-target \
    DEBIAN_FRONTEND=noninteractive \
    PATH=/root/.cargo/bin:/usr/local/bin:/usr/local/sbin:/usr/sbin:/usr/bin:/sbin:/bin \
    PYTHONUNBUFFERED=1 \
    UV_CACHE_DIR=/tmp/uv-cache

RUN test "${TARGETARCH}" = "arm64" || (echo "nvoc-tui aarch64 packaging requires --platform linux/arm64" >&2; exit 64)

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        binutils \
        build-essential \
        ca-certificates \
        curl \
        file \
        git \
        patchelf \
        pkg-config \
        python3 \
        python3-dev \
        python3-pip \
        python3-venv \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" \
    && rustup default "${RUST_VERSION}"

RUN python3 -m pip install --no-cache-dir --break-system-packages "uv==${UV_VERSION}"

WORKDIR /work/nvoc
COPY . .

RUN test -f nvapi-rs/hi/Cargo.toml \
    || (echo "nvapi-rs submodule content is missing; run: git submodule update --init --recursive" >&2; exit 2)

RUN --mount=type=cache,target=/root/.cache/uv \
    --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/root/.cargo/git \
    --mount=type=cache,target=/tmp/nvoc-cargo-target \
    uv sync --locked --package nvoc-tui --group dev --no-config --no-editable

RUN --mount=type=cache,target=/root/.cache/uv \
    --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/root/.cargo/git \
    --mount=type=cache,target=/tmp/nvoc-cargo-target \
    uv run --locked --package nvoc-tui --group dev --no-config --no-editable \
        python -c "import platform, pynvoc, nvoc_tui; print(platform.machine(), pynvoc.__name__, nvoc_tui.__name__)"

WORKDIR /work/nvoc/tui

RUN --mount=type=cache,target=/root/.cache/uv \
    --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/root/.cargo/git \
    --mount=type=cache,target=/tmp/nvoc-cargo-target \
    uv run --locked --package nvoc-tui --group dev --no-config --no-editable \
        pyinstaller --clean --noconfirm nvoc_tui.spec

RUN file dist/nvoc-tui \
    && readelf -h dist/nvoc-tui | grep -q "Machine:[[:space:]]*AArch64"

FROM scratch AS artifact
COPY --from=build /work/nvoc/tui/dist/nvoc-tui /nvoc-tui
