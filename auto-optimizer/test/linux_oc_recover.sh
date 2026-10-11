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
gpu_pci=""
DRV_DIR=/sys/bus/pci/drivers/nvidia
PERSISTED_STOPPED=0
DM_STOPPED=0
DOCKER_GPU_CONTAINERS=()
DOCKER_STOPPED=()
IDX=""

usage() {
  echo "Usage: sudo $0 [--kill-holders] [--force] NV_GPU_PCI_ID (like 0000:01:00.0)" 1>&2
  echo "  --kill-holders  SIGKILL /dev/nvidia* holders and stop docker GPU containers (restarted on exit)" 1>&2
  echo "  --force         skip the health probe, run the ladder regardless" 1>&2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -k|--kill-holders) KILL_HOLDERS=1 ;;
    -f|--force) FORCE=1 ;;
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
  return 0
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

# GSP-firmware wedge: the driver is ALIVE (nvidia-smi answers for the OTHER
# GPUs) but this board's GSP-RM cannot re-initialize — journal signature
# (live 2026-10-12, RTX PRO 6000 / 580.173.02 open module):
#   NVRM: _kgspBootGspRm: unexpected WPR2 already up, cannot proceed
#   NVRM: crashcatWayfinderGetReportQueue_V1: insufficiently-sized L1 ...
#   NVRM: RmInitAdapter: Cannot initialize GSP firmware RM / RmInitAdapter failed
#   nvidia <bdf>: AMD-Vi: Event logged [IO_PAGE_FAULT ...]  (5 s flood)
# nvidia-smi -r refuses forever ("In use by another client" — the driver
# itself is the client holding a half-initialized GPU). RECOVERABLE: stop
# the DM, unload the CLIENT modules (nvidia stays — unloading it would take
# every healthy GPU down too), unbind, SECONDARY BUS reset (deeper than the
# FLR the device may offer; FLR leaves the WPR2 state untouched), rebind.
gpu_gsp_wedged() {
  dmesg 2>/dev/null | grep -q "WPR2 already up" && return 0
  dmesg 2>/dev/null | grep -q "RmInitAdapter.*failed" && return 0
  # IOMMU page-fault flood pinned to this BDF (5 s cadence in the live log)
  dmesg -T 2>/dev/null | grep "IO_PAGE_FAULT" | grep -q "$gpu_pci" && return 0
  return 1
}

unload_client_modules() {
  # peermem -> uvm -> drm -> modeset: reverse of the dependency stack.
  # nvidia itself is deliberately NOT in this list (GPU 0 keeps running).
  local m
  for m in nvidia_peermem nvidia_uvm nvidia_drm nvidia_modeset; do
    lsmod | grep -q "^${m} " || continue
    if run_deadline "$TIMEOUT_MOD" "modprobe -r $m" -- modprobe -r "$m"; then
      echo "  unloaded $m"
    else
      echo "  unload $m failed (client fds still open — rerun with --kill-holders)"
    fi
  done
}

tier2_gsp_bus_reset() {
  echo "=== Tier 2: GSP-wedge bus reset (stop DM -> unload clients -> unbind -> bus reset -> rebind) ==="
  local bound=1
  if [[ "$(readlink "/sys/bus/pci/devices/$gpu_pci/driver" 2>/dev/null)" != */drivers/nvidia ]]; then
    bound=0
    echo "  device not bound to nvidia — skipping unbind"
  fi
  quiesce_display_manager
  quiesce_docker_containers
  unload_client_modules
  if (( bound )); then
    if ! run_deadline "$TIMEOUT_SYSFS" "unbind $gpu_pci" -- \
        bash -c "echo '$gpu_pci' > '$DRV_DIR/unbind'"; then
      echo "  unbind blocked — fd holders still alive; kill them and rerun (--kill-holders)"
      return 1
    fi
    echo "  unbound"
  fi
  sleep 2
  # Prefer the SECONDARY BUS reset: the live proof reset a board whose FLR
  # class GSP state (WPR2) survived; "bus" clears it. reset_method holds the
  # SUPPORTED list when read; restore the kernel default afterwards.
  local rm_supported
  rm_supported="$(cat "/sys/bus/pci/devices/$gpu_pci/reset_method" 2>/dev/null || true)"
  if [[ "$rm_supported" == *bus* ]]; then
    echo "  reset_method: bus (supported: ${rm_supported//$'\n'/ })"
    echo "bus" > "/sys/bus/pci/devices/$gpu_pci/reset_method" 2>/dev/null || true
  else
    echo "  reset_method: device default (${rm_supported:-unknown})"
  fi
  if run_deadline "$TIMEOUT_SYSFS" "PCI bus reset" -- \
      bash -c "echo 1 > '/sys/bus/pci/devices/$gpu_pci/reset'"; then
    echo "  bus reset issued"
  else
    echo "  reset failed — attempting rebind anyway"
  fi
  echo "default" > "/sys/bus/pci/devices/$gpu_pci/reset_method" 2>/dev/null || true
  sleep 2
  if [[ "$(readlink "/sys/bus/pci/devices/$gpu_pci/driver" 2>/dev/null)" != */drivers/nvidia && -d "$DRV_DIR" ]]; then
    if ! run_deadline "$TIMEOUT_SYSFS" "rebind $gpu_pci" -- \
        bash -c "echo '$gpu_pci' > '$DRV_DIR/bind'"; then
      echo "  rebind failed"
      return 1
    fi
  fi
  echo "  waiting for RmInitAdapter (GSP boot takes seconds)"
  sleep 5
  IDX="$(pci_to_index || true)"
  if [[ -n "$IDX" ]] && gpu_healthy "$IDX"; then return 0; fi
  echo "  bound but not healthy"
  return 1
}

