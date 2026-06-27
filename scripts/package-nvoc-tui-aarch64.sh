#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
platform="${NVOC_TUI_DOCKER_PLATFORM:-linux/arm64}"
dockerfile="${NVOC_TUI_DOCKERFILE:-${repo_root}/packaging/nvoc-tui-aarch64.Dockerfile}"
output_dir="${NVOC_TUI_OUTPUT_DIR:-${repo_root}/dist/linux-aarch64}"
target="${NVOC_TUI_DOCKER_TARGET:-artifact}"
builder="${NVOC_TUI_BUILDX_BUILDER:-}"
progress="${BUILDKIT_PROGRESS:-plain}"

if [[ "${platform}" != "linux/arm64" ]]; then
    echo "Only linux/arm64 is supported by this packaging script, got ${platform}" >&2
    exit 64
fi

if [[ ! -f "${repo_root}/nvapi-rs/hi/Cargo.toml" ]]; then
    echo "nvapi-rs submodule content is missing." >&2
    echo "Run: git submodule update --init --recursive" >&2
    exit 2
fi

if [[ ! -f "${dockerfile}" ]]; then
    echo "Dockerfile not found: ${dockerfile}" >&2
    exit 2
fi

if ! docker buildx version >/dev/null 2>&1; then
    echo "Docker buildx is required to build linux/arm64 artifacts." >&2
    exit 2
fi

host_arch="$(uname -m)"
if [[ "${host_arch}" != "aarch64" && "${host_arch}" != "arm64" ]]; then
    if [[ ! -r /proc/sys/fs/binfmt_misc/qemu-aarch64 ]] \
        || ! grep -q '^enabled$' /proc/sys/fs/binfmt_misc/qemu-aarch64; then
        if [[ "${NVOC_TUI_SKIP_BINFMT:-0}" == "1" ]]; then
            echo "qemu-aarch64 binfmt is not registered; unset NVOC_TUI_SKIP_BINFMT or register it manually." >&2
            exit 2
        fi
        echo "Registering qemu-aarch64 binfmt through Docker..."
        docker run --privileged --rm tonistiigi/binfmt --install arm64
    fi
fi

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/nvoc-tui-aarch64.XXXXXX")"
trap 'rm -rf "${tmp_dir}"' EXIT

build_args=(
    --platform "${platform}"
    --file "${dockerfile}"
    --target "${target}"
    --output "type=local,dest=${tmp_dir}"
    --progress "${progress}"
)

if [[ -n "${builder}" ]]; then
    build_args+=(--builder "${builder}")
fi

docker buildx build "${build_args[@]}" "${repo_root}"

if [[ ! -f "${tmp_dir}/nvoc-tui" ]]; then
    echo "Docker build completed but did not produce nvoc-tui." >&2
    exit 1
fi

mkdir -p "${output_dir}"
install -m 0755 "${tmp_dir}/nvoc-tui" "${output_dir}/nvoc-tui"

file "${output_dir}/nvoc-tui"
echo "Wrote ${output_dir}/nvoc-tui"
