#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
auto_optimizer_dir="$(cd -- "${script_dir}/.." && pwd)"
repo_root="$(cd -- "${auto_optimizer_dir}/.." && pwd)"
bin="${NVOC_AUTO_OPTIMIZER_BIN:-${repo_root}/target/release/nvoc-auto-optimizer}"
cli_bin="${NVOC_CLI_BIN:-${repo_root}/target/release/nvoc-cli}"

red=$'\033[1;91m'
green=$'\033[1;92m'
yellow=$'\033[1;93m'
cyan=$'\033[1;96m'
reset=$'\033[0m'

cd "${auto_optimizer_dir}"

if [[ ! -x "${bin}" ]]; then
    echo "${red}Missing executable: ${bin}${reset}" >&2
    echo "Build it with: cargo build --release -p nvoc-auto-optimizer" >&2
    exit 1
fi
if [[ ! -x "${cli_bin}" ]]; then
    echo "${red}Missing executable: ${cli_bin}${reset}" >&2
    echo "Build it with: cargo build --release -p nvoc-cli" >&2
    exit 1
fi

"${cli_bin}" get-info

echo "Detecting GPUs in system..."
"${cli_bin}" list-gpus
echo
read -r -p "Input target GPU id to be scanned: " gpu_id

if [[ -z "${gpu_id}" ]]; then
    echo "${red}No GPU id provided.${reset}" >&2
    exit 2
fi

echo
echo "Selected GPU: ${gpu_id}"
echo

# Resolve GPU UUID for per-GPU workspace isolation
uuid_raw=$("${cli_bin}" --gpu="${gpu_id}" get-uuid 2>/dev/null | tail -1 | tr -d '[:space:]')
uuid="${uuid_raw#GPU-}"

wsdir="./Scan-${uuid}"
logfile="${wsdir}/vfp.jsonl"
vfptemfile="${wsdir}/vfp-tem.csv"

mkdir -p "${wsdir}"
if [[ ! -f "${logfile}" ]]; then
    : > "${logfile}"
    echo "${green}Log file created: ${logfile}${reset}"
fi

sudo "${cli_bin}" --gpu="${gpu_id}" reset-pstate-clock-offsets
sudo "${bin}" --gpu="${gpu_id}" reset-vfp
sudo "${cli_bin}" --gpu="${gpu_id}" reset-vfp-lock

if [[ ! -f "${wsdir}/vfp-init.csv" ]]; then
    echo "exporting default data..."
    sudo "${bin}" --gpu="${gpu_id}" export-vfp "${wsdir}/vfp-init.csv"
fi

if [[ "${1:-}" == "1" ]]; then
    : > "${logfile}"
    : > "${vfptemfile}"
fi

echo " ================================================================="
echo "${yellow}===================DISCLAIMER=======================${reset}"
echo "${red}vfp scan may consistently trigger your GPU safe limit and crash...${reset}"
echo "${red}WARNING: SYSTEM HANG or CRASH IS EXPECTED!!!!!!!!!${reset}"
echo "${cyan}IF SYSTEM HANGS FOR MORE THAN 3 MIN, FORCE REBOOT.${reset}"
echo "${cyan}IF THAT OCCURS, FORCE RESTART and RUN THIS SCRIPT AGAIN.${reset}"
echo "${green}The scanner WILL CONTINUE from breakpoint AUTOMATICALLY.${reset}"
echo "${green}This will NOT DAMAGE your GPU; the scan result is SAFE to use.${reset}"
echo "${yellow}If crash is unacceptable right now, press Ctrl-C to exit.${reset}"
echo
read -r -p "Press Enter to start autoscan..."

sudo "${bin}" --gpu="${gpu_id}" autoscan-vfp \
    --log "${logfile}" \
    -i "${wsdir}/vfp-init.csv" \
    -o "${vfptemfile}" \
    --test-exe ./test/test_cuda_linux.sh \
    --minload-exe ./test/cli-stressor-cuda-rs-minload.sh \
    --stressor-extra-args --gpu-index "${gpu_id}"
sudo "${bin}" --gpu="${gpu_id}" fix-vfp-result -m 1 -v "${vfptemfile}" -o "${wsdir}/vfp.csv" -l "${logfile}"
sudo "${bin}" --gpu="${gpu_id}" import-vfp "${wsdir}/vfp.csv"
sudo "${bin}" --gpu="${gpu_id}" export-vfp "${wsdir}/vfp-final.csv"

echo "${green}All VFP scan finished. Check ${wsdir}/vfp-final.csv.${reset}"
