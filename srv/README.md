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
                          # (ThermChannel readings decode at 1/256 °C on
                          # Pascal+; the legacy integer sensor is the fallback)
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
idle_delta_c = 4.0        # ≤ target−4 °C forces min duty (fan stop); 0 = off
write_deadband_percent = 1.0  # skip writes within ±1 % of the duty on the wire (anti-chatter)
adaptive_base = true      # learn base_percent online from the integral (see below)
```

CLI overrides (each maps to a field): `--config <path> --foreground --port
--interval-ms --target-c --kp --ki --kd --base-percent`.

## HTTP control plane

Loopback-only, port 14514. **Every mutating endpoint requires POST plus the
non-simple header `X-Requested-With: XMLHttpRequest`** (browser CSRF guard,
unchanged from the legacy service).

| Endpoint | Description |
|---|---|
| `GET /status` | top-level `mode`/`interval_ms`/`target_c`; per-GPU temps (core/hotspot/memory/board), written & measured fan duty, last PID decomposition — term values `p/i/d` **plus the effective gains `kp/ki/kd`** and the (possibly learned) `base_percent`; null when the PID did not run this tick; zone/failsafe state |
| `GET /config` | effective runtime configuration |
| `POST /pid?target_c=&kp=&ki=&kd=&base_percent=&min_percent=&max_percent=&emergency_delta_c=&idle_delta_c=&write_deadband_percent=&adaptive_base=&interval_ms=&sensor=` | partial PID update, validated atomically, live |
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
> The idle-release parameters were replaced by `idle_delta_c` (config keys
> `release_below_c` / `engage_below_c` / `release_ticks` are gone — delete
> them from your TOML).

## Forced zones (overtemp / idle)

A PID alone idles badly: after a load drop the feed-forward (base + integral)
reflects the *old* load and keeps the fan spinning while the temperature
sinks below the setpoint. Symmetric hysteresis zones clamp both extremes
while the PID keeps stepping underneath (its anti-windup freezes the
integral at the rail, so zone exits are bumpless — no reset, no blast):

```
target − idle_delta_c      target      target + emergency_delta_c
   ↑ forced min duty      ↑ PID active    ↑ forced 100 % duty
   (0 % = fan stopped)                (2 °C exit hysteresis both sides)
