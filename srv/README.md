# NVOC-SRV — closed-loop GPU control service

`nvoc_service` is a long-lived control service for NVIDIA GPUs. Its job is
**closed-loop control**: today a fan-speed PID that regulates GPU temperature;
the controller framework is the intended home for future target-optimization
loops (power, clocks) and for exposing safe control to the stressor / MCP
layers.

One-shot writes and inspection remain in `nvoc cli`; the service owns
**continuous** control with failsafes.

## Architecture

```
┌────────────────────────────────────────────────────────────────────┐
│ nvoc_service (one process)                                         │
│                                                                    │
│  HTTP control plane (tiny_http, 127.0.0.1:14514, CSRF-gated)       │
│        │ config edits / imperative commands (flume)                │
│        ▼                                                           │
│  control loop (std thread, tick = interval_ms, default 1 s)        │
│    per GPU: GpuController ── PidController (pure, unit-tested)     │
│        │  mode: Auto | Pid | Manual                                │
│        │  failsafe: read-failure restore / emergency 100%          │
│        ▼                                                           │
│  NvapiBackend  (SetFanPercent 0xEB44E8AA RMW → NVML fallback)      │
│  watchdog thread (heartbeat > 30 s → restore driver fan control)   │
└────────────────────────────────────────────────────────────────────┘
```

Platform status:

| Platform | Control core | Service registration |
|---|---|---|
| Windows | NVAPI (RMW fan surface, live-verified on 472.12) | Windows SCM (`nvoc-srv-ctl install`) |
| Linux  | NVAPI via libnvidia-api **(fan write unverified — needs live testing)**; falls back to NVML | systemd unit: [`systemd/nvoc-srv.service`](./systemd/nvoc-srv.service) |

## Building

```sh
cargo build --package nvoc-srv --all-targets
cargo test  --package nvoc-srv            # pure logic, no GPU needed
```

Bins:

- `nvoc_service` — the service. Windows: SCM launch path by default, or
  `--foreground` in a console. Linux: always foreground.
- `nvoc-srv-ctl` — Windows SCM management: `install` / `uninstall` / `status`
  / `failure-actions`.

## Configuration

Layered (lowest → highest): built-in defaults → TOML file → CLI flags →
HTTP mutations. The file is read once at startup; HTTP changes are live-only
and revert on restart. Default path: `%PROGRAMDATA%\nvoc\nvoc-srv.toml`
(Windows) or `/etc/nvoc/nvoc-srv.toml` (Linux).

```toml
port = 14514              # HTTP control plane (loopback only)
interval_ms = 1000        # control tick / PID dt (200–10000)
sensor = "core"           # core | hotspot | memory | board | max
gpus = "all"              # "all" or a list: [0] / "0,1"
mode = "pid"              # startup mode: auto | pid | manual
manual_percent = 50       # pinned duty for mode = "manual"
read_fail_reset = 3       # consecutive read failures → restore driver control
watchdog_timeout_s = 30   # heartbeat staleness → watchdog restore

[pid]
target_c = 75.0           # setpoint (30–110)
kp = 2.0                  # fan % per °C of error
ki = 0.05                 # fan % per °C·s of accumulated error
kd = 0.0                  # fan % per °C/s of temperature slope
base_percent = 40.0       # feed-forward duty (see tuning guide)
min_percent = 0.0
max_percent = 100.0
emergency_delta_c = 12.0  # target+12 °C forces 100% duty (2 °C exit hysteresis)
```

CLI overrides (each maps to a field): `--config <path> --foreground --port
--interval-ms --target-c --kp --ki --kd --base-percent`.

## HTTP control plane

Loopback-only, port 14514. **Every mutating endpoint requires POST plus the
non-simple header `X-Requested-With: XMLHttpRequest`** (browser CSRF guard,
unchanged from the legacy service).

| Endpoint | Description |
|---|---|
| `GET /status` | per-GPU temps (core/hotspot/memory/board), written & measured fan duty, PID terms (`error/p/i/d/output`), failsafe state |
| `GET /config` | effective runtime configuration |
| `POST /pid?target_c=&kp=&ki=&kd=&base_percent=&min_percent=&max_percent=&emergency_delta_c=&interval_ms=&sensor=` | partial PID update, validated atomically, live |
| `POST /mode?value=auto\|pid\|manual` | switch control mode (`auto` hands fans back to the driver) |
| `POST /fan?percent=0-100` | pin a duty (switches to manual) |
| `POST /restore` | alias of `/mode?value=auto` |
| `POST /oc_global?oc=<kHz>&gpu=<index>` | legacy one-shot P0 graphics clock delta |
| `POST /shutdown` | graceful stop (fans restored first) |

Examples:

