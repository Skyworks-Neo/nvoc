# GTX 1650 SUPER macOS RM validation — 2026-10-16

Validated on x86_64 macOS 15.8.1 / 24H32 with a GTX 1650 SUPER (TU116,
PCI 10de:2187, subsystem 401a1458, 4 GiB VRAM), NullMoth 2.0.4 and RM / OGKM
610.57.04. The probe and Rust CLI ran as an ordinary user without sudo.

## Verified access

| Surface | Result |
| --- | --- |
| IOKit ctl/device connection, register-fd, version handshake | Passed; RM 610.57.04 |
| Root → device → subdevice allocation | Passed |
| GPU class list | 95 classes returned; individual class capabilities not implied |
| GPU name and PCI address through Rust CLI | NVIDIA GeForce GTX 1650 SUPER; 0000:01:00.0 |
| PCI / VBIOS metadata | Device 218710de; board 0x178, SKU 250, project G166 |
| VRAM / BAR1 / L2 | 4096 MiB / 256 MiB / 1024 KiB |
| Current P-state | P1 during validation |
| RUSD object allocation | Passed with NV00DE_ALLOC_PARAMETERS and poll mask 0x5B |

The native Rust backend exposes device discovery, information and refreshed
P-state through core and nvoc-cli. It does not initialize NVAPI/NVML or spawn
the probe executable. See [backend architecture and usage](../design/macos-rm-backend.md).

## Initial unresolved or unavailable surfaces

| Surface | Result | Interpretation |
| --- | --- | --- |
| EDPp limits | NV_ERR_NOT_SUPPORTED (0x56) | This command path is unavailable |
| PMUMON clocks | NV_ERR_NOT_SUPPORTED (0x56) | Another clock path is needed |
| GPUMON utilization / perf-level info | NV_ERR_INVALID_ARGUMENT (0x1f) | Calling parameters and prerequisites still require investigation |
| RUSD mapping | NV_ERR_INVALID_ARGUMENT (0x1f) | Allocation succeeds; mapping remains unresolved |
| Temperature, clocks, power, fan, utilization | No valid readings | Rust emits null / N/A and an unavailable reason |
| Performance writes / restoration | Not executed | Not exposed by the Rust backend |

RUSD initially failed allocation because the probe supplied null parameters and
returned before reaching its valid-parameter attempt. Passing the official
allocation parameters fixes that step. Mapping 4096 bytes returned
NV_ERR_INVALID_LIMIT (0x2e); requesting the allocated structure size (3152 bytes)
with read-only permissions instead returns INVALID_ARGUMENT. This does not
establish that the GPU or driver lacks RUSD support.

The rated-TDP GET control returned success with zero raw fields. This is only
command acceptance, not a valid rated-wattage or power-limit reading. The probe's
missing-data 0 / -1 placeholders must not be treated as measurements. Likewise,
probe exit code 0 means stage completion, not that every optional query passed.

## Validation performed

- Probe lint/link and six CPU-only CLI regression tests passed.
- Rust core / CLI: 147 regular tests passed; hardware tests stayed ignored by default.
- One explicitly enabled macOS RM read-only hardware test passed, including
  selection, repeated status queries and no NVAPI initialization.
- cargo fmt --all -- --check passed.
- cargo clippy -p nvoc-core -p nvoc-cli --all-targets --no-deps -- -D warnings passed.
- cargo check -p pynvoc and cargo build -p nvoc-cli --locked passed.
- Real CLI checks covered automatic and explicit backend selection, index/hex-ID
  GPU selection, unmatched GPUs, backend flag conflicts, help and refusal of
  unsupported writes.

All compilation and hardware execution occurred on the x86_64 test Mac. Local
development was limited to source editing and analysis. Full workspace builds
and Windows/Linux hardware checks were not run. The pinned nvapi-sys dependency
emits an existing unused-id warning on macOS; the submodule was not modified.
Raw logs remain outside the repository; later SSH failures prevented a fresh
repeat during PR preparation.

The transport currently discovers the first matching service only. Python,
GUI/TUI, optimizer and service integration, complete telemetry and performance
control remain future work. No private-memory smoke, channel workload,
performance SET, driver/EFI change or reboot was performed for Rust integration.

## Same-day update: RUSD telemetry now works

The initial RUSD and GPUMON failures above were caller errors. RM's Unix
mapping path identifies RUSD as system memory and associates the mapping
context with the **control device**. The probe had supplied a device fd.
Using the control fd for both the RM mapping context and IOConnectMapMemory64,
while retaining the exact allocation size and read-only permissions, succeeds.
No driver or EFI changes were needed.

GPUMON requires bufSize equal to the entire samples array, not one sample.
Correcting that changes its result to NV_OK. Its returned tracker is not yet
interpreted, so no utilization is inferred from that ring. The validated Rust
path gets utilization from RUSD instead. PMUMON and EDPp failures remain;
RUSD provides clocks and the enforced power limit through a different path.

The Rust backend now reads temperature, graphics/memory clocks, instantaneous
GPU power, enforced GPU power limit, maximum reported cooler RPM and GPU
utilization, together with per-field RUSD timestamps. Polled fields must advance
within the current session; unstable, unavailable or non-advancing data stays
null. Valid zero fan/utilization values stay zero. Mapping is explicitly removed
before object cleanup. Device discovery/info does not allocate a shared page.

Observed desktop-idle examples: temperature 48.5625 C, graphics 1530000 kHz,
memory 6000984 kHz, GPU power 11368 mW, enforced limit 100000 mW, fan 1603 RPM,
utilization 0 percent. These are RM-reported values, not an independently
calibrated power measurement or validation under GPU load. No performance
settings, polling interval, driver, EFI or reboot was changed for this work.

Follow-up validation: 150 core/CLI regular tests and two explicitly enabled
read-only hardware tests passed. The latter check repeated timestamp advancement
and no fd accumulation. CPU C checks cover invalid/in-progress/unchanged
timestamps, even sequences and stable copying; Rust checks cover fixed-point
conversion, valid zero readings, missing data and invalid-value rejection.
Rustfmt, core/CLI Clippy, strict C syntax checking, probe lint/build and its six
CPU-only regressions passed. The locked CLI build passed.

Six consecutive desktop-idle CLI queries returned fresh timestamps for every
reported field. Observed ranges: 48.75–49.1875 C, 11.385–11.578 W, 1599–1605 RPM;
graphics 1530 MHz, memory 6000.984 MHz, enforced limit 100 W and utilization 0%
were steady during that short run. This validates repeated idle reads, not
GPU-load response or calibration against an independent instrument.
