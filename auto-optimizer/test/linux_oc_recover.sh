#! /bin/bash
# NVOC GPU recovery for Linux — merge of the original unbind+FLR path and the
# nvidia-smi -r / module-reload escalation ladder, hang-proofed.
#
# Expect: this_script [--kill-holders] {nv_pci_like_0000:01:00.0}
#
# Why not a bare `echo $pci > .../unbind`: the kernel blocks that write until
# EVERY /dev/nvidia* file descriptor is closed. Sensor pollers holding NVML
# handles, nvidia-persistenced, and CUDA apps stuck in D-state keep fds open
# forever, so the writer wedges and even SIGKILL cannot reap it until the
# holders exit. This script therefore:
#   - runs every potentially blocking step as a background writer polled
#     against a deadline, so the script itself can never wedge;
#   - reports /dev/nvidia* holders up front (fuser, fallback lsof); with
#     --kill-holders it SIGKILLs them first (kills ALL GPU clients machine-wide);
#   - stops nvidia-persistenced — the by-design permanent fd holder — before
#     tearing anything down, and restarts it at the end.
#
# Ladder (first tier whose post-check passes wins):
#   1. nvidia-smi -r        in-driver reset, per-GPU, cheapest
#   2. unbind -> PCI FLR -> rebind
#                           original path; FLR is the step that actually
#                           resets hardware state (PLL/VREG) written bad by OC
#   3. PCI remove + rescan  rescues devices that fell off the bus (Xid 79)
#   4. module stack reload  last resort; affects every GPU on the host
#
# Exit codes: 0 recovered (or already healthy), 1 still broken, 127 usage.

set -u

TIMEOUT_SYSFS="${TIMEOUT_SYSFS:-15}"
TIMEOUT_RESET="${TIMEOUT_RESET:-90}"
TIMEOUT_MOD="${TIMEOUT_MOD:-30}"
TIMEOUT_PROBE="${TIMEOUT_PROBE:-10}"

KILL_HOLDERS=0
gpu_pci=""
DRV_DIR=/sys/bus/pci/drivers/nvidia
PERSISTED_STOPPED=0
IDX=""

usage() {
  echo "Usage: sudo $0 [--kill-holders] NV_GPU_PCI_ID (like 0000:01:00.0)" 1>&2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -k|--kill-holders) KILL_HOLDERS=1 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" 1>&2; usage; exit 127 ;;
    *) gpu_pci="$1" ;;
  esac
  shift
done

[[ -n "$gpu_pci" ]] || { usage; exit 127; }
[[ "$(id -u)" == "0" ]] || { echo "Run with sudo" 1>&2; exit 127; }

gpu_pci="${gpu_pci,,}"
if [[ ! "$gpu_pci" =~ ^[0-9a-f]{4}:[0-9a-f]{2}:[0-9a-f]{2}\.[0-9a-f]$ ]]; then
  echo "Bad PCI address: $gpu_pci (expected like 0000:01:00.0)" 1>&2
  exit 127
fi
[[ -e "/sys/bus/pci/devices/$gpu_pci" ]] || {
  echo "No such PCI device: $gpu_pci" 1>&2
  exit 127
}

# run_deadline SEC [DESC] -- cmd...
# Runs cmd in the background and polls with 1s granularity. On deadline the
# writer is abandoned (it stays in D-state until the kernel unblocks it; that
# is harmless) and 124 is returned, so the script keeps control.
run_deadline() {
  local sec="$1" desc="${2:-}"; shift 2
  [[ "${1:-}" == "--" ]] && shift
  [[ -n "$desc" ]] && echo "  >> $desc (deadline ${sec}s)"
  "$@" &
  local pid=$! waited=0
  while kill -0 "$pid" 2>/dev/null && (( waited < sec )); do
    sleep 1
    waited=$((waited + 1))
  done
  if kill -0 "$pid" 2>/dev/null; then
    [[ -n "$desc" ]] && echo "  !! deadline hit — writer abandoned"
    return 124
  fi
  wait "$pid"
}

norm_pci() { # 0000:01:00.0 / 00000000:1:00.0 -> 0:1:00.0
  local d b df
  IFS=: read -r d b df <<<"${1,,}" || return 1
  printf '%d:%d:%s\n' "$((16#${d:-0}))" "$((16#${b:-0}))" "$df"
}

