#! /bin/bash
# NVOC GPU recovery for Linux — merge of the original unbind+FLR path and the
# nvidia-smi -r / module-reload escalation ladder, hang-proofed.
#
# Expect: this_script [--kill-holders] [--force] {nv_pci_like_0000:01:00.0}
#
# Why not a bare `echo $pci > .../unbind`: the kernel blocks that write until
# EVERY /dev/nvidia* file descriptor is closed. Sensor pollers holding NVML
# handles, nvidia-persistenced, and CUDA apps stuck in D-state keep fds open
# forever, so the writer wedges and even SIGKILL cannot reap it until the
# holders exit. This script therefore:
#   - runs every potentially blocking step as a background writer polled
#     against a deadline, so the script itself can never wedge;
#   - reports /dev/nvidia* holders up front (fuser, fallback lsof) plus any
#     docker containers holding the device (their containerd-shim shows in
#     fuser but not what it serves); with --kill-holders it SIGKILLs the
#     holders and stops those containers first (kills ALL GPU clients
#     machine-wide; stopped containers are restarted on exit);
#   - stops nvidia-persistenced — the by-design permanent fd holder — before
#     tearing anything down, and restarts it at the end.
#
# "Healthy" means more than nvidia-smi answering: a wedged board can still
# respond to NVML while its sensor fields read ERR! (nvidia-smi -q carries
# more of the same). The health probe therefore queries the fields that go bad
# first and treats any error marker as unhealthy — N/A is NOT an error here:
# some server/embedded cards (e.g. PG199 automotive A100) legitimately report
# N/A on sensor fields they lack. Pass --force to skip the probe entirely and
# run the ladder regardless of what nvidia-smi reports.
#
# Ladder (first tier whose post-check passes wins):
#   1. nvidia-smi -r        in-driver reset, per-GPU, cheapest
#   2. GSP-wedge bus reset  the 2026-10-12 live-proven recovery for the
#                           "WPR2 already up" / RmInitAdapter-failed GSP wedge
#                           (RTX PRO 6000, 580.173.02): stop display-manager,
#                           unload the nvidia CLIENT modules (never nvidia
#                           itself), unbind, SECONDARY BUS reset (deeper than
#                           FLR — FLR alone does not clear the GSP WPR2 state),
#                           restore reset_method, rebind, restart the DM
#   3. unbind -> PCI FLR -> rebind
#                           original path; FLR is the step that actually
#                           resets hardware state (PLL/VREG) written bad by OC
#   4. PCI remove + rescan  rescues devices that fell off the bus (Xid 79)
#   5. module stack reload  last resort; affects every GPU on the host
#   6. PCIe slot power      pure-ACPI power cycle of the slot — never touches
#                           the nvidia driver, so it still works when a D-state
#                           writer holds the device lock (recovery-path
#                           self-lock); 6b falls back to an RTD3-class power
#                           cycle via vfio-pci runtime PM (the Linux equivalent
#                           of the Windows-laptop Runtime D3 recovery), and
#                           prints ipmitool guidance when both are unavailable
#
# Every run prints DIAGNOSIS: <signature codes> (Xid 154 recovery action,
# PMU halt / WPR2 / RmInitAdapter, RC watchdog, non-zero usage count, D-state
# writers) so the verdicts name the fault that actually fired.
#
# Driver-path ops (unbind/reset/rebind/remove while bound) are gated on fd
# holders: with holders alive every such write is a guaranteed D-state wedge
# (the driver waits for its usage count, and the wedged writer holds the
# device lock), so the tiers skip cleanly instead of piling stuck writers.
#
# Exit codes: 0 recovered (or already healthy), 1 still broken, 3 wedged
# below the driver (reboot required), 127 usage.

set -u

TIMEOUT_SYSFS="${TIMEOUT_SYSFS:-15}"
TIMEOUT_RESET="${TIMEOUT_RESET:-90}"
TIMEOUT_MOD="${TIMEOUT_MOD:-30}"
TIMEOUT_PROBE="${TIMEOUT_PROBE:-10}"

KILL_HOLDERS=0
FORCE=0
SINGLE=0
ALL_WEDGED=0
LEDGER_PATH=""
gpu_pci=""
gpu_pci_short=""
DRV_DIR=/sys/bus/pci/drivers/nvidia
PERSISTED_STOPPED=0
DM_STOPPED=0
DOCKER_GPU_CONTAINERS=()
DOCKER_STOPPED=()
UNLOADED_CLIENTS=()
ABANDONED_WRITERS=()
DIAG_CODES=()
DIAG_XID154_ACTION=""
DIAG_STUCK_WRITERS=0
HOLDGATE=0
PROBE_CMD=""
PURGE=1
HAD_HOLDERS=0
HOLD_KILL_FAILED=0
POSITIONAL=()
START_SECONDS=$SECONDS
IDX=""

