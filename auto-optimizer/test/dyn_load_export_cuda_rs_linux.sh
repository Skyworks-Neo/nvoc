#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/../.." && pwd)"
binary="${NVOC_CUDA_RS_BIN:-${repo_root}/target/release/cli-stressor-cuda-rs}"
config="${script_dir}/cli-stressor-cuda-rs-dyn-export.conf"

exec "${binary}" --config "${config}" "$@"