```sh
curl -s http://127.0.0.1:14514/status | python -m json.tool
curl -s -X POST -H "X-Requested-With: XMLHttpRequest" \
     "http://127.0.0.1:14514/pid?target_c=72&kp=3"
curl -s -X POST -H "X-Requested-With: XMLHttpRequest" \
     "http://127.0.0.1:14514/mode?value=auto"
```

> **Breaking change vs. the old service**: the VFP voltage-lock soft-wall
> loop was removed, so `/set_temp_limit_soft_vfp` is gone; `GET /config`
> returns the new schema. PID `error` convention: positive = too hot.

## Failsafe behavior

| Trigger | Response |
|---|---|
| Graceful stop (SCM stop, Ctrl-C, SIGTERM, `/shutdown`) | `ResetNvapiFanControl` on every controlled GPU, then exit |
| ≥ `read_fail_reset` consecutive sensor-read failures | fan restored to driver control; auto-resumes PID on the first good read |
| temp ≥ `target_c + emergency_delta_c` | 100% duty forced; exits with 2 °C hysteresis, PID re-engages from reset |
| control-loop heartbeat stale > `watchdog_timeout_s` | independent watchdog thread restores driver fan control |
| process abort (release builds abort on panic) | configure `nvoc-srv-ctl failure-actions` so SCM restarts the service; worst case the pin survives until the next start/reboot — run `nvoc cli fan reset` manually |

Note the failure-actions step is strongly recommended after `install`:

```sh
./nvoc-srv-ctl.exe failure-actions   # restart after 5 s / 30 s / 60 s
```

## Installing

### Windows (SCM)

```bat
:: admin prompt
nvoc-srv-ctl.exe install
nvoc-srv-ctl.exe failure-actions
net start nvoc_service
```

Logs: `%PROGRAMDATA%\nvoc\logs\nvoc-srv.log` (100 MB × 2 rotation; stdout/stderr
are redirected there in service mode).

### Linux (systemd)

```sh
sudo cp target/release/nvoc_service /usr/local/bin/
sudo cp srv/systemd/nvoc-srv.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now nvoc-srv
```

The unit runs as root (NVAPI/NVML RM access). SIGTERM is graceful — the loop
restores driver fan control before exiting.

**Known unknown (deliberate):** whether the NDA fan-write surface answers
through libnvidia-api on Linux is unverified. If it rejects (-104/-3) the
backend falls back to the NVML manual pin automatically; check
`journalctl -u nvoc-srv` for the fallback warning and verify with
`nvidia-smi`/`GET /status` before trusting it.

## PID tuning guide

Tune on a loaded or semi-loaded GPU (a fixed synthetic load gives a fixed
plant). All parameters are hot-tunable via `POST /pid` — watch the response
and `/status` `pid` terms while tuning. Log (and `/status`) show every duty
write.

0. **Baseline.** Set `mode = "auto"`, run your typical load, note the
   driver's steady-state fan duty and core temperature (from `/status` while
   in `auto`, or GPU-Z). Compute the feed-forward:

   `base_percent ≈ duty the driver needs to hold a sensible temp at this load`

1. **P only.** `POST /pid?ki=0&kd=0&base_percent=<from step 0>`, then
   `mode=pid`. Start `kp = 1`. Raise `kp` (2 → 4 → 8 …) until the fan duty
   and temperature visibly oscillate around `target_c` in `/status`
   (sustained periodic wiggle), then back `kp` off to **~60 % of that value**.
   Rule of thumb from our calibration: GPU thermal plants tolerate
   `kp ≈ 2–4 %/°C` for a 1 s tick.

2. **I for offset elimination.** If the temperature settles above/below
   `target_c` (steady error ≠ 0), add small `ki` — start `ki = 0.02` and
   double at most until the offset vanishes within a minute. Too much `ki`
   shows up as slow overshoot/undershoot around the setpoint; anti-windup
   keeps it bounded but not invisible.

3. **D only if needed.** GPU sensors are noisy and thermal mass is large;
   `kd` is usually unnecessary. If overshoot after a load step matters,
   try `kd = 0.5–2` and watch for duty chatter (writes every tick = noise
   amplification — reduce it).

4. **Setpoint sanity.** `target_c` should sit ≥ 10 °C below your card's
   slowdown threshold so the emergency 100% state stays a last resort, not a
   routine visitor.

5. **Verify recovery.** Flip `mode=auto` → `pid` → `auto` and stop/start the
   service; confirm in each case that the driver regains fan control
   (`/status` shows `fan_written_percent: null` and the tach follows the
   stock curve).

Interpreting `/status`:

- `pid.error_c` — positive means hotter than target.
- `pid.i` growing while pinned at a rail → you are saturated; consider more
  `base_percent` (feed-forward) instead of more gain.
- Duty writes every single tick → oscillation or noise; back `kp` off.