pci_to_index() { # best effort; empty output when NVML cannot see the GPU
  local want tmp rc idx bus
  want="$(norm_pci "$gpu_pci")" || return 1
  tmp="$(mktemp)"
  run_deadline "$TIMEOUT_PROBE" "" -- \
    nvidia-smi --query-gpu=index,pci.bus_id --format=csv,noheader >"$tmp" 2>/dev/null
  rc=$?
  if (( rc == 0 )); then
    while IFS=, read -r idx bus; do
      idx="${idx//[[:space:]]/}"
      bus="${bus//[[:space:]]/}"
      if [[ -n "$idx" && "$(norm_pci "$bus" 2>/dev/null)" == "$want" ]]; then
        rm -f "$tmp"
        echo "$idx"
        return 0
      fi
    done <"$tmp"
  fi
  rm -f "$tmp"
  return 1
}

gpu_healthy() { # $1 = optional NVML index; probes are deadline-guarded too
  local idx="${1:-}"
  if [[ -n "$idx" ]]; then
    run_deadline "$TIMEOUT_PROBE" "" -- \
      nvidia-smi -i "$idx" --query-gpu=name --format=csv,noheader >/dev/null 2>&1
  else
    run_deadline "$TIMEOUT_PROBE" "" -- \
      nvidia-smi --query-gpu=name --format=csv,noheader >/dev/null 2>&1
  fi
}

report_holders() {
  echo "=== GPU clients ==="
  if ! ls /dev/nvidia* >/dev/null 2>&1; then
    echo "  (no /dev/nvidia* device nodes present)"
    return 0
  fi
  if command -v fuser >/dev/null 2>&1; then
    fuser -v /dev/nvidia* 2>&1 || true
  elif command -v lsof >/dev/null 2>&1; then
    lsof /dev/nvidia* 2>/dev/null || true
  else
    echo "  (neither fuser nor lsof installed — cannot list holders)"
  fi
}

quiesce_persistenced() {
  if systemctl is-active --quiet nvidia-persistenced 2>/dev/null; then
    echo "Stopping nvidia-persistenced (permanent fd holder; restarted at the end)"
    systemctl stop nvidia-persistenced 2>/dev/null && PERSISTED_STOPPED=1 || true
  fi
}

restore_persistenced() {
  (( PERSISTED_STOPPED )) || return 0
  echo "Restarting nvidia-persistenced"
  systemctl start nvidia-persistenced 2>/dev/null || true
}

tier1_nvml_reset() {
  echo "=== Tier 1: nvidia-smi -r (in-driver reset) ==="
  if [[ -z "$IDX" ]]; then
    echo "  skipped: GPU has no NVML index"
    return 1
  fi
  if ! run_deadline "$TIMEOUT_RESET" "nvidia-smi -i $IDX -r" -- nvidia-smi -i "$IDX" -r; then
    echo "  -r failed/refused (usually: GPU still in use)"
    return 1
  fi
  sleep 2
  if gpu_healthy "$IDX"; then return 0; fi
  echo "  -r ran but GPU still not healthy"
  return 1
}