```

- Why not hand the fan back to the driver? The driver runs its **own**
  temperature setpoint (typically ≈ 58 °C under load). Surrendering control
  would cap the temperature at the driver's setpoint instead of ours — a
  stress test targeting 75 °C would never get there. In Pid mode the
  controller never yields the fan (only mode changes, `/restore`, shutdown
  and the watchdog do).
- `idle_delta_c = 4` (default) forces `min_percent` (0 % = fan stopped at
  the default floor) once the temperature falls 4 °C below target. `0`
  disables the idle zone.
- With `adaptive_base = true` the learned feed-forward also **decays while
  the idle zone holds**, so after a load drop the exit output converges
  back to the idle need within a couple of zone visits instead of pulsing.
- Sensor failures still trip the read-failure failsafe in every state (the
  fan is software-pinned even at 0 %, so a dead sensor must not be trusted
  to it).

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

### The plant you are controlling

```
C·dT/dt = Q_load − k·u^0.8·(T − T_ambient)     thermal lump, τ_T ≈ 10–40 s
rpm follows duty with its own lag              actuator, τ_fan ≈ 0.3–2 s
```

Three consequences that drive every recommendation below:

1. **The static gain is not constant**: `g = ∂T/∂u ≈ 0.8·(T−T_ambient)/u`.
   Halving the duty roughly doubles the loop gain — the loop that is tame at
   load can self-oscillate in the low-duty region. (The idle-release band
   exists precisely to keep the PID out of the worst-gain region.)
2. **Cooling rate is capped** by `k·u^0.8·ΔT`. No controller beats it; a
   loop tuned "faster than the plant" only injects oscillation energy.
   Aim for τ_T-scale settling (15–40 s), not seconds.
3. **The loop has two lags plus the integrator.** Sampling adds phase too
   (≈ `ω_c·T_s/2`), which is why a faster `interval_ms` buys real margin —
   but only up to the sensor's own update rate.

### Sizing the gains from a two-point calibration

With a fixed synthetic load (the stressor is ideal):

1. `mode=manual`, pin 40 % → let temperature settle → read T₁; pin 60 % →
   settle → T₂. Static gain at the operating point:
   `g ≈ (T₁−T₂)/20` °C per %. τ_T = time to 63 % of that temperature
   change (watch the log / `/status` at 1 s resolution).
2. **kp = 1.5/g to start; keep kp·g ≤ 3** for comfortable phase margin
   (≤ 5 is marginal, and only OK on fast-fan cards with large τ_T).
3. **ki ≤ kp/(2·τ_T)** — the default (kp 2, ki 0.05) assumes τ_T = 20 s.
4. **base_percent = the steady duty that holds target_c** at this load.
   The feed-forward carries the static `u^0.8` map; the smaller the
   integral's job, the quieter the loop.

### Self-oscillation fingerprints (diagnose via /status)

| Fingerprint | Period | Root cause | Prescription |
|---|---|---|---|
| Fast duty chatter | 2–6 ticks, ±1–2 % | quantized writes acting as a relay; tick faster than sensor refresh | raise `write_deadband_percent` to 2; slow `interval_ms` to the sensor rate |
| Slow limit cycle | ≈ 4–6·τ_T sawtooth (±2–5 °C) | kp·g too high, or base_percent far from the steady need so the integral carries the load | halve kp; set base_percent to the average duty /status shows near target |
| Ringing after load steps | 2–3 overshoots, then settles | marginal margin (kp·g ≈ 5) | cut kp ~30 %; leave kd = 0 until/if derivative noise filtering exists |
| Idle-zone cycling at idle | fan 0 % ↔ mid every few s–min, decaying | the forced-idle zone unwinding a stale feed-forward | benign and decaying; raise `ki` slightly (faster base re-learning) or lower `idle_delta_c` if the travel bothers you |

One extra hazard to rule out: if the legacy thermal sensor itself lags
seconds behind reality (cross-check against GPU-Z), that dead time eats
phase margin faster than any gain. Fix the sensor choice (`sensor=`) before
touching gains.

### Adaptive feed-forward (learning base_percent)

The load level is not constant, so a hand-set `base_percent` is always a
compromise. With `adaptive_base = true` (the default) the controller keeps
`base_percent` itself: whenever the loop is settled (|error| ≤ 1 °C), it
continuously re-centers integral authority into the base at a bounded rate.
The transfer is **output-continuous** — the base rises by exactly what the
integral falls — so learning adds no loop dynamics; it only re-partitions
state. Effects:

- `base_percent` tracks the current load level automatically (watch
  `pid.base_percent` in `/status`); the learned value survives idle-release
  periods and is only re-seeded from the config when `adaptive_base` turns
  off or the service restarts.
- The integral stays small, so anti-windup and the 4× discharge have little
  to fight — the loop runs quieter.
- Requirements: `ki > 0` (the integral is the teacher). With `ki = 0`,
  learning is inert.
- What it cannot do: anticipate a load *step*. The base only learns after
  the temperature shows the new load; a large step still transiently
  overshoots toward the emergency line (bounded there). Feed-forward from a
  measured load signal (NVML power draw, visible before temperature moves)
  is the future completion of this.

Set `adaptive_base = false` to pin `base_percent` to the configured value
(config-owned, applies every tick).

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

4. **Tick period.** The loop runs every `interval_ms` (default 1000; the PID
   dt follows it). 500 ms makes the loop snappier at the cost of 2× NVAPI
   traffic — hot-tunable via `POST /pid?interval_ms=500`. If the temperature
   reading itself only updates every few seconds on your card, a faster tick
   buys nothing; check `/status` temp granularity first.

5. **Idle zone.** After load drops the forced-idle zone (section above)
   clamps the fan to `min_percent` and decays the learned feed-forward, so
   the temperature is allowed to sag below target while the fan spins down.
   Size `idle_delta_c` to how far below target you accept that sag.

6. **Setpoint sanity.** `target_c` should sit ≥ 10 °C below your card's
   slowdown threshold so the emergency 100% state stays a last resort, not a
   routine visitor.

7. **Verify recovery.** Flip `mode=auto` → `pid` → `auto` and stop/start the
   service; confirm in each case that the driver regains fan control
   (`/status` shows `fan_written_percent: null` and the tach follows the
   stock curve).

Interpreting `/status`:

- `pid.error_c` — positive means hotter than target.
- `pid.i` growing while pinned at a rail → you are saturated; consider more
  `base_percent` (feed-forward) instead of more gain.
- Duty writes every single tick → oscillation or noise; back `kp` off.