usage() {
  echo "Usage: sudo $0 [flags] BDF... | --all-wedged   (like 0000:01:00.0)" 1>&2
  echo "  One BDF = single-card flow. Several BDFs or --all-wedged = multi-card mode:" 1>&2
  echo "                  one GPU-side reset per card, then ONE driver-state purge for the whole incident." 1>&2
  echo "  --kill-holders  SIGKILL holders of each target GPU's device node and stop docker GPU containers (restarted on exit)" 1>&2
  echo "  --all-wedged    auto-detect wedged cards (NVML ERR! fields + PCI-bound-but-NVML-invisible)" 1>&2
  echo "  --force         skip the health probe, run the ladder regardless" 1>&2
  echo "  --probe-cmd CMD extra health gate: run CMD deadline-guarded after NVML checks — plug your own RM/NVAPI query" 1>&2
  echo "  --no-purge      skip the post-recovery driver-state purge (default ON: full module reload when no fd remains — the only driver-side residue cleaner)" 1>&2
  echo "  --ledger PATH   append one JSON line per card (diagnosis/tier/result) for OC-scan fault datasets" 1>&2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -k|--kill-holders) KILL_HOLDERS=1 ;;
    -f|--force) FORCE=1 ;;
    --no-purge) PURGE=0 ;;
    --single) SINGLE=1 ;;            # internal: one-card child invocation
    -a|--all-wedged) ALL_WEDGED=1 ;;
    -p|--probe-cmd)
      if [[ $# -lt 2 ]]; then
        echo "--probe-cmd requires a COMMAND argument" 1>&2
        usage
        exit 127
      fi
      PROBE_CMD="$2"
      shift
      ;;
    -l|--ledger)
      if [[ $# -lt 2 ]]; then
        echo "--ledger requires a PATH argument" 1>&2
        usage
        exit 127
      fi
      LEDGER_PATH="$2"
      shift
      ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "Unknown option: $1" 1>&2; usage; exit 127 ;;
    *) POSITIONAL+=("$1") ;;
  esac
  shift
done

[[ "$(id -u)" == "0" ]] || { echo "Run with sudo" 1>&2; exit 127; }
# Single recovery agent: two uncoordinated writers on the same sysfs files are
# a zombie-writer amplifier (live 2026-11-21: the scan harness's own auto-
# recovery raced this script — D6 writers grew 12 -> 16 and it even issued a
# remove on the healthy CE). Serialize nvoc invocations; child invocations
# (--single) skip the lock because the parent already holds it for the family.
if (( ! SINGLE )) && command -v flock >/dev/null 2>&1; then
  exec 9>/run/lock/nvoc-recover.lock 2>/dev/null || true
  if ! flock -w "$TIMEOUT_MOD" 9 2>/dev/null; then
    echo "another nvoc recovery holds /run/lock/nvoc-recover.lock — refusing to race it" 1>&2
    exit 127
  fi
fi
# (target-BDF validation lives below the function definitions: the multi-card
# parent block needs the helpers, and single-card mode takes its BDF from
# POSITIONAL now)

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
    # The kernel operation is NOT cancelled (D-state writers cannot be killed):
    # it may complete late or keep holding the device lock. Tracked so verdicts
    # can report lingering writers and the ladder can avoid piling more.
    ABANDONED_WRITERS+=("${desc:-[unnamed writer]} (pid $pid)")
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

# NOTE: no CUDA smoke gate here — live 2026-11-21, CUDA workloads ran fine on
# incompletely-recovered boards, so compute success does not discriminate the
# rs-residue class; the discriminator is --probe-cmd (a deep query).
nvml_healthy() { # $1 = optional NVML index; probes are deadline-guarded too
  local idx="${1:-}" out rc
  # Exit-0 on --query-gpu=name alone is NOT health: a wedged board keeps
  # answering NVML while its sensor fields read ERR! (nvidia-smi -q shows
  # more of the same). Probe the fields that go bad first and treat any error
  # marker as unhealthy so the ladder keeps escalating instead of stopping at
  # a card that merely "responds". N/A is deliberately accepted: some
  # server/embedded boards (e.g. PG199 automotive A100) report it normally.
  if [[ -n "$idx" ]]; then
    out="$(run_deadline "$TIMEOUT_PROBE" "" -- \
      nvidia-smi -i "$idx" \
      --query-gpu=name,temperature.gpu,utilization.gpu,memory.used \
      --format=csv,noheader 2>/dev/null)"
  else
    out="$(run_deadline "$TIMEOUT_PROBE" "" -- \
      nvidia-smi \
      --query-gpu=name,temperature.gpu,utilization.gpu,memory.used \
      --format=csv,noheader 2>/dev/null)"
  fi
  rc=$?
  if (( rc != 0 )); then return 1; fi
  # The four fields above are numeric on a working board (or legitimately
  # N/A on boards lacking the sensor), so matching these markers never hits
  # a real error.
  if [[ ! "$out" =~ [^[:space:]] ]] || [[ "${out,,}" == *err* ]] \
    || [[ "$out" == *"Unknown Error"* ]]; then
    return 1
  fi
  # stage 2: deep sweep — `nvidia-smi -q` walks every NVML subsystem; a card
  # with post-recovery residue answers the fast fields but deadlocks in deeper
  # query classes (live 2026-11-21: script-recovered boards locked up in some
  # query paths). Hang/timeout here = unhealthy, so the ladder escalates
  # instead of blessing a half-alive board. No ERR-marker grep on this output:
  # -q has legitimate "Errors" section names that would false-positive.
  run_deadline "$TIMEOUT_PROBE" "" -- \
    nvidia-smi ${idx:+-i "$idx"} -q >/dev/null 2>&1 || return 1
  return 0
}

gpu_healthy() { # NVML health + (opt-in) custom deep probe — the CUDA path is
  # deliberately NOT gated here: live 2026-11-21, CUDA workloads ran fine on
  # incompletely-recovered boards, so compute success does not discriminate
  # the rs-residue class; the discriminator is --probe-cmd (a deep query).

  nvml_healthy "$1" || return 1
  [[ -n "$PROBE_CMD" ]] || return 0
  local prc=0
  run_deadline "$TIMEOUT_PROBE" "custom probe" -- \
    bash -c "$PROBE_CMD" >/dev/null 2>&1 || prc=$?
  if (( prc != 0 )); then
    echo "  (custom probe failed: exit $prc — $PROBE_CMD)"
    return 1
  fi
  echo "  custom probe: OK — $PROBE_CMD"
  return 0
}

gpu_minor_for_bdf() { # prints this GPU's /dev/nvidiaN minor; empty when unknown
  local info="/proc/driver/nvidia/gpus/$gpu_pci/information"
  [[ -r "$info" ]] || return 0
  awk '/^(Device )?Minor:/ {print $NF; exit}' "$info" 2>/dev/null
}

list_holders() {
  if command -v fuser >/dev/null 2>&1; then
    fuser -v "$@" 2>&1 || true
  elif command -v lsof >/dev/null 2>&1; then
    lsof "$@" 2>/dev/null || true
  else
    echo "  (neither fuser nor lsof installed — cannot list holders)"
  fi
}

report_holders() {
  echo "=== GPU clients ==="
  if ! ls /dev/nvidia* >/dev/null 2>&1; then
    echo "  (no /dev/nvidia* device nodes present)"
    return 0
  fi
  local minor n other=()
  minor="$(gpu_minor_for_bdf)"
  if [[ -n "$minor" && -e "/dev/nvidia$minor" ]]; then
    echo "--- THIS GPU (/dev/nvidia$minor) holders ---"
    list_holders "/dev/nvidia$minor"
    for n in /dev/nvidia*; do
      [[ "$n" == "/dev/nvidia$minor" ]] && continue
      other+=("$n")
    done
    if (( ${#other[@]} > 0 )); then
      echo "--- shared nodes + other GPUs ---"
      list_holders "${other[@]}"
    fi
  else
    # minor unresolvable (driver blind) — legacy flat listing
    list_holders /dev/nvidia*
  fi
}

# fd holders on this GPU's device node pin the driver's usage count: the first
# device-lock-taking sysfs write (unbind / reset-of-bound-device / remove)
# wedges in D-state, and the wedged writer holds the device lock for
# everything behind it (live 2026-11-21: one wedged unbind turned tiers 2-5
# into three abandoned writers). Gate driver-path ops on this instead of
# wedging into it; --kill-holders turns the skip into a scoped kill.
gpu_node_holders() { # prints holder pids (one per line); empty when none
  local minor
  minor="$(gpu_minor_for_bdf)"
  [[ -n "$minor" && -e "/dev/nvidia$minor" ]] || return 0
  if command -v fuser >/dev/null 2>&1; then
    fuser "/dev/nvidia$minor" 2>/dev/null | tr ' ' '\n' | sed '/^$/d'
  elif command -v lsof >/dev/null 2>&1; then
    lsof -t "/dev/nvidia$minor" 2>/dev/null
  fi
  return 0
}

holders_gate() { # $1 = op description; exit 0 = proceed, 1 = holders pin the path
  local pids
  pids="$(gpu_node_holders)"
  [[ -n "$pids" ]] || return 0
  if (( KILL_HOLDERS )); then
    local minor
    minor="$(gpu_minor_for_bdf)"
    echo "  killing holders of /dev/nvidia$minor: $(echo "$pids" | tr '\n' ' ')"
    fuser -k -KILL "/dev/nvidia$minor" 2>/dev/null || true
    sleep 3
    pids="$(gpu_node_holders)"
    [[ -n "$pids" ]] || return 0
    if holder_dstate; then
      echo "  holders survived -KILL: stuck in D-state ioctls on this GPU (circular deadlock —"
      echo "  SIGKILL cannot reap them); no software sequence can proceed past this"
    else
      echo "  holders survived -KILL — $1 would wedge"
    fi
    HOLD_KILL_FAILED=1
    HOLDGATE=1
    return 1
  fi
  echo "  $1 skipped: fd holders pin this GPU's usage count (pids: $(echo "$pids" | tr '\n' ' '))"
  echo "  every device-lock write would wedge in D-state — rerun with --kill-holders"
  HOLDGATE=1
  return 1
}

# A holder stuck in D-state cannot be reaped by SIGKILL: the signal stays
# pending until the ioctl returns — which needs the GPU reset — which needs
# the fd closed. Circular deadlock. Detect it so verdicts stop advising a
# kill that provably cannot work (live 2026-11-21: partner python pinned
# everything while stuck in an ioctl on the wedged board).
holder_dstate() { # exit 0 = at least one holder is in D-state
  local p pid st
  p="$(gpu_node_holders)"
  [[ -n "$p" ]] || return 1
  for pid in $p; do
    st="$(ps -o stat= -p "$pid" 2>/dev/null | tr -d ' ')"
    [[ "$st" == D* ]] && return 0
  done
  return 1
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

# The display manager holds /dev/nvidia* fds through its session (a stopped
# gdm still leaves the seat session's clients alive — live 2026-10-12: -r
# kept answering "In use by another client" AFTER gdm died until the client
# modules were unloaded). Tier 2 stops it outright; restored at finish.
# Stopping it kills the console graphical session — SSH sessions survive.
quiesce_display_manager() {
  if systemctl is-active --quiet display-manager 2>/dev/null; then
    echo "Stopping display-manager (graphical console session dies; restarted at the end)"
    systemctl stop display-manager 2>/dev/null && DM_STOPPED=1 || true
  fi
}

restore_display_manager() {
  (( DM_STOPPED )) || return 0
  echo "Restarting display-manager"
  systemctl start display-manager 2>/dev/null || true
}

restore_client_modules() {
  (( ${#UNLOADED_CLIENTS[@]} > 0 )) || return 0
  echo "Restoring client modules we unloaded: ${UNLOADED_CLIENTS[*]}"
  local m r found
  # dependency order: modeset -> drm -> uvm -> peermem
  for m in nvidia_modeset nvidia_drm nvidia_uvm nvidia_peermem; do
    found=0
    for r in "${UNLOADED_CLIENTS[@]}"; do
      [[ "$r" == "$m" ]] && found=1
    done
    (( found )) || continue
    if ! run_deadline "$TIMEOUT_MOD" "modprobe $m" -- modprobe "$m"; then
      echo "  restore $m failed (load it manually)"
    fi
  done
  UNLOADED_CLIENTS=()
  return 0
}

# Docker GPU containers hold /dev/nvidia* through containerd-shim — fuser
# shows the shim pid but not what it serves. Detect them up front; --kill-
# holders stops them (restarted at the end), without it they are reported
# so the operator knows why an unbind can block.
find_docker_gpu_containers() {
  command -v docker >/dev/null 2>&1 || return 0
  docker info >/dev/null 2>&1 || return 0
  local id info
  for id in $(docker ps -q 2>/dev/null); do
    info="$(docker inspect -f \
      '{{json .HostConfig.DeviceRequests}} {{json .HostConfig.Devices}}' "$id" 2>/dev/null)" || continue
    [[ "$info" == *nvidia* ]] || continue
    DOCKER_GPU_CONTAINERS+=("$id $(docker inspect -f '{{.Name}}' "$id" 2>/dev/null || true)")
  done
  if (( ${#DOCKER_GPU_CONTAINERS[@]} )); then
    echo "Docker GPU containers running:"
    local entry
    for entry in "${DOCKER_GPU_CONTAINERS[@]}"; do
      echo "  ${entry/ /  }"
    done
  fi
}

quiesce_docker_containers() {
  (( ${#DOCKER_GPU_CONTAINERS[@]} )) || return 0
  if (( KILL_HOLDERS )); then
    local id
    for id in "${DOCKER_GPU_CONTAINERS[@]}"; do
      id="${id%% *}"
      if run_deadline 30 "docker stop ${id::12}" -- docker stop -t 10 "$id"; then
        DOCKER_STOPPED+=("$id")
      fi
    done
  else
    echo "Docker GPU containers are holding the device — rerun with --kill-holders to stop them"
  fi
}

restore_docker_containers() {
  (( ${#DOCKER_STOPPED[@]} )) || return 0
  echo "Restarting stopped docker containers"
  local id
  for id in "${DOCKER_STOPPED[@]}"; do
    docker start "$id" >/dev/null 2>&1 || echo "  docker start $id failed (start it manually)"
  done
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

# ---- fault-signature diagnosis -------------------------------------------------
# One refresh scans the kernel log + /proc once and fills DIAG_*; verdicts then
# name the signatures that ACTUALLY fired instead of a canned guess (live
# 2026-11-21: the wedged verdict claimed "RC watchdog + non-zero usage count"
# while the real blocker was a D-state writer, and the card needed only FLR).
#
# GSP-firmware wedge signatures (live 2026-10-12 RTX PRO 6000 / 580.173.02;
# 2026-11-21 RTX 3090 Xid 62 + 154):
#   NVRM: _kgspBootGspRm: unexpected WPR2 already up, cannot proceed
#   NVRM: GPU<N> _kgspRpcGspEventPmuHalted: Received signal from GSP that PMU has halted.
#   NVRM: Xid (PCI:<bdf>): 154, GPU recovery action changed ... to 0x1 (PF FLR)
#   NVRM: RmInitAdapter: Cannot initialize GSP firmware RM / RmInitAdapter failed
#   nvidia <bdf>: AMD-Vi: Event logged [IO_PAGE_FAULT ...]  (5 s flood)
# The driver is ALIVE in this wedge (nvidia-smi answers for the OTHER GPUs);
# nvidia-smi -r refuses forever ("In use by another client" — the driver itself
# is the client holding a half-initialized GPU). RECOVERABLE: stop the DM,
# unload the CLIENT modules (nvidia stays — unloading it would take every
# healthy GPU down too), unbind, reset (SBR for the WPR2 class; the Xid 154
# "PF FLR" class recovers with plain FLR), rebind.
diag_has() { [[ " ${DIAG_CODES[*]} " == *" $1 "* ]]; }
diag_add() { diag_has "$1" || DIAG_CODES+=("$1"); return 0; }

refresh_diagnosis() {
  local log stuck x154 re
  # shellcheck disable=SC2154  # re is assigned right below
  re='to (0x[0-9a-fA-F]+) \(([^)]+)\)'
  DIAG_CODES=()
  DIAG_XID154_ACTION=""
  DIAG_STUCK_WRITERS=0
  log="$(dmesg 2>/dev/null || true)"

  if grep -q "WPR2 already up" <<<"$log"; then diag_add "GPU_GSP_WPR2_STUCK"; fi
  if grep -q "_kgspRpcGspEventPmuHalted" <<<"$log" \
      || grep -Eq "Xid \(PCI:${gpu_pci_short}\): 62," <<<"$log"; then
    diag_add "GPU_PMU_HALT"
  fi
  if grep -q "RmInitAdapter.*failed" <<<"$log"; then diag_add "GPU_GSP_RMINIT_FAIL"; fi
  if diag_has "GPU_GSP_WPR2_STUCK" || diag_has "GPU_PMU_HALT" \
      || diag_has "GPU_GSP_RMINIT_FAIL"; then
    diag_add "GPU_GSP_WEDGE"
  fi

  # Xid 154: the GPU's own recovery-action demand — deliver exactly that reset
  # class (0x1 PF FLR recovered the 2026-11-21 3090; the WPR2 class needs SBR).
  x154="$(grep -E "Xid \(PCI:${gpu_pci_short}\): 154," <<<"$log" | tail -n 1 || true)"
  if [[ -n "$x154" && "$x154" =~ $re ]]; then
    DIAG_XID154_ACTION="${BASH_REMATCH[1]} (${BASH_REMATCH[2]})"
    diag_add "GPU_RESET_REQUESTED_BY_DRIVER"
  fi

  if grep -qi "RC watchdog: GPU is probably locked" <<<"$log"; then
    diag_add "GPU_RC_WATCHDOG_LOCK"
  fi
  if grep -qiE "non-zero usage count.*${gpu_pci}" <<<"$log"; then
    diag_add "PCI_DEVICE_USAGE_PINNED"
  fi
  # Driver-side resource-server corruption from a serviced wedge: clients that
  # survived the wedge reference the pre-reset epoch and deadlock deep query
  # paths later (live 2026-11-21: boards that passed nvidia-smi afterwards
  # locked the whole stack — both GPUs plus nvidia-smi itself — on the first
  # deep query, with rs_client/rs_server asserts in the log).
  if grep -Eq "nvAssertFailedNoLog.*(rs_client|rs_server)\.c" <<<"$log"; then
    diag_add "HOST_RS_RESIDUE"
  fi
  # IOMMU page-fault flood pinned to this BDF (5 s cadence in the live log)
  if dmesg -T 2>/dev/null | grep "IO_PAGE_FAULT" | grep -q "$gpu_pci"; then
    diag_add "IOMMU_PAGE_FAULT"
  fi

  # D-state sysfs writers pinned to this BDF (recovery-path self-lock);
  # kill -9 cannot clear D-state, and each timed-out retry leaves one more.
  # Require a sysfs path in the cmdline so unrelated D-state processes that
  # merely mention the BDF are not counted.
  stuck="$(ps -eo stat=,cmd= 2>/dev/null | grep -E '^[D]' \
    | grep -F "$gpu_pci" | grep -c '/sys/bus/pci' || true)"
  DIAG_STUCK_WRITERS="$stuck"
  if (( stuck > 0 )); then diag_add "HOST_DRIVER_DRAIN_STUCK"; fi
  return 0
}

print_diagnosis() {
  local codes="none" w
  (( ${#DIAG_CODES[@]} > 0 )) && codes="${DIAG_CODES[*]}"
  echo "DIAGNOSIS: $codes (kernel-log signatures accumulated since boot)"
  [[ -n "$DIAG_XID154_ACTION" ]] \
    && echo "  driver-requested recovery action (Xid 154): $DIAG_XID154_ACTION"
  (( DIAG_STUCK_WRITERS > 0 )) \
    && echo "  D-state sysfs writers on $gpu_pci: $DIAG_STUCK_WRITERS"
  if (( ${#ABANDONED_WRITERS[@]} > 0 )); then
    echo "  writers abandoned on deadline: ${#ABANDONED_WRITERS[@]} (may complete late or pin the device)"
    for w in "${ABANDONED_WRITERS[@]}"; do echo "    $w"; done
  fi
  return 0
}

# Recovery-path self-lock (2026-11-21 postmortem, refined live the same day):
# a sysfs writer stuck in D-state inside the nvidia driver holds the DEVICE
# lock — and the lock survives the device becoming unbound, because the
# binding state does not matter to the lock. Tiers 2-5 and 6b all take that
# lock, so retrying them only piles on more stuck writers. Fires on ANY stuck
# writers for this BDF, including ones inherited from earlier runs (live: 7
# inherited writers made an unbound-device reset wedge 5 more).
stuck_writer_selflock() {
  refresh_diagnosis
  diag_has "HOST_DRIVER_DRAIN_STUCK" || return 1
  echo "  self-lock: $DIAG_STUCK_WRITERS D-state sysfs writer(s) pinned to $gpu_pci — they"
  echo "  hold the device lock (bound or not); tiers 2-5 and 6b would all wedge behind them."
  echo "  Fix the holders (--kill-holders) or wait for the writers to drain; Tier 6a slot"
  echo "  power / IPMI power cycle remain the only bypasses."
  return 0
}

unload_client_modules() {
  # peermem -> uvm -> drm -> modeset: reverse of the dependency stack.
  # nvidia itself is deliberately NOT in this list (GPU 0 keeps running).
  # rmmod, never `modprobe -r`: kmod's -r also removes now-unused dependencies,
  # and with drm/modeset gone plus no open fds, dropping nvidia_uvm cascades
  # nvidia itself — every GPU unbinds, the driver dir vanishes, and the next
  # unbind hits ENOENT (live 2026-11-21, dual RTX 3090: /proc/modules showed
  # refcount 1 held by nvidia_uvm alone while the healthy GPU stayed bound —
  # bound devices hold no module reference, so nothing else blocks the cascade).
  local m
  for m in nvidia_peermem nvidia_uvm nvidia_drm nvidia_modeset; do
    lsmod | grep -q "^${m} " || continue
    if run_deadline "$TIMEOUT_MOD" "rmmod $m" -- rmmod "$m"; then
      echo "  unloaded $m"
      UNLOADED_CLIENTS+=("$m")
    else
      echo "  unload $m failed (client fds still open — rerun with --kill-holders)"
    fi
  done
}

tier2_gsp_bus_reset() {
  echo "=== Tier 2: GSP-wedge reset (RESET FIRST -> unload clients -> unbind -> rebind) ==="
  # RESET FIRST: the sysfs reset goes through the PCI layer and never touches
  # the driver's usage count, so it is the ONLY operation that can fire while
  # fd holders are alive. A successful reset reboots the GSP; the stuck
  # clients' ioctls error out and drain, which frees the usage count and
  # un-wedges the unbind/rebind afterwards. The 2026-11-21 unbind-first order
  # wedged the writer inside nvidia's channel teardown and starved the reset
  # behind the same device lock — three abandoned writers for nothing.
  quiesce_display_manager
  quiesce_docker_containers
  unload_client_modules
  # Prefer the SECONDARY BUS reset: the live proof reset a board whose FLR
  # class GSP state (WPR2) survived; "bus" clears it. reset_method holds the
  # SUPPORTED list when read; restore the kernel default afterwards. A list
  # without "bus" is fine for the Xid 154 "recovery action PF FLR" wedge
  # class — plain FLR recovered it (live 2026-11-21 RTX 3090, supported: flr).
  local rm_supported reset_kind="bus" rrc=0
  rm_supported="$(cat "/sys/bus/pci/devices/$gpu_pci/reset_method" 2>/dev/null || true)"
  if [[ "$rm_supported" == *bus* ]]; then
    echo "  reset_method: bus (supported: ${rm_supported//$'\n'/ })"
    echo "bus" > "/sys/bus/pci/devices/$gpu_pci/reset_method" 2>/dev/null || true
  else
    reset_kind="device default (${rm_supported:-unknown})"
    echo "  reset_method: $reset_kind"
  fi
  run_deadline "$TIMEOUT_SYSFS" "PCI reset" -- \
    bash -c "echo 1 > '/sys/bus/pci/devices/$gpu_pci/reset'" || rrc=$?
  echo "default" > "/sys/bus/pci/devices/$gpu_pci/reset_method" 2>/dev/null || true
  if (( rrc == 124 )); then
    # A stuck sysfs writer holds the device lock — the self-lock class. The
    # ladder-level guard should normally have caught this; treat the race the
    # same way: nothing below can take the lock, so skip the rest.
    echo "  reset wedged — a stuck writer holds the device lock (self-lock race)"
    HOLDGATE=1
    return 1
  elif (( rrc != 0 )); then
    echo "  reset failed ($rrc) — attempting unbind/rebind anyway"
  else
    echo "  reset issued (method: $reset_kind) — waiting for GSP boot and client drain"
  fi
  sleep 5
  # Unbind/rebind AFTER the reset: with the GSP rebooted, the stuck clients
  # have drained and the usage count is free. If holders somehow persist,
  # keep the device bound — the driver re-initializes the adapter on next
  # access after an external reset, so the health check still means something.
  local bound=0
  [[ "$(readlink "/sys/bus/pci/devices/$gpu_pci/driver" 2>/dev/null)" == */drivers/nvidia ]] && bound=1
  if (( bound )); then
    if holders_gate "tier 2 unbind (post-reset)"; then
      local rc=0
      run_deadline "$TIMEOUT_SYSFS" "unbind $gpu_pci" -- \
        bash -c "echo '$gpu_pci' > '$DRV_DIR/unbind'" || rc=$?
      if (( rc == 124 )); then
        echo "  unbind wedged — keeping the device bound (in-place recovery after the reset)"
        IDX="$(pci_to_index || true)"
        if [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then return 0; fi
        echo "  reset applied but GPU not healthy (device still bound)"
        return 1
      elif (( rc != 0 )); then
        echo "  unbind failed ($rc) — attempting health check with the device as-is"
        IDX="$(pci_to_index || true)"
        if [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then return 0; fi
        echo "  reset applied but GPU not healthy"
        return 1
      fi
      echo "  unbound"
    else
      echo "  (holders alive — relying on the reset's in-place driver recovery)"
      IDX="$(pci_to_index || true)"
      if [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then return 0; fi
      echo "  reset applied but GPU not healthy (holders persist)"
      return 1
    fi
  fi
  if [[ ! -d "$DRV_DIR" ]]; then
    echo "  nvidia driver dir missing — reloading nvidia"
    run_deadline "$TIMEOUT_MOD" "modprobe nvidia" -- modprobe nvidia || true
  fi
  if [[ "$(readlink "/sys/bus/pci/devices/$gpu_pci/driver" 2>/dev/null)" != */drivers/nvidia && -d "$DRV_DIR" ]]; then
    run_deadline "$TIMEOUT_SYSFS" "rebind $gpu_pci" -- \
      bash -c "echo '$gpu_pci' > '$DRV_DIR/bind'" || true
    sleep 2
  fi
  echo "  waiting for RmInitAdapter (GSP boot takes seconds)"
  sleep 5
  IDX="$(pci_to_index || true)"
  if [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then return 0; fi
  echo "  reset applied but GPU not healthy"
  return 1
}

tier3_flr_rebind() {
  echo "=== Tier 3: unbind -> PCI FLR -> rebind ==="
  local bound=0
  if [[ "$(readlink "/sys/bus/pci/devices/$gpu_pci/driver" 2>/dev/null)" == */drivers/nvidia ]]; then
    bound=1
  else
    # Unbound devices need no unbind. Xid 154 "recovery action 0x1 (PF FLR)"
    # is the GSP asking for exactly this reset class (live 2026-11-21, RTX
    # 3090 PMU-halt wedge) — skipping on unbound would skip the sanctioned fix.
    echo "  device not bound to nvidia — FLR needs no bind, continuing"
  fi
  if (( bound )); then
    local rc=0
    if ! holders_gate "tier 3 unbind"; then
      return 1
    fi
    run_deadline "$TIMEOUT_SYSFS" "unbind $gpu_pci" -- \
      bash -c "echo '$gpu_pci' > '$DRV_DIR/unbind'" || rc=$?
    if (( rc == 124 )); then
      echo "  unbind wedged — skipping this tier's FLR (same device lock)"
      return 1
    elif (( rc != 0 )); then
      echo "  unbind did not complete ($rc) — proceeding to the FLR anyway"
    else
      echo "  unbound"
    fi
  fi
  sleep 2
  if run_deadline "$TIMEOUT_SYSFS" "PCI FLR reset" -- \
      bash -c "echo 1 > '/sys/bus/pci/devices/$gpu_pci/reset'"; then
    echo "  FLR issued"
  else
    echo "  FLR failed/unsupported — attempting rebind anyway"
  fi
  sleep 2
  if [[ ! -d "$DRV_DIR" ]]; then
    echo "  nvidia driver dir missing — reloading nvidia"
    run_deadline "$TIMEOUT_MOD" "modprobe nvidia" -- modprobe nvidia || true
  fi
  if [[ "$(readlink "/sys/bus/pci/devices/$gpu_pci/driver" 2>/dev/null)" != */drivers/nvidia && -d "$DRV_DIR" ]]; then
    if ! run_deadline "$TIMEOUT_SYSFS" "rebind $gpu_pci" -- \
        bash -c "echo '$gpu_pci' > '$DRV_DIR/bind'"; then
      echo "  rebind failed"
      return 1
    fi
  fi
  sleep 2
  IDX="$(pci_to_index || true)"
  if [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then return 0; fi
  echo "  bound but not healthy"
  return 1
}

# GPU wedged BELOW the driver: a D-state writer inside the driver's
# remove/probe path, or the RC-watchdog + usage-count pair — the driver
# cannot tear down a GPU that stopped answering, so no unload or PCI trick
# through the driver can clear it. Only a host reboot (or Tier 6 slot power,
# which bypasses the driver) re-initializes the stack from scratch.
gpu_wedged_below_driver() {
  refresh_diagnosis
  diag_has "HOST_DRIVER_DRAIN_STUCK" && return 0
  if diag_has "GPU_RC_WATCHDOG_LOCK" && diag_has "PCI_DEVICE_USAGE_PINNED"; then
    return 0
  fi
  return 1
}

finish_wedged() { # $1 = tier label for the ledger
  local tier="${1:-final}"
  echo
  echo "=== Final state ==="
  echo "Recent NVRM/Xid lines:"
  dmesg 2>/dev/null | grep -Ei 'NVRM|Xid' | tail -n 15 || true
  refresh_diagnosis
  print_diagnosis
  report_holders
  restore_persistenced
  restore_docker_containers
  restore_display_manager
  restore_client_modules
  ledger_append "wedged" "$tier"
  echo "RECOVERY: IMPOSSIBLE FROM THE HOST —"
  if diag_has "HOST_DRIVER_DRAIN_STUCK"; then
    echo "  $gpu_pci has $DIAG_STUCK_WRITERS sysfs writer(s) stuck in D-state inside the driver's"
    echo "  remove/probe/reset path — they hold the device lock; every further write wedges."
    if holder_dstate || (( HOLD_KILL_FAILED )); then
      # holder_dstate is a single ps sample and the client can oscillate
      # between D and S; a proven kill failure is the stronger evidence
      # (live 2026-11-21: the verdict flipped between runs on sampling alone)
      echo "  CIRCULAR DEADLOCK: fd holders survived -KILL (stuck in D-state ioctls on this"
      echo "  GPU — SIGKILL is pending forever), the fds pin the usage count, the count pins"
      echo "  every reset, and the reset is what would let the ioctls finish."
      echo "  NO HOST-SIDE SOFTWARE SEQUENCE CAN BREAK THIS CYCLE."
    else
      echo "  Kill the fd holders (--kill-holders) and rerun — once the holders exit, even Tier 1"
      echo "  (-r) alone may finish the recovery; if writers persist with no holders, reboot."
    fi
  elif diag_has "GPU_RC_WATCHDOG_LOCK" && diag_has "PCI_DEVICE_USAGE_PINNED"; then
    echo "  $gpu_pci is wedged below the driver (RC watchdog hard lock + non-zero usage count"
    echo "  pinning the PCI device). Every tier above is structurally unable to touch it: the"
    echo "  driver cannot release or unload over a GPU that stopped answering."
  else
    echo "  $gpu_pci is wedged below the driver (signatures: ${DIAG_CODES[*]:-unknown})."
  fi
  echo "  HOST REBOOT REQUIRED — and if still wedged after a warm reboot, power-cycle:"
  echo "    locally:   ipmitool chassis power cycle                            (session dies with the host)"
  echo "    headless:  ipmitool -I lanplus -H <bmc> -U <user> -P <pass> chassis power cycle"
  exit 3
}

tier4_remove_rescan() {
  echo "=== Tier 4: PCI remove + rescan ==="
  # removing a BOUND device goes through the driver's remove path (device
  # lock + usage count) — gated like the other driver-path ops; removing an
  # already-unbound device is lock-free and proceeds even with stale holders
  if [[ "$(readlink "/sys/bus/pci/devices/$gpu_pci/driver" 2>/dev/null)" == */drivers/nvidia ]] \
      && ! holders_gate "tier 4 remove (bound device)"; then
    return 1
  fi
  local remove_ok=0
  # The kernel parses this write with kstrtoul and requires exactly "1" — the
  # old BDF write returned -EINVAL on every attempt, so remove never ran and
  # the wedged-below-driver "non-zero usage count" signature could never fire.
  if run_deadline "$TIMEOUT_SYSFS" "remove $gpu_pci" -- \
      bash -c "echo 1 > '/sys/bus/pci/devices/$gpu_pci/remove'"; then
    remove_ok=1
  fi
  if (( ! remove_ok )) && gpu_wedged_below_driver; then
    finish_wedged "tier4-remove"
  fi
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

tier5_module_reload() {
  echo "=== Tier 5: full nvidia module reload (hits every GPU on the host) ==="
  if gpu_wedged_below_driver; then
    echo "  skipped: modules are pinned by the wedged GPU (unload provably fails)"
    return 2
  fi
  local -a removed=()
  local m r
  # rmmod, not `modprobe -r`: same cascade hazard as unload_client_modules —
  # removing uvm first would cascade nvidia away and shuffle the removal set.
  for m in nvidia_peermem nvidia_uvm nvidia_drm nvidia_modeset nvidia; do
    lsmod | grep -q "^${m} " || continue
    if run_deadline "$TIMEOUT_MOD" "rmmod $m" -- rmmod "$m"; then
      removed+=("$m")
      UNLOADED_CLIENTS+=("$m")
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
  # modprobe nvidia_uvm pulls nvidia back as a dependency, but if only nvidia
  # itself was unloaded nothing else re-registers the PCI driver — ensure the
  # driver dir exists before judging health.
  if [[ ! -d "$DRV_DIR" ]]; then
    run_deadline "$TIMEOUT_MOD" "modprobe nvidia" -- modprobe nvidia || true
  fi
  IDX="$(pci_to_index || true)"
  if [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then return 0; fi
  echo "  modules reloaded but GPU not healthy"
  return 1
}

# Tier 6: slot power cycle — pure ACPI path that never touches the nvidia
# driver, so it still works when every sysfs write blocks on the driver's
# device lock (the recovery-path self-lock case). Skips gracefully when the
# platform does not expose slot power control.
tier6_slot_power() {
  echo "=== Tier 6: PCIe slot power cycle (ACPI — bypasses the nvidia driver entirely) ==="
  local slot="" d addr
  for d in /sys/bus/pci/slots/*/; do
    [[ -d "$d" ]] || continue
    addr="$(cat "${d}address" 2>/dev/null || true)"
    if [[ "$addr" == "$gpu_pci_short" || "$addr" == "$gpu_pci" ]] && [[ -w "${d}power" ]]; then
      slot="${d%/}"
      break
    fi
  done
  if [[ -z "$slot" ]]; then
    echo "  skipped: no ACPI slot with writable power for $gpu_pci (/sys/bus/pci/slots)"
    return 1
  fi
  echo "  slot: $slot — cutting power (hardware state is lost, driver lock chains break)"
  if ! run_deadline 15 "slot power off" -- bash -c "echo 0 > '$slot/power'"; then
    echo "  power-off write failed/blocked"
    return 1
  fi
  sleep 2
  run_deadline 15 "slot power on" -- bash -c "echo 1 > '$slot/power'" || true
  sleep 2
  run_deadline "$TIMEOUT_SYSFS" "PCI rescan" -- bash -c "echo 1 > '/sys/bus/pci/rescan'" || true
  sleep 3
  if [[ ! -e "/sys/bus/pci/devices/$gpu_pci" ]]; then
    echo "  device did not reappear after slot power cycle"
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

# Tier 6b: RTD3-class power cycle — the Linux equivalent of the Windows-laptop
# "Runtime D3" recovery (D3cold cuts the rails; everything outside the
# always-on/AON domain dies, which is exactly what clears wedge state that
# FLR/SBR cannot reach). Two routes, tried in order:
#   6b-1  the nvidia driver's own runtime PM (NVreg_DynamicPowerManagement,
#         /proc/driver/nvidia/params): idle + power/control=auto ->
#         runtime_status=suspended -> D3cold via the platform's ACPI _PR3.
#         No unbind needed. NOTE: the same cycle wipes volatile unlock state
#         (nv-uefi-unlocker issue #9 — D3cold is what kills the unlock after
#         handoff) — re-apply the unlock afterwards.
#   6b-2  vfio-pci runtime PM (kernel >= 5.19): release the device from
#         nvidia first, then let vfio-pci park it. Works even when the nvidia
#         suspend path refuses; still needs the driver to release the device,
#         so the self-lock class remains Tier 6a / IPMI territory.
# Without ACPI _PR3 (most desktops/servers) both routes stop at D3hot — the
# D3hot -> D0 transition is still one more internal reset vector.
tier6b_rtd3() {
  echo "=== Tier 6b: RTD3-class power cycle ==="
  [[ -e "/sys/bus/pci/devices/$gpu_pci" ]] || { echo "  device absent from the bus"; return 1; }
  refresh_diagnosis
  if diag_has "HOST_DRIVER_DRAIN_STUCK"; then
    echo "  skipped: $DIAG_STUCK_WRITERS D-state writer(s) hold the device lock (self-lock)"
    return 1
  fi
  if tier6b_nvidia_rtd3; then return 0; fi
  echo "  nvidia runtime-PM route unavailable or refused — trying vfio-pci"
  tier6b_vfio_rtd3
}

tier6b_nvidia_rtd3() {
  local pctl="/sys/bus/pci/devices/$gpu_pci/power"
  [[ -w "$pctl/control" ]] || { echo "  6b-1 skipped: no power/control on $gpu_pci"; return 1; }
  if [[ "$(readlink "/sys/bus/pci/devices/$gpu_pci/driver" 2>/dev/null)" != */drivers/nvidia ]]; then
    echo "  6b-1 skipped: device not bound to nvidia (runtime suspend is a driver path)"
    return 1
  fi
  if ! holders_gate "tier 6b-1 (nvidia runtime suspend)"; then return 1; fi
  local dpm val
  dpm="$(grep -E '^DynamicPowerManagement:' /proc/driver/nvidia/params 2>/dev/null | head -n1 || true)"
  val="${dpm##*: }"
  if [[ -z "$val" ]]; then
    echo "  6b-1 skipped: /proc/driver/nvidia/params absent (driver not loaded)"
    return 1
  fi
  echo "  DPM: $dpm"
  if [[ "$val" == "0x00" || "$val" == "0x0" || "$val" == "0" ]]; then
    echo "  6b-1 skipped: NVreg_DynamicPowerManagement=$val (RTD3 disabled — see disable-rtd3.sh)"
    return 1
  fi
  # deadline-guarded writes: a wedged suspend path can hang the writer, and a
  # writer stuck in "suspending" holds the runtime-PM lock for the resume too
  run_deadline 10 "power/control -> auto" -- bash -c "echo auto > '$pctl/control'" 2>/dev/null || true
  local waited=0 suspended=0
  while (( waited < 25 )); do
    if [[ "$(cat "$pctl/runtime_status" 2>/dev/null)" == "suspended" ]]; then
      suspended=1
      break
    fi
    sleep 1
    waited=$((waited + 1))
  done
  if (( ! suspended )); then
    echo "  device did not auto-suspend in 25s (runtime_status: $(cat "$pctl/runtime_status" 2>/dev/null || echo '?'))"
    run_deadline 10 "power/control -> on" -- bash -c "echo on > '$pctl/control'" 2>/dev/null || true
    return 1
  fi
  # Do NOT poke config space to distinguish D3cold from D3hot: any config
  # access on a runtime-suspended PCI device auto-resumes it (kernel
  # pm_runtime_get_sync in the config reader) and defeats the cycle.
  echo "  runtime-suspended after ${waited}s — D3cold (rails cut) if ACPI _PR3 backs the device, else D3hot"
  run_deadline 10 "power/control -> on" -- bash -c "echo on > '$pctl/control'" 2>/dev/null || true
  local w2=0
  while (( w2 < 15 )); do
    [[ "$(cat "$pctl/runtime_status" 2>/dev/null)" == "active" ]] && break
    sleep 1
    w2=$((w2 + 1))
  done
  sleep 3   # adapter re-init + GSP boot
  IDX="$(pci_to_index || true)"
  if [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then return 0; fi
  echo "  resumed but not healthy"
  return 1
}

tier6b_vfio_rtd3() {
  if [[ "$(readlink "/sys/bus/pci/devices/$gpu_pci/driver" 2>/dev/null)" == */drivers/nvidia ]]; then
    if ! holders_gate "tier 6b-2 unbind"; then return 1; fi
    local rc=0
    run_deadline "$TIMEOUT_SYSFS" "unbind $gpu_pci" -- \
      bash -c "echo '$gpu_pci' > '$DRV_DIR/unbind'" || rc=$?
    if (( rc != 0 )); then
      echo "  unbind did not complete ($rc) — RTD3 route needs the driver to release the device"
      return 1
    fi
    echo "  unbound"
  fi
  if ! run_deadline "$TIMEOUT_MOD" "modprobe vfio_pci" -- modprobe vfio_pci; then
    echo "  skipped: vfio_pci unavailable on this kernel"
    return 1
  fi
  local cur
  cur="$(readlink "/sys/bus/pci/devices/$gpu_pci/driver" 2>/dev/null || true)"
  if [[ -n "$cur" ]]; then
    echo "  releasing current driver ($(basename "$cur"))"
    run_deadline "$TIMEOUT_SYSFS" "unbind from $(basename "$cur")" -- \
      bash -c "echo '$gpu_pci' > '$cur/unbind'" || true
  fi
  if ! run_deadline "$TIMEOUT_SYSFS" "bind $gpu_pci to vfio-pci" -- \
      bash -c "echo '$gpu_pci' > '/sys/bus/pci/drivers/vfio-pci/bind'"; then
    echo "  vfio-pci bind refused"
    return 1
  fi
  local pctl="/sys/bus/pci/devices/$gpu_pci/power"
  run_deadline 10 "power/control -> auto" -- bash -c "echo auto > '$pctl/control'" 2>/dev/null || true
  local waited=0 suspended=0
  while (( waited < 25 )); do
    if [[ "$(cat "$pctl/runtime_status" 2>/dev/null)" == "suspended" ]]; then
      suspended=1
      break
    fi
    sleep 1
    waited=$((waited + 1))
  done
  if (( suspended )); then
    echo "  runtime-suspended after ${waited}s — D3cold when ACPI _PR3 backs the device, else D3hot"
  else
    echo "  device did not auto-suspend (runtime_status: $(cat "$pctl/runtime_status" 2>/dev/null || echo '?')) — cycling anyway"
  fi
  run_deadline 10 "power/control -> on" -- bash -c "echo on > '$pctl/control'" 2>/dev/null || true
  sleep 2
  run_deadline "$TIMEOUT_SYSFS" "unbind $gpu_pci from vfio-pci" -- \
    bash -c "echo '$gpu_pci' > '/sys/bus/pci/drivers/vfio-pci/unbind'" || true
  sleep 1
  run_deadline "$TIMEOUT_SYSFS" "PCI rescan" -- bash -c "echo 1 > '/sys/bus/pci/rescan'" || true
  sleep 3
  if [[ ! -e "/sys/bus/pci/devices/$gpu_pci" ]]; then
    echo "  device did not reappear after RTD3 cycle"
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

# One JSON line per run for the OC-scan fault -> recovery-level dataset.
ledger_append() { # $1 result, $2 tier
  [[ -n "$LEDGER_PATH" ]] || return 0
  local codes="-"
  (( ${#DIAG_CODES[@]} > 0 )) && codes="${DIAG_CODES[*]}"
  printf '{"ts":"%s","bdf":"%s","result":"%s","tier":"%s","diagnosis":"%s","xid154_action":"%s","stuck_writers":%d,"abandoned_writers":%d,"had_holders":%d,"duration_s":%d}\n' \
    "$(date -Is 2>/dev/null || date +%Y-%m-%dT%H:%M:%S)" \
    "$gpu_pci" "$1" "$2" "$codes" \
    "${DIAG_XID154_ACTION:--}" "$DIAG_STUCK_WRITERS" \
    "${#ABANDONED_WRITERS[@]}" "$HAD_HOLDERS" "$(( SECONDS - START_SECONDS ))" \
    >>"$LEDGER_PATH" 2>/dev/null \
    || echo "  (ledger write to $LEDGER_PATH failed)"
  return 0
}

# Clean-recovery contract, clause 2: the GPU side rebuilds via FLR/SBR/RTD3,
# but the driver's client/resource-server state has exactly ONE rebuild path —
# a full module-stack reload with every fd closed. Skip it and the board
# passes nvidia-smi while carrying residue that detonates the whole stack on
# the first deep query (live 2026-11-21: two "recovered" 3090s, one NVAPI
# read, both cards plus nvidia-smi hard-locked). Purge is default-ON and
# skips itself (with an explicit PARTIAL marking) when any nvidia node is
# still held — rmmod would fail and other GPUs' clients must not be murdered
# as a side effect.
purge_driver_state() { # exit 0 = purge ran (or nothing needed rebuild)
  local m removed=() held=0 n
  for n in /dev/nvidia[0-9]* /dev/nvidiactl /dev/nvidia-uvm /dev/nvidia-modeset; do
    [[ -e "$n" ]] || continue
    if command -v fuser >/dev/null 2>&1 && fuser "$n" >/dev/null 2>&1; then
      echo "  purge skipped: $n still held (pids: $(fuser "$n" 2>/dev/null | tr -s ' '))"
      held=1
    fi
  done
  (( held )) && return 1
  echo "Purging driver-side residue: full module stack reload (rs/client state rebuilt)"
  for m in nvidia_peermem nvidia_uvm nvidia_drm nvidia_modeset nvidia; do
    if lsmod | grep -q "^${m} "; then
      if run_deadline "$TIMEOUT_MOD" "rmmod $m" -- rmmod "$m"; then
        removed+=("$m")
      fi
    fi
  done
  if [[ " ${removed[*]} " != *" nvidia "* ]]; then
    echo "  purge skipped: nvidia itself could not be unloaded (nothing to rebuild)"
    return 1
  fi
  run_deadline "$TIMEOUT_MOD" "modprobe nvidia" -- modprobe nvidia \
    || { echo "  reload failed — residue may remain"; return 1; }
  for m in nvidia_modeset nvidia_drm nvidia_uvm nvidia_peermem; do
    if [[ " ${removed[*]} " == *" $m "* ]]; then
      run_deadline "$TIMEOUT_MOD" "modprobe $m" -- modprobe "$m" || true
    fi
  done
  sleep 5
  return 0
}

finish_ok() { # $1 = tier label for the ledger
  local tier="${1:-unknown}" purged=0 result="success"
  echo
  echo "=== Final state ==="
  refresh_diagnosis
  print_diagnosis
  nvidia-smi || true
  if (( PURGE )) && [[ "$tier" != "tier5" ]]; then
    if purge_driver_state; then
      purged=1
      restore_client_modules
      IDX="$(pci_to_index || true)"
      if [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then
        echo "  post-purge health: OK — driver-side state rebuilt (clean recovery)"
      else
        echo "  post-purge health: FAILED — the board did not survive a driver rebuild:"
        echo "  this 'recovery' was fake (card-side residue that FLR-class resets cannot reach)."
        echo "  Rerun the ladder, power-cycle via IPMI, or reboot."
        result="post-purge-failed"
      fi
    else
      echo "  purge skipped — fd holders remain, driver-side residue may persist. Treat this"
      echo "  as PARTIAL: re-verify with a real client (--probe-cmd) before work."
      result="success-residue-risk"
    fi
  fi
  if (( HAD_HOLDERS )) && (( ! purged )); then
    echo "RESIDUE WARNING: this GPU had live clients through the wedge/recovery — the driver's"
    echo "  client/resource-server state may still reference the pre-wedge epoch (live 2026-11-21:"
    echo "  boards that passed nvidia-smi afterwards deadlocked the whole stack on the first"
    echo "  deep query). Re-verify with a real client (--probe-cmd) before work."
  fi
  restore_persistenced
  restore_docker_containers
  restore_display_manager
  restore_client_modules
  ledger_append "$result" "$tier"
  if [[ "$result" == "post-purge-failed" ]]; then
    echo "RECOVERY: INVALID — post-purge re-verification failed; the GPU-side fix did not hold."
    exit 1
  fi
  echo "RECOVERY: SUCCESS"
  exit 0
}

finish_fail() {
  echo
  echo "=== Final state ==="
  echo "Recent NVRM/Xid lines:"
  dmesg 2>/dev/null | grep -Ei 'NVRM|Xid' | tail -n 15 || true
  refresh_diagnosis
  print_diagnosis
  report_holders
  restore_persistenced
  restore_docker_containers
  restore_display_manager
  restore_client_modules
  if [[ -n "$(gpu_node_holders)" ]] && { holder_dstate || (( HOLD_KILL_FAILED )); }; then
    echo "RECOVERY: CIRCULAR DEADLOCK — fd holders survived -KILL (stuck in D-state ioctls on"
    echo "  this GPU); their fds pin every reset path."
    echo "  HOST REBOOT / IPMI POWER CYCLE REQUIRED."
    ledger_append "circular-deadlock" "ladder-exhausted"
  elif [[ -n "$(gpu_node_holders)" ]]; then
    echo "RECOVERY: BLOCKED BY FD HOLDERS — rerun with --kill-holders; once the holders exit,"
    echo "  even Tier 1 (-r) alone may finish the recovery."
    ledger_append "blocked-by-holders" "ladder-exhausted"
  else
    echo "RECOVERY: FAILED — remaining option is a host reboot."
    ledger_append "failed" "ladder-exhausted"
  fi
  nvidia-smi || true
  exit 1
}

# ---- multi-card orchestration (parent mode) ------------------------------------
# The clean-recovery contract makes the driver-side purge a GLOBAL operation
# (one full module reload rebuilds every card's rs/client state), so N wedged
# cards should not pay N purges. Parent mode: one GPU-side reset per card
# (this same script re-invoked per BDF with --single), then exactly ONE purge,
# then per-card re-verification. Exit code = worst child verdict.

detect_wedged_bdfs() { # prints candidate wedged BDFs, one per line
  # Every nvidia-smi call here is deadline-guarded: in a driver-side lockup
  # (the residue detonation) NVML itself hangs, and an unguarded call wedges
  # the detector before it ever finds anything (live 2026-11-21).
  local smi idx bus fields d link bdf vend cls
  smi="$(run_deadline "$TIMEOUT_PROBE" "" -- \
    nvidia-smi --query-gpu=pci.bus_id --format=csv,noheader 2>/dev/null || true)"
  if [[ -z "$smi" ]]; then
    echo "  (nvidia-smi blind or hung — sweeping sysfs for NVIDIA-class cards instead)" 1>&2
  fi
  idx=0
  if [[ -n "$smi" ]]; then
    while IFS= read -r bus; do
      bus="${bus//[[:space:]]/}"
      [[ -z "$bus" ]] && continue
      fields="$(run_deadline "$TIMEOUT_PROBE" "" -- \
        nvidia-smi -i "$idx" --query-gpu=temperature.gpu,utilization.gpu,memory.used \
        --format=csv,noheader 2>/dev/null || true)"
      if [[ "${fields,,}" == *err* ]]; then
        printf '%s\n' "${bus,,}"
      fi
      idx=$((idx + 1))
    done <<<"$smi"
  fi
  # sysfs sweep — catches what nvidia-smi cannot report:
  #   bound to nvidia but NVML-blind  -> the whole stack is sick, flag it
  #   driver loaded, 10de VGA unbound -> failed probe-time init wedge
  for d in /sys/bus/pci/devices/*; do
    [[ -r "$d/vendor" ]] || continue
    vend="$(cat "$d/vendor" 2>/dev/null || true)"
    [[ "$vend" == "0x10de" ]] || continue
    cls="$(cat "$d/class" 2>/dev/null || true)"
    case "$cls" in 0x0300*|0x0302*) ;; *) continue ;; esac
    bdf="${d##*/}"
    link="$(readlink "$d/driver" 2>/dev/null || true)"
    if [[ "$link" == *drivers/nvidia ]]; then
      if [[ -n "$smi" ]] && grep -qiF "$bdf" <<<"$smi"; then
        continue   # NVML sees it and it answered clean — not a candidate
      fi
      printf '%s\n' "${bdf,,}"   # NVML blind/hung, or ERR: candidate
    elif [[ -z "$link" && -d "$DRV_DIR" ]]; then
      printf '%s\n' "${bdf,,}"   # driver loaded, function unclaimed: failed probe
    fi
  done | sort -u
  return 0
}

parent_card_healthy() { # $1 = BDF; 0 when NVML lists it with clean sensor fields
  local want bus t u m fields smi
  want="$(norm_pci "$1" 2>/dev/null)" || return 1
  smi="$(run_deadline "$TIMEOUT_PROBE" "" -- \
    nvidia-smi --query-gpu=pci.bus_id,temperature.gpu,utilization.gpu,memory.used \
    --format=csv,noheader 2>/dev/null || true)"
  [[ -n "$smi" ]] || return 1   # NVML blind = not verifiable = not healthy
  while IFS=, read -r bus t u m; do
    bus="${bus//[[:space:]]/}"
    [[ "$(norm_pci "$bus" 2>/dev/null)" == "$want" ]] || continue
    fields="${t}${u}${m}"
    [[ "${fields,,}" == *err* ]] && return 1
    return 0
  done <<<"$smi"
  return 1   # absent from NVML = not healthy
}

if (( ! SINGLE )); then
  declare -a targets=()
  if (( ${#POSITIONAL[@]} > 0 )); then
    targets=("${POSITIONAL[@]}")
  elif (( ALL_WEDGED )); then
    mapfile -t targets < <(detect_wedged_bdfs)
  else
    echo "no BDF given — pass one or more BDFs, or --all-wedged to auto-detect" 1>&2
    usage
    exit 127
  fi
  if (( ${#targets[@]} == 0 )); then
    echo "no wedged GPUs detected — nothing to recover"
    exit 0
  fi
  t="" rc=0 worst=0 tt=""
  for t in "${targets[@]}"; do
    tt="${t,,}"
    if [[ ! "$tt" =~ ^[0-9a-f]{4}:[0-9a-f]{2}:[0-9a-f]{2}\.[0-9a-f]$ ]]; then
      echo "Bad PCI address in target list: $t" 1>&2
      exit 127
    fi
  done
  echo "=== NVOC GPU recovery — multi-card mode: ${#targets[@]} target(s): ${targets[*]} ==="
  child_args=(--single)
  (( KILL_HOLDERS )) && child_args+=(--kill-holders)
  (( FORCE )) && child_args+=(--force)
  (( PURGE )) || child_args+=(--no-purge)   # the purge runs ONCE, here
  [[ -n "$PROBE_CMD" ]] && child_args+=(--probe-cmd "$PROBE_CMD")
  [[ -n "$LEDGER_PATH" ]] && child_args+=(--ledger "$LEDGER_PATH")
  for t in "${targets[@]}"; do
    echo
    echo "########## card $t ##########"
    # invoke via bash explicitly: the script is routinely run without +x
    # (as `bash ./linux_oc_recover.sh`), so a bare "$0" exec fails EACCES
    # (live 2026-11-21)
    bash "$0" "${child_args[@]}" "$t"
    rc=$?
    (( rc > worst )) && worst=$rc
  done
  if (( PURGE )); then
    echo
    echo "########## global driver-state purge (once for the whole incident) ##########"
    if purge_driver_state; then
      sleep 3
      for t in "${targets[@]}"; do
        if parent_card_healthy "$t"; then
          echo "  $t: healthy after purge — clean recovery"
        else
          echo "  $t: NOT healthy after purge — the GPU-side fix did not hold (card-side residue);"
          echo "    power-cycle via IPMI or reboot."
          (( worst < 1 )) && worst=1
        fi
      done
    else
      echo "  purge skipped (fd holders remain) — child verdicts stand; residue risk is in the ledger"
    fi
  fi
  exit "$worst"
fi

# ===== single-card mode =====
if (( ${#POSITIONAL[@]} != 1 )); then
  echo "single-card mode takes exactly one BDF (multi-card: pass several BDFs or --all-wedged)" 1>&2
  usage
  exit 127
fi
gpu_pci="${POSITIONAL[0]}"
gpu_pci="${gpu_pci,,}"
if [[ ! "$gpu_pci" =~ ^[0-9a-f]{4}:[0-9a-f]{2}:[0-9a-f]{2}\.[0-9a-f]$ ]]; then
  echo "Bad PCI address: $gpu_pci (expected like 0000:01:00.0)" 1>&2
  exit 127
fi
[[ -e "/sys/bus/pci/devices/$gpu_pci" ]] || {
  echo "No such PCI device: $gpu_pci" 1>&2
  exit 127
}
# Xid lines print PCI:<domain:bus:device> without the function, e.g.
# "Xid (PCI:0000:ce:00): 154" — keep the short form for kernel-log matching.
gpu_pci_short="${gpu_pci%.*}"

echo "=== NVOC GPU recovery: $gpu_pci ==="

# Read-only probe first: do not touch a system whose GPU is already fine.
# --force skips this gate entirely and runs the ladder no matter what
# nvidia-smi reports.
IDX="$(pci_to_index || true)"
if (( FORCE )); then
  echo "--force: ignoring health probe, running the recovery ladder regardless"
  if [[ -n "$IDX" ]]; then
    echo "probe said: index $IDX, fields: $(nvidia-smi -i "$IDX" \
      --query-gpu=name,temperature.gpu,utilization.gpu,memory.used \
      --format=csv,noheader 2>/dev/null || echo '<probe failed>')"
  fi
elif [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then
  echo "GPU already responds to NVML (index $IDX) — nothing to recover."
  nvidia-smi -i "$IDX" || true
  ledger_append "already_healthy" "probe"
  exit 0
fi

report_holders
find_docker_gpu_containers
if (( KILL_HOLDERS )) && ls /dev/nvidia* >/dev/null 2>&1; then
  minor="$(gpu_minor_for_bdf)"
  if [[ -n "$minor" && -e "/dev/nvidia$minor" ]]; then
    echo "Killing processes holding THIS GPU's node (/dev/nvidia$minor) — other GPUs' clients untouched"
    fuser -k -KILL "/dev/nvidia$minor" 2>/dev/null || true
  else
    echo "Killing all processes holding /dev/nvidia* (could not resolve this GPU's minor)"
    fuser -k -KILL /dev/nvidia* 2>/dev/null || true
  fi
  sleep 3
  if [[ -n "$(gpu_node_holders)" ]]; then
    echo "  !! holders survived -KILL (D-state clients cannot be reaped — circular deadlock; see verdict)"
    HOLD_KILL_FAILED=1
  fi
fi
quiesce_persistenced
if [[ -d "$DRV_DIR" ]]; then
  IDX="$(pci_to_index || true)"
  echo "NVML index: ${IDX:-<unresolvable — GPU invisible to nvidia-smi>}"
else
  IDX=""
  echo "NVML index: <nvidia driver not loaded — NVML blind (module absent)>"
fi
# holders that SURVIVED the quiesce/kill are the residue risk: their client
# state crosses the wedge epoch and can deadlock deep queries post-recovery
[[ -n "$(gpu_node_holders)" ]] && HAD_HOLDERS=1
refresh_diagnosis
print_diagnosis
if diag_has "GPU_GSP_WEDGE"; then
  echo "GSP-firmware wedge signature present — tier 2 is the proven path"
fi

if tier1_nvml_reset; then finish_ok "tier1"; fi
SELFLOCK=0
if stuck_writer_selflock; then SELFLOCK=1; fi
if (( ! SELFLOCK )); then
  if tier2_gsp_bus_reset; then finish_ok "tier2"; fi
  if (( HOLDGATE )); then
    echo "  tiers 3-5 skipped: fd holders pin the driver path (see above)"
  else
    if tier3_flr_rebind; then finish_ok "tier3"; fi
    if tier4_remove_rescan; then finish_ok "tier4"; fi
    if tier5_module_reload; then finish_ok "tier5"; fi
  fi
fi
if tier6_slot_power; then finish_ok "tier6"; fi
if tier6b_rtd3; then finish_ok "tier6b"; fi
if gpu_wedged_below_driver; then finish_wedged "ladder-end"; fi
finish_fail
