#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/../.." && pwd)"
binary="${NVOC_CUDA_RS_BIN:-${repo_root}/target/release/cli-stressor-cuda-rs}"
config="${script_dir}/cli-stressor-cuda-rs-6g-8g.toml"

test_code="${1:-}"
loops="${2:-30}"
shift $(( $# >= 1 ? 1 : 0 ))
shift $(( $# >= 1 ? 1 : 0 ))

duration=$((loops * 5))
args=()
for arg in "$@"; do
    case "${arg}" in
        --aggressive-recovery)
            ;;
        *)
            args+=("${arg}")
            ;;
    esac
done

echo "CUDA RS Linux stressor: test=${test_code:-default} loops=${loops} duration=${duration}s"
exec "${binary}" --config "${config}" --duration "${duration}" "${args[@]}"