tier2_unbind_flr_rebind() {
  echo "=== Tier 2: unbind -> PCI FLR -> rebind ==="
  if [[ "$(readlink "/sys/bus/pci/devices/$gpu_pci/driver" 2>/dev/null)" != */drivers/nvidia ]]; then
    echo "  skipped: device not bound to nvidia"
    return 1
  fi
  if ! run_deadline "$TIMEOUT_SYSFS" "unbind $gpu_pci" -- \
      bash -c "echo '$gpu_pci' > '$DRV_DIR/unbind'"; then
    echo "  unbind blocked — fd holders still alive; kill them and rerun (--kill-holders)"
    return 1
  fi
  sleep 2
  if run_deadline "$TIMEOUT_SYSFS" "PCI FLR reset" -- \
      bash -c "echo 1 > '/sys/bus/pci/devices/$gpu_pci/reset'"; then
    echo "  FLR issued"
  else
    echo "  FLR failed/unsupported — attempting rebind anyway"
  fi
  sleep 2
  if ! run_deadline "$TIMEOUT_SYSFS" "rebind $gpu_pci" -- \
      bash -c "echo '$gpu_pci' > '$DRV_DIR/bind'"; then
    echo "  rebind failed"
    return 1
  fi
  sleep 2
  IDX="$(pci_to_index || true)"
  if [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then return 0; fi
  echo "  bound but not healthy"
  return 1
}

tier3_remove_rescan() {
  echo "=== Tier 3: PCI remove + rescan ==="
  run_deadline "$TIMEOUT_SYSFS" "remove $gpu_pci" -- \
    bash -c "echo '$gpu_pci' > '/sys/bus/pci/devices/$gpu_pci/remove'" || true
  sleep 1
  run_deadline "$TIMEOUT_SYSFS" "PCI rescan" -- \
    bash -c "echo 1 > '/sys/bus/pci/rescan'" || true
  sleep 3
  if [[ ! -e "/sys/bus/pci/devices/$gpu_pci" ]]; then
    echo "  device did not reappear"
    return 1
  fi
  if [[ "$(readlink "/sys/bus/pci/devices/$gpu_pci/driver" 2>/dev/null)" != */drivers/nvidia && -d "$DRV_DIR" ]]; then
    run_deadline "$TIMEOUT_SYSFS" "bind $gpu_pci" -- \
      bash -c "echo '$gpu_pci' > '$DRV_DIR/bind'" || true
    sleep 2
  fi
  IDX="$(pci_to_index || true)"
  if [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then return 0; fi
  echo "  device back but not healthy"
  return 1
}

tier4_module_reload() {
  echo "=== Tier 4: full nvidia module reload (hits every GPU on the host) ==="
  local -a removed=()
  local m r
  for m in nvidia_peermem nvidia_uvm nvidia_drm nvidia_modeset nvidia; do
    lsmod | grep -q "^${m} " || continue
    if run_deadline "$TIMEOUT_MOD" "modprobe -r $m" -- modprobe -r "$m"; then
      removed+=("$m")
    else
      echo "  unload $m failed (still in use)"
    fi
  done
  sleep 2
  if (( ${#removed[@]} == 0 )); then
    echo "  nothing could be unloaded"
    return 1
  fi
  # modprobe pulls dependencies itself, so loading nvidia covers the stack
  for m in nvidia nvidia_modeset nvidia_drm nvidia_uvm nvidia_peermem; do
    for r in "${removed[@]}"; do
      if [[ "$r" == "$m" ]]; then
        run_deadline "$TIMEOUT_MOD" "modprobe $m" -- modprobe "$m" \
          || echo "  reload $m failed"
        break
      fi
    done
  done
  sleep 3
  IDX="$(pci_to_index || true)"
  if [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then return 0; fi
  echo "  modules reloaded but GPU not healthy"
  return 1
}

finish_ok() {
  echo
  echo "=== Final state ==="
  nvidia-smi || true
  restore_persistenced
  echo "RECOVERY: SUCCESS"
  exit 0
}

finish_fail() {
  echo
  echo "=== Final state ==="
  echo "Recent NVRM/Xid lines:"
  dmesg 2>/dev/null | grep -Ei 'NVRM|Xid' | tail -n 15 || true
  report_holders
  restore_persistenced
  nvidia-smi || true
  echo "RECOVERY: FAILED — remaining option is a host reboot."
  exit 1
}

echo "=== NVOC GPU recovery: $gpu_pci ==="

# Read-only probe first: do not touch a system whose GPU is already fine.
IDX="$(pci_to_index || true)"
if [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then
  echo "GPU already responds to NVML (index $IDX) — nothing to recover."
  nvidia-smi -i "$IDX" || true
  exit 0
fi

report_holders
if (( KILL_HOLDERS )) && ls /dev/nvidia* >/dev/null 2>&1; then
  echo "Killing all processes holding /dev/nvidia*"
  fuser -k -KILL /dev/nvidia* 2>/dev/null || true
  sleep 3
fi
quiesce_persistenced
IDX="$(pci_to_index || true)"
echo "NVML index: ${IDX:-<unresolvable — GPU invisible to nvidia-smi>}"

if tier1_nvml_reset; then finish_ok; fi
if tier2_unbind_flr_rebind; then finish_ok; fi
if tier3_remove_rescan; then finish_ok; fi
if tier4_module_reload; then finish_ok; fi
finish_fail