tier3_flr_rebind() {
  echo "=== Tier 3: unbind -> PCI FLR -> rebind ==="
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

# GPU wedged BELOW the driver: the NVRM usage count pins the PCI device
# because the driver cannot tear down a GPU that stopped answering (the
# RC watchdog is the teardown attempt failing). No unload or PCI trick
# can clear it — only a host reboot re-initializes GSP-RM from scratch.
gpu_wedged_below_driver() {
  # signature 1: D-state writers stuck in the nvidia probe on THIS device —
  # each prior recovery run leaves one more (kill -9 cannot clear D-state)
  local stuck
  stuck=$(ps -eo stat=,cmd= 2>/dev/null | grep -E '^[D]' | grep -c "echo '$gpu_pci' > .*bind" || true)
  (( stuck > 0 )) && return 0
  # signature 2: RC watchdog firing (GPU stopped answering channel preemption)
  local wedged
  wedged=$(dmesg 2>/dev/null | grep -ci 'RC watchdog: GPU is probably locked' || true)
  # signature 3: NVRM refuses PCI remove on this BDF (non-zero usage count)
  local recent
  recent=$(dmesg -T 2>/dev/null | grep -i "non-zero usage count.*$gpu_pci" | tail -n 1 || true)
  (( wedged >= 3 )) && [[ -n "$recent" ]]
}

finish_wedged() {
  echo
  echo "=== Final state ==="
  echo "Recent NVRM/Xid lines:"
  dmesg 2>/dev/null | grep -Ei 'NVRM|Xid' | tail -n 15 || true
  report_holders
  restore_persistenced
  restore_docker_containers
  restore_display_manager
  echo "RECOVERY: IMPOSSIBLE FROM THE HOST —"
  echo "  $gpu_pci is wedged below the driver (RC watchdog hard lock + non-zero"
  echo "  usage count pinning the PCI device). Every tier above is structurally"
  echo "  unable to touch it: the driver cannot release or unload over a GPU"
  echo "  that stopped answering. HOST REBOOT REQUIRED"
  echo "  (if the card is still wedged after a warm reboot, power-cycle the host)."
  exit 3
}

tier4_remove_rescan() {
  echo "=== Tier 4: PCI remove + rescan ==="
  local remove_ok=0
  if run_deadline "$TIMEOUT_SYSFS" "remove $gpu_pci" -- \
      bash -c "echo '$gpu_pci' > '/sys/bus/pci/devices/$gpu_pci/remove'"; then
    remove_ok=1
  fi
  if (( ! remove_ok )) && gpu_wedged_below_driver; then
    finish_wedged
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
  restore_docker_containers
  restore_display_manager
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
  restore_docker_containers
  restore_display_manager
  nvidia-smi || true
  echo "RECOVERY: FAILED — remaining option is a host reboot."
  exit 1
}

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
  exit 0
fi

report_holders
find_docker_gpu_containers
if (( KILL_HOLDERS )) && ls /dev/nvidia* >/dev/null 2>&1; then
  echo "Killing all processes holding /dev/nvidia*"
  fuser -k -KILL /dev/nvidia* 2>/dev/null || true
  sleep 3
fi
quiesce_persistenced
IDX="$(pci_to_index || true)"
echo "NVML index: ${IDX:-<unresolvable — GPU invisible to nvidia-smi>}"
if gpu_gsp_wedged; then
  echo "GSP-firmware wedge signature present (WPR2/RmInitAdapter/IOMMU faults) — tier 2 is the proven path"
fi

if tier1_nvml_reset; then finish_ok; fi
if tier2_gsp_bus_reset; then finish_ok; fi
if tier3_flr_rebind; then finish_ok; fi
if tier4_remove_rescan; then finish_ok; fi
if tier5_module_reload; then finish_ok; fi
if gpu_wedged_below_driver; then finish_wedged; fi
finish_fail
