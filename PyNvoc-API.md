# PyNvoc API

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

`pynvoc` is the native Python binding for NVOC. It is a [PyO3](https://pyo3.rs) extension module (`pynvoc._native`) compiled from `nvoc-python/src/lib.rs` against `nvoc-core`, exposed to Python through a thin wrapper package (`nvoc-python/python/pynvoc/__init__.py`). It mirrors most `nvoc-cli` operations as direct Python calls and is the backend used by both Python frontends — the GUI ([[GUI-Guide]]) and the TUI ([[TUI-Guide]]) — via `importlib.import_module("pynvoc")`. See [[Architecture]] and [[Components]] for where it sits in the stack.

### Installation / Import

There is no PyPI package; build the extension from the repo. `maturin` is installed automatically by `uv sync` (dev dependency group):

```bash
cd nvoc-python
uv sync
uv run maturin develop          # debug build, editable install (add --release for release)
```

Then import anywhere in the workspace environment:

```python
import pynvoc                    # wrapper package, re-exports all 105 functions
import pynvoc._native            # the raw PyO3 module (rarely needed directly)
```

- Native module name: `pynvoc._native` (`.pyd` on Windows, `.so` on Linux); abi3 wheels for Python >= 3.8 (PyO3 0.29, `abi3-py38`).
- If you change Rust code, re-run `uv run maturin develop` or the change will not be visible in Python.
- If PyInstaller downstream packaging reports "Invalid NT Headers signature", the `.pyd` was mangled — re-run `maturin develop` to regenerate it.

### Conventions

**GPU IDs.** Every function takes the GPU as a string. All of these resolve to the same GPU: `"0x0700"` (the `gpu_id_hex` key), `"0700"`, `"1792"`, `"gpu 0"`, `"GPU0"`. `discover_gpus` returns the canonical `gpu_id_hex`.

**Backend sets** (read/discover functions): `"both"` or `"all"`, `"nvapi"`, `"nvml"`. An invalid string raises `ValueError` before any hardware access.

**Backend** (write functions and backend-switchable calls): `"nvapi"`, `"nvml"`, plus fan aliases `"nvapi-cooler"`, `"nvml-cooler"`.

**Clock domains:** `"core"` / `"gpu"` / `"graphics"` and `"mem"` / `"memory"`. **P-States:** `"P0"`…`"P15"` (case-insensitive; plain numbers work for the native lock).

**Optional arguments.** Functions marked below with `name=default` have real Python defaults. All other `Option<...>` parameters are still positional and **must be passed** — pass `None` explicitly (e.g. `pynvoc.set_fan(gpu, "nvapi", "all", "auto", None)`).

**Return shapes.** Reads return plain dicts/lists (ints, floats, bools, strings, nested objects). Keys the driver did not report are omitted rather than fabricated. Private NVAPI families return `{"supported": false}` instead of raising when the driver does not expose them; `query_tgp_watt_range`, `query_dnotifier` and `query_power_ceiling` return `None` in that case.

**Error model.** Invalid argument strings (backend set, backend, domain, P-State, display ID, EDID hex, API name, out-of-range D-Notifier level, short delta lists) raise `ValueError` *before* touching hardware. Operational failures (no GPU selected, NVAPI/NVML call failed) raise `RuntimeError`; the core error text embeds the NVAPI status name and the thread-local NVAPI `last_status_error` passthrough, so a stamp-gated `-9 INCOMPATIBLE_STRUCT_VERSION` reads differently from a true "not present".

**Threading.** The GPU inventory is cached per backend set at process level (first use discovers; `discover_gpus` forces a refresh; a lookup miss — e.g. a dGPU that went GCOFF — triggers one automatic re-discovery and retry). Heavy calls release the GIL (`py.detach`), so GUI/TUI worker threads can poll at 1 Hz without stalling the UI. Note the 1 Hz `query_status` poll is deliberately NVAPI-only for power/clock telemetry: NVML device-level queries segfault inside `nvml.dll` on a stale handle during dGPU D3cold<->D0 transitions, so live power comes from the NVAPI PowerMonitor "Board" rail and the enforced power limit is served from a process-level cache filled by `query_info`.

**Mobile dGPU.** Call `pynvoc.force_wake(gpu)` (returns `True` if a GC6 wake was performed, `False` if the GPU was already awake / not applicable) before capability/limit reads on a laptop dGPU that may be powered off. The wake is not persistent.

### Read API

| Function | Backend | Notes |
| --- | --- | --- |
| `discover_gpus(backends)` | both | Forced re-discovery. Items: `index`, `gpu_id`, `gpu_id_hex`, `backend_nvapi`, `backend_nvml`, `name`, `codename`, `arch`, `is_mobile`, `is_server`, `is_legacy_voltage`, `xbar_supported` |
| `force_wake(gpu)` -> `bool` | NVAPI | GC6 exit for mobile dGPUs |
| `query_info(gpu, backends="both")` | both | Identity (`name`, `codename`, `arch`, `gpu_series`, `bios_version`, `bus`, `uuid`), clock ranges (`core_clock_min/max`, `mem_clock_min/max`, MHz), power limits (W), thermal limits (C), legacy overvolt ranges, and capability flags: `is_mobile`, `is_server`, `is_legacy_voltage`, `xbar_supported`, `is_ampere_plus` |
| `query_status(gpu, backends="both")` | both | 1 Hz dashboard payload: `pstate`, `voltage_mv`, `gpu_clock_mhz`, `mem_clock_mhz`, `video_clock_mhz`, `eff_*_clock_mhz`, `all_clocks_mhz` (all 32 domains, dict), `temperature_c` + `temp_core` / `temp_hotspot` / `temp_memory`, `target_temp_c` / `max_temp_c`, `power_w`, `power_limit_w`, `power_rails_w`, `perf.limits_decoded` (throttle reasons), `vfp_locked` / `vfp_lock_mv`, `utilization`, `vram`, `coolers`, `pcie_lanes` |
| `query_settings(gpu, backends="both")` | both | Applied settings: `voltage_boost_current`, `power_limit_current`, `thermal_limit_current`, `core/mem_clock_current`, `supported_pstates`, `pstate_ranges`, `fan_count/min/max`, `temperature_thresholds`, `vfp_locks`, legacy overvolt |
| `query_supported_applications_clocks(gpu, backends="nvml")` | NVML | App-clock roster |
| `query_power_limits(gpu)` | NVML | `{min_watt, current_watt, max_watt}` |
| `query_pstates(gpu)` | NVML | P-State clock ranges |
| `query_fan_info(gpu)` | NVML | `{count, min_percent, max_percent, current_percent}` |
| `query_temperature_thresholds(gpu)` | NVML | `[{name, celsius}]` |
| `query_throttle_reasons(gpu)` | NVML | `[{name, active}]` |
| `query_auto_boost(gpu)` | NVML | `{enabled, default_enabled}` |
| `query_api_restriction(gpu, api_type)` | NVML | `api_type`: `"app-clocks"` / `"application-clocks"` / `"auto-boost"` / `"autoboost"` |
| `query_legacy_gpc_rail_volt_range(gpu, pstate=None)` | NVAPI | Legacy core overvolt ranges (uV) |
| `query_pstate_base_voltage(gpu, pstate=None)` | NVAPI | Base voltage + editable delta window (P0 default) |
| `query_voltage_boost(gpu)` | NVAPI | `{voltage_boost_percent}` |
| `get_display_list(gpu, all=False)` | NVAPI | Display descriptors (`display_id`, connector, flags, active/connected bools) |
| `query_edid(gpu, display_id)` | NVAPI | `{display_id, bytes, edid_hex}` |
| `query_clock_offset(gpu, backends, domain, pstate)` | NVML or NVAPI | `{mhz}`. NVML: integer MHz; NVAPI: live P-State delta with sub-MHz resolution. Pass `None` for `pstate` (= P0) |
| `query_public_vftable(gpu, domain=None, infer_missing_default=True)` | NVAPI | Public V-F curve points: `[{index, voltage_uv, frequency_khz, delta_khz, default_frequency_khz, point_type}]`; `point_type` = `prog` / `fixed` / `dyn` |
| `query_vbios_vf_curve(gpu)` | NVAPI + core parser | vBIOS boost ladder (see below) |
| `query_vfp_point_voltage(gpu, point)` | NVAPI | `{microvolts}` |
| `query_tdp_temp_limits(gpu)` | NVAPI | `{min_tdp, default_tdp, max_tdp, min_temp, default_temp, max_temp}` (missing entries omitted) |
| `probe_voltage_limits(gpu)` | NVAPI | `{lower_point, upper_point}` |
| `check_voltage_frequency(gpu, point)` | NVAPI | `{precise, matched_point, microvolts}` |
| `query_tgp_watt_range(gpu)` | NVAPI | TGP policy `{policy_index, min_watt, default_watt, max_watt}` or `None` |
| `query_dnotifier(gpu)` | NVAPI | `{active: "D3", levels: [{level, watts}]}` or `None` |
| `query_power_ceiling(gpu)` | NVAPI | Effective PPAB wall (see below) |
| `query_target_temp_policies(gpu)` | NVAPI | Private ClientThermalTarget table (GET-prime 0xC4554575) |

Example:

```python
import pynvoc

# 1. Enumerate GPUs (refreshes the process-level inventory cache)
gpus = pynvoc.discover_gpus("both")
gpu = gpus[0]["gpu_id_hex"]                # e.g. "0x0700"

# 2. Read before writing
info = pynvoc.query_info(gpu, "both")      # identity, ranges, capability flags
status = pynvoc.query_status(gpu, "both")  # live dashboard payload
settings = pynvoc.query_settings(gpu, "both")
print(info["name"], info["gpu_series"], info["is_ampere_plus"])
print(status["temperature_c"], status["power_w"], status["gpu_clock_mhz"])
print("core range MHz:", info["core_clock_min"], "-", info["core_clock_max"])
```

### Write API

All writes are immediate and process-wide; nothing is persisted across reboot unless the driver says otherwise. NVAPI writes require an elevated process; private admin-gated families are flagged below.

**Clocks (public paths)**

- `set_clock_offset(gpu, backend, domain, value_mhz, pstate)` — one decimal MHz allowed (divides the 2.5 MHz GUI grid); NVAPI takes kHz (rounded), NVML integer MHz. Pass `None` for `pstate` (= P0).
- `reset_core_clocks(gpu, backend)` / `reset_mem_clocks(gpu, backend)` — NVAPI path clears the V-F frequency lock *and* the P0 global offset.
- `set_pstate_clock_offset(gpu, pstate, domain, delta_khz)` / `reset_pstate_clock_offsets(gpu, [("P0", "core"), ...])` — NVAPI per-P-State deltas (kHz).
- `set_locked_clocks(gpu, backend, domain, min_mhz, max_mhz)` / `reset_locked_clocks(gpu, backend, domain)` — NVML path only.
- `set_applications_clocks(gpu, memory_mhz, graphics_mhz)` / `reset_applications_clocks(gpu)` — NVML app clocks.
- `set_legacy_clocks(gpu, core_mhz, memory_mhz)` — NVAPI absolute legacy clocks.
- Public V-F curve edits (NVAPI, kHz deltas): `set_vfp_point_delta(gpu, point, delta)`, `set_vfp_range_delta(gpu, start, end, delta)`, `set_domain_vfp_deltas(gpu, domain, [(point, delta), ...])`, `reset_vfp_deltas(gpu, domain)` (`"all"`/`"core"`/`"memory"`).
- `set_vfp_frequency_lock(gpu, domain, upper_khz, lower_khz=None)` / `reset_vfp_frequency_lock(gpu, domain)`.
- `set_vfp_voltage_lock(gpu, point, voltage_uv, feedback=False)` — GPC voltage lock; pass **either** `point` or `voltage_uv` (the other as `None`). `reset_vfp_lock(gpu)` clears it.
- `sync_memory_pstate_as_p0(gpu)` — NVAPI helper for memory P-State coupling.

**Voltage**

- `set_voltage_boost(gpu, percent)` / `reset_voltage_boost(gpu)`.
- `set_legacy_voltage_delta(gpu, uv, pstate=None)` / `set_pstate_base_voltage(gpu, pstate, delta_uv)` / `reset_pstate_base_voltages(gpu)`.
- `set_volt_rail_offset(gpu, rail_bit, offset_uv, expect_type=None)` and `set_volt_rail_target(gpu, rail_bit, target_mv, expect_type=None)` — private multi-rail walls (50-series core+Xbar, GB10). The target form derives the offset internally and returns `{base_wall_uV, offset_uV, applied_uV, effective_wall_uV, ...}`; the driver clamps to `min(target, vbios_wall, vrm_max_wall)`. Enumerate rails first with `query_volt_rails(gpu)`.
- `set_core_voltage_control(gpu, value)` — private core-voltage control object (SET 0xDC2BD4A6, **admin**); read with `get_core_voltage_control(gpu)` (GET 0xA91F88EB).

**Power / thermal**

- `set_power_limit(gpu, backend, value)` — units differ: NVML takes **watts**, NVAPI takes **percent**.
- `set_nvapi_power_limits(gpu, [percent, ...])` / `reset_nvapi_power_limits(gpu)`; `set_nvapi_sensor_limits(gpu, [celsius, ...])` / `reset_nvapi_sensor_limits(gpu)`; `set_thermal_limit(gpu, celsius)` (NVAPI preferred, NVML fallback).
- `set_target_temp(gpu, celsius, policy_index=None)`.
- `set_perf_freq_cap(gpu, max_mhz, min_mhz)` — perf-cap clamp (NVAPI PerfLimitsSetStatus 0x32CA4983, the ref tool `-gpuclk` setter); pass `-1, -1` to reset, `0` leaves a side unset.
- `set_tgp_watt(gpu, watts, policy_index=None)` / `reset_tgp_watt(gpu, policy_index=None)` (defaults to policy 2).
- `set_ppab_status(gpu, active)` and `set_dnotifier(gpu, level)` — see PPAB section below.
- `set_temp_sim(gpu, temperature_c)` / `reset_temp_sim(gpu)` — **dangerous research tool**: fakes the driver-visible temperature. Secured-Overrides gated; read with `get_temp_sim(gpu)`.

**Fan**

- `set_fan(gpu, backend, fan_id, policy, level)` — `fan_id`: `"all"`, `"1"`, `"2"` or an index; `level` default 60. `policy="auto"` is the **true reset** (see fan section below); NVML accepts `"continuous"` / `"manual"`; NVAPI accepts those plus the cooler-policy names (`"default"`, `"perf"`, `"discrete"`, `"continuous"`, `"hybrid"`, `"software"`, `"default32"`).
- `reset_fan_speed(gpu, fan_index)` — NVML `nvmlDeviceSetDefaultFanSpeed_v2` (the documented "restore default control policy" call), wrapped in TDR recovery.
- `set_cooler_levels(gpu, policy, level, target_name=None)` / `reset_cooler_levels(gpu)` — NVAPI cooler policy table.
- `set_fanstop_status(gpu, enable, curve_index=None)` — zero-RPM slot toggle (NVAPI FanArbiterSet NDA 0x44CD3014, struct magic 0x10144).
- `set_fan_rpm(gpu, rpm, cooler_index=None)` — RPM simulation via private FanCoolerSetControl (NDA 0xEB44E8AA); `rpm=-1` disables (return to auto).

**P-State locks**

- `set_nvapi_pstate_lock(gpu, first_pstate, second_pstate=None)` / `set_nvml_pstate_lock(...)` — lock a P-State or contiguous range via a derived memory-clock window. Returns `None` on a clean apply, or a warning string when identical memory clocks make the range inseparable (the lock is applied anyway; surfaces the caveat to the UI).
- `set_pstate_native_lock(gpu, pstate)` / `reset_pstate_native_lock(gpu)` — pins a single P-State directly (NVAPI PerfClientLimitsSetStatus NDA 0x39442CFB, mode-1). The fallback for pre-Kepler GPUs where the NVML-range derivation cannot run.
- `set_pstate_lock_level(gpu, level)` — admin-free perf-level lock (SetPerfLevel 0x75DD3E6A, escape 0x7000040). `level` is an **index into the GPU's real available P-State list**, not a fixed enum; no release value exists — reboot/driver reload clears it.

**Other**

- `set_auto_boost(gpu, enabled)` / `set_auto_boost_default(gpu, enabled)`; `set_api_restriction(gpu, api_type, restricted)` — NVML.
- `set_edid(gpu, display_id, edid_hex)` / `clear_edid(gpu, display_id)` — EDID override.
- `set_background_oc_scanner(gpu, enable)` (0x06DC7CE8); `get_oc_scanner_incomplete(gpu)` (0xBE371D0A).
- `get_pmgr_arbiter(gpu)` (GET 0x717648FD) / `set_pmgr_arbiter(gpu, values)` (SET 0x9C4BB8D0, **admin**) — PMGR voltage-request arbiter; `values` must be exactly 11 dwords; GET-patch-SET recommended.
- `get_rated_tdp(gpu)` — rated-TDP readback trio (0xED2BEA09 / 0x87BD35EF / 0xFCBDF642): `{control_mode, info_capabilities, status_raw}`.
- `reset_all(gpu, domain=None)` — the panic button: zero voltage boost, reset sensor/power limits, cooler levels, public V-F offsets + GPC lock, legacy overvolt, P0 clock offsets (NVAPI side) and NVML freq locks. `domain`: `"all"`/`"core"`/`"memory"`.

### Private / experimental surfaces

These wrap reverse-engineered private NVAPI families. They return `{"supported": false}` when the driver does not expose the family, and several are gated by driver version, generation, or elevation. Treat them as experimental; the CLI exposes the same surfaces (see [[CLI-Command-Reference]]).

#### query_cooler_info — private NVAPI cooler family

`query_cooler_info(gpu) -> {"count": int, "coolers": [{"index", "type", "min", "max", "current", "current_pwm_percent"}]}`

Backed by the private `FanCoolerGetInfo` (0x65CE5BFC), NVAPI-only. The frontends pair it with NVML fan info for the legacy verdict: **NVML fans >= 1 with an empty private NVAPI cooler family is the <= Kepler-driver signature**; modern cards report their coolers through this family too.

```python
coolers = pynvoc.query_cooler_info(gpu)
print(coolers["count"], coolers["coolers"])
```

#### NVAPI fan reset — control-block override clear first

`set_fan(gpu, "nvapi", fan_id, "auto", None)` performs the fan *reset* in two tiers:

1. **Modern cards:** `ResetNvapiFanControl` clears the cooler level-override bit (bit0) in the control block — live A/B on 1650S + A4000 showed this is the **only** reset that actually unpins the level (the 0x214AC reset bitmask is accepted but leaves the pin; `RestoreCoolerSettings` / `RestoreCoolerPolicyTable` return NOT_SUPPORTED).
2. **Legacy drivers** (e.g. R391/Fermi) reject the NDA ClientFanCoolers family outright, so the binding falls back to the public `RestoreCoolerSettings` (`reset_cooler_levels`). A combined error is surfaced only when **both** paths fail — NDA-first is what makes the reset work on no-cooler-table desktop cards.

`"auto"` is a true reset, deliberately not the SW temperature-continuous policy: on GPUs whose ClientFanPolicies curve table is empty (default fan control is firmware-side), switching to the curve-following mode parks the fan near 0 RPM until the temperature spikes.

```python
pynvoc.set_fan(gpu, "nvapi", "all", "manual", 60)   # fixed 60% level
pynvoc.set_fan(gpu, "nvapi", "all", "auto", None)   # true reset (clears the override)
```

#### NVML fan TDR recovery — global shutdown + re-discovery + retry

`reset_fan_speed` and the NVML branch of `set_fan` are wrapped in a TDR recovery helper. After an OC instability triggers a driver TDR, the process-cached NVML instance is dead — `SetFanControlPolicy` reports NotFound — and a plain re-init returns AlreadyInitialized, so a normal refresh cannot rebuild it. On the first failure the wrapper: (1) force-runs a global `nvmlShutdown`, (2) forces a full re-discovery (fresh `Nvml::init` re-enumeration), (3) retries the operation once. If it still fails, the recovered error is raised.

#### query_vbios_vf_curve — vBIOS boost-ladder payload

`query_vbios_vf_curve(gpu) -> dict`

Reads the full VBIOS image (`QueryVbiosImage`) and decodes it with the core legacy BIT-family vBIOS parser: the GPU Boost 2.0 boost-ladder table (version 0x10, BIT `'P'`+0x34) — 79 five-byte points (u16 half-MHz frequency + vmap voltage index) plus P-State boundary marks. This is the Maxwell/Kepler "hidden" V-F curve; the GUI/TUI render it as a read-only BIOS VF ladder. The image read is a multi-escape loop (2 KB chunks) and releases the GIL.

Payload: `available`, `table_version`, `curve_start_index`, `curve_end_index`, `points[{index, freq_mhz, v_min_mv, v_max_mv, vmap_index}]`, `pstate_marks[{pstate, code_raw, index}]`, `warnings`. When the image has no boost ladder (non-Maxwell/Kepler generation) it returns `{"available": false}` instead of raising, so callers can hide the feature silently.

```python
curve = pynvoc.query_vbios_vf_curve(gpu)
if curve["available"]:
    start, end = curve["curve_start_index"], curve["curve_end_index"]
    for p in curve["points"][start:end + 1]:
        print(p["index"], p["freq_mhz"], p["v_min_mv"], "-", p["v_max_mv"], "mV")
```

#### set_ppab_status and the effective power ceiling

- `set_ppab_status(gpu, active: bool)` — toggles PPAB (mobile dynamic-boost wall) via `SetNvapiDynamicBoost`. This is a **renamed** binding (the GUI/TUI toggles were migrated to `set_ppab_status`); there is **no readback**: a `get_dynamic_boost` wrap was withdrawn because GET 0xC80068A1 reads the PCF platform status bytes, not the PPAB enable written here.
- `query_power_ceiling(gpu)` — the effective PPAB wall (nvidia-smi's "Ceiling" trio): `{policy_index, default_watt, requested_watt, dnotify_watt, ceiling_watt}` where `ceiling_watt = min(requested TGP, active D-Notifier cap)`. This is the "you set 100 W — here is what actually applies" value the GUI/TUI power slider anchors to. Returns `None` where the private family is unavailable.

```python
pynvoc.set_ppab_status(gpu, True)       # enable PPAB (mobile dynamic-boost wall)
pynvoc.set_tgp_watt(gpu, 140, None)     # request 140 W
ceiling = pynvoc.query_power_ceiling(gpu)
print(ceiling["requested_watt"], "->", ceiling["ceiling_watt"], "W")
```

#### Generation verdicts and the private V-F raw-record transport

`query_info` / `discover_gpus` carry core's generation verdict (`detect_gpu_type` from name + codename): `gpu_series`, `is_mobile`, `is_server`, `is_legacy_voltage`, `xbar_supported`, and `is_ampere_plus` (query_info). These flags gate frontend behaviour — capability must come from them, not the raw arch string (Ada reads `Unknown` there).

- **`is_ampere_plus`** marks the ClkDomains bit1 coupling boundary: on 30-series and Ada the bit1 domain couples SYS, and a bit3 write needs `-f` to cancel.
- **Blackwell** (`GpuType::is_blackwell()`, 50-series consumer/workstation/server) gates the ClkDomains plane-slot shift (slot2 = freq / slot3 = volt, live user probe). `query_private_vftable` serializes `volt_offset_uV` per point — signed, **Blackwell records only**, 0 elsewhere.
- **Raw-record transport:** the private V/F-points family (GetInfo 0x8895B510 -> GetStatus 0x7FEE9032) can ride raw GetStatus records through (`include_raw`), and the SetControl side (ID 0xFEC00D04) writes per-point records in one read-modify-write cycle with snapshot + readback + restore-on-mismatch. These are the surfaces behind the GUI/TUI V-F curve editor's "private/raw" path when the traditional public VFP interface is unsupported:

  - `query_private_vftable(gpu)` — bank masks, segments (`vf_curve` / `pstate_bins` with an empirical `domain` hint) and points (`voltage_uV`, `freq_default_mhz`, `freq_current_mhz`, `volt_current_uV`, `volt_offset_uV`, EXT-section `domain_freq_mhz` / `domain_volt_uV`). Degenerate-voltage segments (driver leaves voltage at 0, e.g. GP100/TCC) are patched from the public GPC voltage grid by index so plots keep a real voltage axis.
  - `set_vfp_point_private(gpu, bank, idx, value_khz, freq_mode)` — write one point. `freq_mode=True` = mode 0 (u32 kHz field; **Pascal** GPUs get the 2x axis scaling applied automatically); `freq_mode=False` = mode 1 (raw i16 delta, 1:1 on Pascal).
  - `set_vfp_range_per_point_private(gpu, bank, start, end, deltas)` — mode-1 raw i16 per-point deltas in a single RMW cycle.
  - `clk_vf_delta_for_target_mhz(def_mhz, target_mhz, class)` — pure computation (no GPU): translate a MHz target into the mode-1 raw delta via the universal g(def) prior; `class` = `"graphics"`/`"gpc"` or `"fabric"`/`"xbar"`/`"msd"`. Returns `{"delta": int}` or `{"delta": None}` when the prior has no coefficient for that default band.
  - `reset_vfp_private(gpu, bank, only_mode=None)` — the **only** way to clear private raw/converted offsets (the public `reset_vfp_deltas` cannot reach private state).

```python
# raw-converted private point write (public VFP unsupported):
delta = pynvoc.clk_vf_delta_for_target_mhz(1800, 1900.0, "graphics")["delta"]
if delta is not None:
    pynvoc.set_vfp_point_private(gpu, 0, 42, delta, False)   # mode-1 raw delta
```

Related private clock surfaces: `query_private_freq_domain_info(gpu)` (controllable-domain block, RM GET_CONTROL 0x2080901b), `query_clk_domain_freq(gpu, domain_bit)` (two-sample MEASURE_FREQ, RM 0x20809006; domain bits GPC=0, XBAR=1, SYS=2, MCLK=4), `query_private_freq_domain_status(gpu, domain_bit)` (green-curve direct measure 0x527FC458, no 50 ms sleep, HBM already divided), and `set_clk_domain_offset(gpu, domain_bit, offset_khz, slot=None, temporary=None)` (SET_CONTROL 0x2080d01c; slot 0 = the signed frequency offset, slot mapping is generation-dependent; `temporary=True` restores the snapshot before returning; **no magnitude limit is enforced — you own the range policy**). All of these are marked **DANGEROUS GPU clock writes** in the source.

### Generation / driver / privilege caveats

- **Modern driver required for NDA families.** ClientFanCoolers, PerfClientLimits, ClientThermalTarget, ClockClient and the private V/F-points family are absent from old user-mode DLLs (verified on R391/Fermi: the family returns NVAPI_ERROR -1). The binding degrades gracefully: public fallbacks for fan reset, `{"supported": false}` for reads.
- **Legacy GPUs (<= Kepler).** `is_legacy_voltage` marks the legacy voltage path (GT730-class). On legacy drivers NVML init itself can fail; NVML-dependent reads fall back to NVAPI-derived data where implemented (e.g. P-State roster). Use `set_pstate_native_lock` instead of `set_nvapi_pstate_lock` there.
- **Maxwell/Kepler** have no runtime V-F curve — only the read-only vBIOS boost ladder (`query_vbios_vf_curve`).
- **Pascal:** private mode-0 kHz writes carry a 2x axis encoding; the binding applies it automatically (reported as `pascal_2x_write` in the result). Mode-1 raw deltas are 1:1.
- **Ampere+ (30-series, Ada):** `is_ampere_plus` — ClkDomains bit1 SYS coupling; a compensating bit3 write is required to cancel.
- **Blackwell (50-series):** ClkDomains plane-slot shift (slot2=freq/slot3=volt); `volt_offset_uV` only present in Blackwell VFP records.
- **Mobile dGPU:** everything in the 1 Hz `query_status` poll is segfault-safe by design (NVAPI-only telemetry + cached NVML power limit); `force_wake` before capability reads after GCOFF.
- **Privilege.** NVAPI writes require an elevated (administrator) process. The source explicitly marks `set_core_voltage_control`, `set_pmgr_arbiter` (admin) and the thermal-sim family (Secured-Overrides gated) as elevated surfaces; `set_pstate_lock_level` is the notable *admin-free* lock. Recovery behaviour for every family is documented in [[Safety-and-Recovery]]; hardware support per generation in [[GPU-Support-Matrix]].

### Safety

1. **Read before write.** Anchor every write to `query_info` ranges / `query_settings` current values; re-read after writing to confirm the driver accepted (and clamped) the value.
2. **Prefer the documented reset paths.** `reset_all(gpu)` restores the full NVAPI+NVML default set; per-family resets (`reset_core_clocks`, `reset_voltage_boost`, `reset_cooler_levels`, `reset_vfp_private`, ...) are narrower. Private offsets *must* be cleared with `reset_vfp_private`.
3. **Keep recovery visible.** Pair every write path with [[Safety-and-Recovery]]; the CLI equivalents of every binding are in [[CLI-Guide]] and [[CLI-Command-Reference]], which is also the reference for unit and range semantics.

---

<a id="chinese"></a>

## 中文

`pynvoc` 是 NVOC 的原生 Python 绑定。它是一个基于 [PyO3](https://pyo3.rs) 的扩展模块（`pynvoc._native`），由 `nvoc-python/src/lib.rs` 针对 `nvoc-core` 编译而成，并通过薄封装包（`nvoc-python/python/pynvoc/__init__.py`）暴露给 Python。它把大多数 `nvoc-cli` 操作镜像为直接的 Python 调用，是两个 Python 前端 —— GUI（[[GUI-Guide]]）和 TUI（[[TUI-Guide]]）—— 共用的后端，通过 `importlib.import_module("pynvoc")` 加载。它在技术栈中的位置参见 [[Architecture]] 与 [[Components]]。

### 安装 / 导入

没有 PyPI 包，需要从仓库构建扩展模块。`maturin` 会由 `uv sync` 自动安装（dev 依赖组）：

```bash
cd nvoc-python
uv sync
uv run maturin develop          # debug 构建 + 可编辑安装（发布版加 --release）
```

然后即可在工作区环境中的任意位置导入：

```python
import pynvoc                    # 封装包，转发全部 105 个函数
import pynvoc._native            # 原始 PyO3 模块（一般无需直接使用）
```

- 原生模块名：`pynvoc._native`（Windows 为 `.pyd`，Linux 为 `.so`）；abi3 wheel，支持 Python >= 3.8（PyO3 0.29，`abi3-py38`）。
- 修改 Rust 代码后必须重新运行 `uv run maturin develop`，否则更改不会反映到 Python。
- 如果下游用 PyInstaller 打包时报 "Invalid NT Headers signature"，说明 `.pyd` 二进制被损坏 —— 重新运行 `maturin develop` 重建即可。

### 约定

**GPU ID。** 所有函数都用字符串指定 GPU。以下写法指向同一块 GPU：`"0x0700"`（即 `gpu_id_hex`）、`"0700"`、`"1792"`、`"gpu 0"`、`"GPU0"`。`discover_gpus` 返回规范的 `gpu_id_hex`。

**后端集合**（读取/发现类函数）：`"both"` 或 `"all"`、`"nvapi"`、`"nvml"`。非法字符串会在触碰硬件之前抛出 `ValueError`。

**后端**（写入函数及可切换后端的调用）：`"nvapi"`、`"nvml"`，另有风扇别名 `"nvapi-cooler"`、`"nvml-cooler"`。

**时钟域：** `"core"` / `"gpu"` / `"graphics"` 与 `"mem"` / `"memory"`。**P-State：** `"P0"`…`"P15"`（大小写不敏感；native lock 还接受纯数字）。

**可选参数。** 下文标注 `name=default` 的函数具有真正的 Python 默认值。其余 `Option<...>` 参数仍是位置参数，**必须传入** —— 请显式传 `None`（例如 `pynvoc.set_fan(gpu, "nvapi", "all", "auto", None)`）。

**返回结构。** 读取类函数返回纯 dict/list（int、float、bool、字符串、嵌套对象）。驱动未上报的键会被省略而不是伪造。驱动未暴露对应私有家族时，私有 NVAPI 接口返回 `{"supported": false}` 而不是抛异常；`query_tgp_watt_range`、`query_dnotifier` 和 `query_power_ceiling` 在该情况下返回 `None`。

**错误模型。** 非法参数字符串（后端集合、后端、时钟域、P-State、display ID、EDID hex、API 名称、越界 D-Notifier 等级、delta 列表长度不足）会在触碰硬件**之前**抛出 `ValueError`。操作失败（未选中 GPU、NVAPI/NVML 调用失败）抛出 `RuntimeError`；core 的错误文本内嵌 NVAPI 状态名以及线程本地的 NVAPI `last_status_error` 透传，因此带 stamp 门控的 `-9 INCOMPATIBLE_STRUCT_VERSION` 与真正的"不存在"可读出差异。

**线程模型。** GPU inventory 按后端集合在进程级缓存（首次使用时发现；`discover_gpus` 强制刷新；查找未命中 —— 例如 dGPU 进入 GCOFF —— 会触发一次自动重新发现并重试）。重负载调用会释放 GIL（`py.detach`），GUI/TUI 工作线程可以 1 Hz 轮询而不阻塞 UI。注意 1 Hz 的 `query_status` 轮询在功耗/时钟遥测上是刻意 NVAPI-only 的：NVML 设备级查询在 dGPU D3cold<->D0 切换期间遇到失效句柄时会在 `nvml.dll` 内部段错误，因此实时功耗来自 NVAPI PowerMonitor 的 "Board" 轨，而生效的功耗上限由 `query_info` 填充的进程级缓存提供。

**移动端 dGPU。** 在可能已掉电的笔记本 dGPU 上读取能力/限制之前，先调用 `pynvoc.force_wake(gpu)`（执行了 GC6 唤醒返回 `True`；本就在线或不适用返回 `False`）。唤醒非持久。

### 读取 API

| 函数 | 后端 | 说明 |
| --- | --- | --- |
| `discover_gpus(backends)` | both | 强制重新发现。条目：`index`、`gpu_id`、`gpu_id_hex`、`backend_nvapi`、`backend_nvml`、`name`、`codename`、`arch`、`is_mobile`、`is_server`、`is_legacy_voltage`、`xbar_supported` |
| `force_wake(gpu)` -> `bool` | NVAPI | 移动端 dGPU 的 GC6 唤醒 |
| `query_info(gpu, backends="both")` | both | 身份信息（`name`、`codename`、`arch`、`gpu_series`、`bios_version`、`bus`、`uuid`）、时钟范围（`core_clock_min/max`、`mem_clock_min/max`，MHz）、功耗上限（W）、温度上限（C）、legacy 过压范围，以及能力标志：`is_mobile`、`is_server`、`is_legacy_voltage`、`xbar_supported`、`is_ampere_plus` |
| `query_status(gpu, backends="both")` | both | 1 Hz 仪表盘负载：`pstate`、`voltage_mv`、`gpu_clock_mhz`、`mem_clock_mhz`、`video_clock_mhz`、`eff_*_clock_mhz`、`all_clocks_mhz`（全部 32 个域，dict）、`temperature_c` 与 `temp_core` / `temp_hotspot` / `temp_memory`、`target_temp_c` / `max_temp_c`、`power_w`、`power_limit_w`、`power_rails_w`、`perf.limits_decoded`（降频原因）、`vfp_locked` / `vfp_lock_mv`、`utilization`、`vram`、`coolers`、`pcie_lanes` |
| `query_settings(gpu, backends="both")` | both | 已应用的设置：`voltage_boost_current`、`power_limit_current`、`thermal_limit_current`、`core/mem_clock_current`、`supported_pstates`、`pstate_ranges`、`fan_count/min/max`、`temperature_thresholds`、`vfp_locks`、legacy 过压 |
| `query_supported_applications_clocks(gpu, backends="nvml")` | NVML | App-clock 清单 |
| `query_power_limits(gpu)` | NVML | `{min_watt, current_watt, max_watt}` |
| `query_pstates(gpu)` | NVML | P-State 时钟范围 |
| `query_fan_info(gpu)` | NVML | `{count, min_percent, max_percent, current_percent}` |
| `query_temperature_thresholds(gpu)` | NVML | `[{name, celsius}]` |
| `query_throttle_reasons(gpu)` | NVML | `[{name, active}]` |
| `query_auto_boost(gpu)` | NVML | `{enabled, default_enabled}` |
| `query_api_restriction(gpu, api_type)` | NVML | `api_type`：`"app-clocks"` / `"application-clocks"` / `"auto-boost"` / `"autoboost"` |
| `query_legacy_gpc_rail_volt_range(gpu, pstate=None)` | NVAPI | Legacy 核心过压范围（uV） |
| `query_pstate_base_voltage(gpu, pstate=None)` | NVAPI | 基准电压 + 可编辑 delta 窗口（默认 P0） |
| `query_voltage_boost(gpu)` | NVAPI | `{voltage_boost_percent}` |
| `get_display_list(gpu, all=False)` | NVAPI | 显示器描述（`display_id`、连接器、flags、active/connected 布尔） |
| `query_edid(gpu, display_id)` | NVAPI | `{display_id, bytes, edid_hex}` |
| `query_clock_offset(gpu, backends, domain, pstate)` | NVML 或 NVAPI | `{mhz}`。NVML：整数 MHz；NVAPI：实时 P-State delta，带亚 MHz 分辨率。`pstate` 传 `None`（= P0） |
| `query_public_vftable(gpu, domain=None, infer_missing_default=True)` | NVAPI | 公开 V-F curve 点：`[{index, voltage_uv, frequency_khz, delta_khz, default_frequency_khz, point_type}]`；`point_type` = `prog` / `fixed` / `dyn` |
| `query_vbios_vf_curve(gpu)` | NVAPI + core 解析器 | vBIOS boost 阶梯（见下） |
| `query_vfp_point_voltage(gpu, point)` | NVAPI | `{microvolts}` |
| `query_tdp_temp_limits(gpu)` | NVAPI | `{min_tdp, default_tdp, max_tdp, min_temp, default_temp, max_temp}`（缺失条目省略） |
| `probe_voltage_limits(gpu)` | NVAPI | `{lower_point, upper_point}` |
| `check_voltage_frequency(gpu, point)` | NVAPI | `{precise, matched_point, microvolts}` |
| `query_tgp_watt_range(gpu)` | NVAPI | TGP 策略 `{policy_index, min_watt, default_watt, max_watt}`，或 `None` |
| `query_dnotifier(gpu)` | NVAPI | `{active: "D3", levels: [{level, watts}]}`，或 `None` |
| `query_power_ceiling(gpu)` | NVAPI | 生效的 PPAB 功耗墙（见下） |
| `query_target_temp_policies(gpu)` | NVAPI | 私有 ClientThermalTarget 表（GET-prime 0xC4554575） |

示例：

```python
import pynvoc

# 1. 枚举 GPU（刷新进程级 inventory 缓存）
gpus = pynvoc.discover_gpus("both")
gpu = gpus[0]["gpu_id_hex"]                # 例如 "0x0700"

# 2. 写入之前先读取
info = pynvoc.query_info(gpu, "both")      # 身份、范围、能力标志
status = pynvoc.query_status(gpu, "both")  # 实时仪表盘负载
settings = pynvoc.query_settings(gpu, "both")
print(info["name"], info["gpu_series"], info["is_ampere_plus"])
print(status["temperature_c"], status["power_w"], status["gpu_clock_mhz"])
print("core range MHz:", info["core_clock_min"], "-", info["core_clock_max"])
```

### 写入 API

所有写入立即生效且进程级有效；除非驱动另行说明，重启后不会保留。NVAPI 写入需要提权进程；私有 admin 门控家族在下文标注。

**时钟（公开路径）**

- `set_clock_offset(gpu, backend, domain, value_mhz, pstate)` — 允许一位小数 MHz（可整除 2.5 MHz 的 GUI 网格）；NVAPI 以 kHz（就近取整）提交，NVML 仅整数 MHz。`pstate` 传 `None`（= P0）。
- `reset_core_clocks(gpu, backend)` / `reset_mem_clocks(gpu, backend)` — NVAPI 路径会同时清除 V-F 频率锁*和* P0 全局偏移。
- `set_pstate_clock_offset(gpu, pstate, domain, delta_khz)` / `reset_pstate_clock_offsets(gpu, [("P0", "core"), ...])` — NVAPI 按 P-State 的 delta（kHz）。
- `set_locked_clocks(gpu, backend, domain, min_mhz, max_mhz)` / `reset_locked_clocks(gpu, backend, domain)` — 仅 NVML 路径。
- `set_applications_clocks(gpu, memory_mhz, graphics_mhz)` / `reset_applications_clocks(gpu)` — NVML app clocks。
- `set_legacy_clocks(gpu, core_mhz, memory_mhz)` — NVAPI 绝对值 legacy 时钟。
- 公开 V-F curve 编辑（NVAPI，kHz delta）：`set_vfp_point_delta(gpu, point, delta)`、`set_vfp_range_delta(gpu, start, end, delta)`、`set_domain_vfp_deltas(gpu, domain, [(point, delta), ...])`、`reset_vfp_deltas(gpu, domain)`（`"all"`/`"core"`/`"memory"`）。
- `set_vfp_frequency_lock(gpu, domain, upper_khz, lower_khz=None)` / `reset_vfp_frequency_lock(gpu, domain)`。
- `set_vfp_voltage_lock(gpu, point, voltage_uv, feedback=False)` — GPC 电压锁；`point` 与 `voltage_uv` **二选一**（另一个传 `None`）。`reset_vfp_lock(gpu)` 清除。
- `sync_memory_pstate_as_p0(gpu)` — 显存 P-State 耦合的 NVAPI 辅助。

**电压**

- `set_voltage_boost(gpu, percent)` / `reset_voltage_boost(gpu)`。
- `set_legacy_voltage_delta(gpu, uv, pstate=None)` / `set_pstate_base_voltage(gpu, pstate, delta_uv)` / `reset_pstate_base_voltages(gpu)`。
- `set_volt_rail_offset(gpu, rail_bit, offset_uv, expect_type=None)` 与 `set_volt_rail_target(gpu, rail_bit, target_mv, expect_type=None)` — 私有多轨电压墙（50 系 core+Xbar、GB10）。target 形式会在内部推导 offset，并返回 `{base_wall_uV, offset_uV, applied_uV, effective_wall_uV, ...}`；驱动会钳制到 `min(target, vbios_wall, vrm_max_wall)`。先调用 `query_volt_rails(gpu)` 枚举电压轨。
- `set_core_voltage_control(gpu, value)` — 私有核心电压控制对象（SET 0xDC2BD4A6，**admin**）；读取用 `get_core_voltage_control(gpu)`（GET 0xA91F88EB）。

**功耗 / 温度**

- `set_power_limit(gpu, backend, value)` — 单位随后端不同：NVML 为**瓦特**，NVAPI 为**百分比**。
- `set_nvapi_power_limits(gpu, [percent, ...])` / `reset_nvapi_power_limits(gpu)`；`set_nvapi_sensor_limits(gpu, [celsius, ...])` / `reset_nvapi_sensor_limits(gpu)`；`set_thermal_limit(gpu, celsius)`（优先 NVAPI，NVML 兜底）。
- `set_target_temp(gpu, celsius, policy_index=None)`。
- `set_perf_freq_cap(gpu, max_mhz, min_mhz)` — perf-cap 钳制（NVAPI PerfLimitsSetStatus 0x32CA4983，即参考工具的 `-gpuclk` setter）；`-1, -1` 重置，`0` 表示该侧不动。
- `set_tgp_watt(gpu, watts, policy_index=None)` / `reset_tgp_watt(gpu, policy_index=None)`（默认策略 2）。
- `set_ppab_status(gpu, active)` 与 `set_dnotifier(gpu, level)` — 见下文 PPAB 一节。
- `set_temp_sim(gpu, temperature_c)` / `reset_temp_sim(gpu)` — **危险的研究工具**：伪造驱动可见温度。Secured-Overrides 门控；读取用 `get_temp_sim(gpu)`。

**风扇**

- `set_fan(gpu, backend, fan_id, policy, level)` — `fan_id`：`"all"`、`"1"`、`"2"` 或索引；`level` 默认 60。`policy="auto"` 是**真正的重置**（见下文风扇一节）；NVML 接受 `"continuous"` / `"manual"`；NVAPI 额外接受 cooler-policy 名称（`"default"`、`"perf"`、`"discrete"`、`"continuous"`、`"hybrid"`、`"software"`、`"default32"`）。
- `reset_fan_speed(gpu, fan_index)` — NVML `nvmlDeviceSetDefaultFanSpeed_v2`（文档定义的"恢复默认控制策略"调用），带 TDR 恢复包装。
- `set_cooler_levels(gpu, policy, level, target_name=None)` / `reset_cooler_levels(gpu)` — NVAPI cooler 策略表。
- `set_fanstop_status(gpu, enable, curve_index=None)` — 停转槽位开关（NVAPI FanArbiterSet NDA 0x44CD3014，struct magic 0x10144）。
- `set_fan_rpm(gpu, rpm, cooler_index=None)` — 经私有 FanCoolerSetControl（NDA 0xEB44E8AA）模拟转速；`rpm=-1` 取消模拟（回到自动）。

**P-State 锁定**

- `set_nvapi_pstate_lock(gpu, first_pstate, second_pstate=None)` / `set_nvml_pstate_lock(...)` — 通过推导的显存时钟窗口锁定单个 P-State 或连续区间。干净应用返回 `None`；当相同显存时钟导致区间不可分时返回警告字符串（锁仍会应用，警告交由 UI 展示）。
- `set_pstate_native_lock(gpu, pstate)` / `reset_pstate_native_lock(gpu)` — 直接钉住单个 P-State（NVAPI PerfClientLimitsSetStatus NDA 0x39442CFB，mode-1）。在 NVML 区间推导无法运行的 pre-Kepler GPU 上作为兜底。
- `set_pstate_lock_level(gpu, level)` — 免提权的 perf-level 锁（SetPerfLevel 0x75DD3E6A，escape 0x7000040）。`level` 是**该 GPU 真实可用 P-State 列表的索引**，不是固定枚举；没有解除值 —— 重启/驱动重载即清除。

**其他**

- `set_auto_boost(gpu, enabled)` / `set_auto_boost_default(gpu, enabled)`；`set_api_restriction(gpu, api_type, restricted)` — NVML。
- `set_edid(gpu, display_id, edid_hex)` / `clear_edid(gpu, display_id)` — EDID 覆写。
- `set_background_oc_scanner(gpu, enable)`（0x06DC7CE8）；`get_oc_scanner_incomplete(gpu)`（0xBE371D0A）。
- `get_pmgr_arbiter(gpu)`（GET 0x717648FD） / `set_pmgr_arbiter(gpu, values)`（SET 0x9C4BB8D0，**admin**）— PMGR 电压请求仲裁器；`values` 必须恰好 11 个 dword；推荐 GET-修改-SET。
- `get_rated_tdp(gpu)` — 额定 TDP 读取三件套（0xED2BEA09 / 0x87BD35EF / 0xFCBDF642）：`{control_mode, info_capabilities, status_raw}`。
- `reset_all(gpu, domain=None)` — 全局恢复按钮：归零电压 boost，重置温度/功耗上限、cooler 等级、公开 V-F 偏移 + GPC 锁、legacy 过压、P0 时钟偏移（NVAPI 侧）以及 NVML 频率锁。`domain`：`"all"`/`"core"`/`"memory"`。

### 私有 / 实验性接口

以下接口封装逆向得到的私有 NVAPI 家族。驱动未暴露对应家族时返回 `{"supported": false}`；其中若干受驱动版本、显卡世代或提权状态门控。请将它们视为实验性接口；CLI 暴露同样的能力（见 [[CLI-Command-Reference]]）。

#### query_cooler_info — 私有 NVAPI cooler 家族

`query_cooler_info(gpu) -> {"count": int, "coolers": [{"index", "type", "min", "max", "current", "current_pwm_percent"}]}`

由私有 `FanCoolerGetInfo`（0x65CE5BFC）支撑，仅 NVAPI。前端将其与 NVML 风扇信息配对做世代判定：**NVML 风扇数 >= 1 且私有 NVAPI cooler 家族为空，即 <= Kepler 驱动的特征**；现代显卡同样通过该家族上报 cooler。

```python
coolers = pynvoc.query_cooler_info(gpu)
print(coolers["count"], coolers["coolers"])
```

#### NVAPI 风扇重置 — 优先清除控制块的 override 位

`set_fan(gpu, "nvapi", fan_id, "auto", None)` 分两级执行风扇*重置*：

1. **现代显卡：** `ResetNvapiFanControl` 清除控制块中的 cooler level-override 位（bit0）—— 在 1650S + A4000 上的实测 A/B 证明这是**唯一**能真正解除 level 钉住的重置（0x214AC 的重置位掩码会被接受但 pin 仍在；`RestoreCoolerSettings` / `RestoreCoolerPolicyTable` 返回 NOT_SUPPORTED）。
2. **旧驱动**（如 R391/Fermi）直接拒绝 NDA ClientFanCoolers 家族，此时绑定回退到公开的 `RestoreCoolerSettings`（`reset_cooler_levels`）。只有**两条**路径都失败时才报合并错误 —— NDA 优先正是无 cooler 表桌面显卡重置生效的原因。

`"auto"` 是真正的重置，刻意不用 SW 温度连续策略：在 ClientFanPolicies curve 表为空的 GPU 上（默认风扇控制位于固件侧），切到曲线跟随模式会让风扇在温度飙升前一直停在接近 0 RPM。

```python
pynvoc.set_fan(gpu, "nvapi", "all", "manual", 60)   # 固定 60% level
pynvoc.set_fan(gpu, "nvapi", "all", "auto", None)   # 真重置（清除 override）
```

#### NVML 风扇 TDR 恢复 — 全局 shutdown + 重新发现 + 重试

`reset_fan_speed` 与 `set_fan` 的 NVML 分支包在 TDR 恢复包装里。OC 不稳定触发驱动 TDR 后，进程缓存的 NVML 实例已死 —— `SetFanControlPolicy` 报 NotFound —— 而对已初始化的库再次 `Nvml::init()` 返回 AlreadyInitialized，普通刷新重建不出活实例。首次失败时包装器会：(1) 强制执行全局 `nvmlShutdown`，(2) 强制完整重新发现（全新 `Nvml::init` 重枚举），(3) 重试一次。仍失败则抛出恢复后的错误。

#### query_vbios_vf_curve — vBIOS boost 阶梯数据

`query_vbios_vf_curve(gpu) -> dict`

读取完整 VBIOS 镜像（`QueryVbiosImage`），用 core 的 legacy BIT 族 vBIOS 解析器解码 GPU Boost 2.0 boost-ladder 表（版本 0x10，BIT `'P'`+0x34）—— 79 个五字节点（u16 半 MHz 频率 + vmap 电压索引）加 P-State 边界标记。这就是 Maxwell/Kepler 的"隐藏" V-F curve；GUI/TUI 将其渲染为只读的 BIOS VF 阶梯。镜像读取是多次 escape 的循环（2 KB 分块），并释放 GIL。

负载：`available`、`table_version`、`curve_start_index`、`curve_end_index`、`points[{index, freq_mhz, v_min_mv, v_max_mv, vmap_index}]`、`pstate_marks[{pstate, code_raw, index}]`、`warnings`。镜像中没有 boost 阶梯（非 Maxwell/Kepler 世代）时返回 `{"available": false}` 而不是抛错，便于上层静默隐藏该功能。

```python
curve = pynvoc.query_vbios_vf_curve(gpu)
if curve["available"]:
    start, end = curve["curve_start_index"], curve["curve_end_index"]
    for p in curve["points"][start:end + 1]:
        print(p["index"], p["freq_mhz"], p["v_min_mv"], "-", p["v_max_mv"], "mV")
```

#### set_ppab_status 与生效功耗上限

- `set_ppab_status(gpu, active: bool)` — 经 `SetNvapiDynamicBoost` 切换 PPAB（移动端 dynamic-boost 墙）。这是一个**改名后**的绑定（GUI/TUI 的 PPAB 开关已迁移到 `set_ppab_status`）；**没有读取回显**：`get_dynamic_boost` 封装已撤回，因为 GET 0xC80068A1 读的是 PCF 平台状态字节，而不是这里写入的 PPAB 使能位。
- `query_power_ceiling(gpu)` — 生效的 PPAB 功耗墙（nvidia-smi 的 "Ceiling" 三元组）：`{policy_index, default_watt, requested_watt, dnotify_watt, ceiling_watt}`，其中 `ceiling_watt = min(请求的 TGP, 生效的 D-Notifier 上限)`。这是"你设了 100 W —— 实际生效的是这个"的值，GUI/TUI 的功耗滑杆以它为锚。私有家族不可用时返回 `None`。

```python
pynvoc.set_ppab_status(gpu, True)       # 启用 PPAB（移动端 dynamic-boost 墙）
pynvoc.set_tgp_watt(gpu, 140, None)     # 请求 140 W
ceiling = pynvoc.query_power_ceiling(gpu)
print(ceiling["requested_watt"], "->", ceiling["ceiling_watt"], "W")
```

#### 世代判定与私有 V-F 原始记录传输

`query_info` / `discover_gpus` 携带 core 的世代判定（依据名称 + 代号的 `detect_gpu_type`）：`gpu_series`、`is_mobile`、`is_server`、`is_legacy_voltage`、`xbar_supported`，以及 `is_ampere_plus`（query_info）。这些标志决定前端行为 —— 能力判断必须来自它们，而不是原始 arch 字符串（Ada 在那里读到 `Unknown`）。

- **`is_ampere_plus`** 标记 ClkDomains bit1 耦合分界：30 系与 Ada 的 bit1 域与 SYS 耦合，bit3 写入需要 `-f` 抵消。
- **Blackwell**（`GpuType::is_blackwell()`，50 系消费/工作站/服务器）门控 ClkDomains 的 plane-slot 偏移（slot2 = freq / slot3 = volt，2026-09-02 实测用户探针）。`query_private_vftable` 会逐点序列化 `volt_offset_uV` —— 带符号，**仅 Blackwell 记录非零**，其余世代为 0。
- **原始记录传输：** 私有 V/F-points 家族（GetInfo 0x8895B510 -> GetStatus 0x7FEE9032）可以让原始 GetStatus 记录直通（`include_raw`），SetControl 侧（ID 0xFEC00D04）在单个读-改-写周期内写入逐点记录，带快照 + 读回 + 失配恢复。当传统公开 VFP 接口不受支持时，这些是 GUI/TUI V-F curve 编辑器"私有/raw"路径背后的接口：

  - `query_private_vftable(gpu)` — bank 掩码、段（`vf_curve` / `pstate_bins`，附经验性的 `domain` 提示）与点（`voltage_uV`、`freq_default_mhz`、`freq_current_mhz`、`volt_current_uV`、`volt_offset_uV`、EXT 段 `domain_freq_mhz` / `domain_volt_uV`）。退化电压段（驱动把电压留 0，如 GP100/TCC）会按索引借用公开 GPC 电压网格修补，保证绘图有真实电压轴。
  - `set_vfp_point_private(gpu, bank, idx, value_khz, freq_mode)` — 写单个点。`freq_mode=True` = mode 0（u32 kHz 字段；**Pascal** 自动应用 2x 轴缩放）；`freq_mode=False` = mode 1（原始 i16 delta，在 Pascal 上 1:1）。
  - `set_vfp_range_per_point_private(gpu, bank, start, end, deltas)` — 单个 RMW 周期内写入 mode-1 原始 i16 逐点 delta。
  - `clk_vf_delta_for_target_mhz(def_mhz, target_mhz, class)` — 纯计算（不触 GPU）：经通用 g(def) 先验把 MHz 目标换算为 mode-1 原始 delta；`class` = `"graphics"`/`"gpc"` 或 `"fabric"`/`"xbar"`/`"msd"`。返回 `{"delta": int}`；先验在该默认频段无系数时返回 `{"delta": None}`。
  - `reset_vfp_private(gpu, bank, only_mode=None)` — 清除私有 raw/converted 偏移的**唯一**途径（公开的 `reset_vfp_deltas` 走 pstate20 / 公开 Client VfPoints 家族，触及不到私有状态）。

```python
# 私有 raw-converted 点写入（公开 VFP 不受支持时）：
delta = pynvoc.clk_vf_delta_for_target_mhz(1800, 1900.0, "graphics")["delta"]
if delta is not None:
    pynvoc.set_vfp_point_private(gpu, 0, 42, delta, False)   # mode-1 原始 delta
```

相关私有时钟接口：`query_private_freq_domain_info(gpu)`（可控域控制块，RM GET_CONTROL 0x2080901b）、`query_clk_domain_freq(gpu, domain_bit)`（两采样 MEASURE_FREQ，RM 0x20809006；域位 GPC=0、XBAR=1、SYS=2、MCLK=4）、`query_private_freq_domain_status(gpu, domain_bit)`（green-curve 直接测量 0x527FC458，无 50 ms 睡眠，HBM 已做除法），以及 `set_clk_domain_offset(gpu, domain_bit, offset_khz, slot=None, temporary=None)`（SET_CONTROL 0x2080d01c；slot 0 = 有符号频率偏移，slot 映射随世代不同；`temporary=True` 会在返回前恢复快照；**不强制幅值限制 —— 范围策略由调用方负责**）。这些在源码中均标注为 **DANGEROUS GPU clock writes**。

### 世代 / 驱动 / 提权注意事项

- **NDA 家族需要新驱动。** ClientFanCoolers、PerfClientLimits、ClientThermalTarget、ClockClient 与私有 V/F-points 家族在旧的用户态 DLL 中不存在（R391/Fermi 实测：家族调用返回 NVAPI_ERROR -1）。绑定会优雅降级：风扇重置走公开回退，读取返回 `{"supported": false}`。
- **旧显卡（<= Kepler）。** `is_legacy_voltage` 标记 legacy 电压路径（GT730 一类）。在旧驱动上 NVML 初始化本身可能失败；依赖 NVML 的读取会在已实现的场景回退到 NVAPI 派生数据（如 P-State 清单）。此时请用 `set_pstate_native_lock` 代替 `set_nvapi_pstate_lock`。
- **Maxwell/Kepler** 没有运行时 V-F curve —— 只有只读的 vBIOS boost 阶梯（`query_vbios_vf_curve`）。
- **Pascal：** 私有 mode-0 kHz 写入带 2x 轴编码；绑定会自动应用（结果中以 `pascal_2x_write` 报告）。mode-1 原始 delta 为 1:1。
- **Ampere+（30 系、Ada）：** `is_ampere_plus` —— ClkDomains bit1 与 SYS 耦合；需要 bit3 的补偿写入来抵消。
- **Blackwell（50 系）：** ClkDomains plane-slot 偏移（slot2=freq/slot3=volt）；`volt_offset_uV` 仅存在于 Blackwell 的 VFP 记录中。
- **移动端 dGPU：** 1 Hz 的 `query_status` 轮询在设计上就是防段错误的（NVAPI-only 遥测 + 缓存的 NVML 功耗上限）；GCOFF 之后读取能力前先 `force_wake`。
- **提权。** NVAPI 写入需要提权（管理员）进程。源码明确标注 `set_core_voltage_control`、`set_pmgr_arbiter`（admin）与 thermal-sim 家族（Secured-Overrides 门控）为提权接口；`set_pstate_lock_level` 是显著的**免提权**锁。每个家族的恢复行为见 [[Safety-and-Recovery]]；各世代硬件支持见 [[GPU-Support-Matrix]]。

### 安全

1. **写前先读。** 每次写入都以 `query_info` 的范围 / `query_settings` 的当前值为锚；写入后再读回，确认驱动接受（并钳制）了该值。
2. **优先使用文档化的重置路径。** `reset_all(gpu)` 恢复完整的 NVAPI+NVML 默认集合；各家族的窄重置（`reset_core_clocks`、`reset_voltage_boost`、`reset_cooler_levels`、`reset_vfp_private` 等）影响面更小。私有偏移*必须*用 `reset_vfp_private` 清除。
3. **保持恢复行为可见。** 每条写入路径都与 [[Safety-and-Recovery]] 配对使用；所有绑定的 CLI 等价命令见 [[CLI-Guide]] 与 [[CLI-Command-Reference]]，其中同样是单位与范围语义的参考。

*Maintained from: nvoc-python/README.md, nvoc-python/src/*, nvoc-python/python/pynvoc/**.*
