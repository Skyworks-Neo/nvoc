# Native macOS RM backend

The Rust core and `nvoc-cli` expose a read-only NullMoth RM backend. Verified
hardware: GTX 1650 SUPER (`10de:2187`), macOS 15.8.1 / 24H32, NullMoth 2.0.4,
RM 610.57.04, ordinary user without sudo. This does not establish support for
every GPU claimed by NullMoth or other macOS releases.

## Build and run

Build on the x86_64 test Mac with Command Line Tools, the repository-pinned
Rust 1.95.0 toolchain, the pinned `nvapi-rs` submodule, and OGKM 610.57.04 headers:

```sh
export OGKM=/path/to/open-gpu-kernel-modules-610.57.04
cargo build -p nvoc-cli --locked
./target/debug/nvoc-cli get-gpu-list
./target/debug/nvoc-cli get-info --gpu 0 -O json
./target/debug/nvoc-cli get-status --macos-rm -O json
```

macOS selects RM automatically. `--macos-rm` selects it explicitly and conflicts
with NVAPI/NVML flags and library-path overrides. An explicit request on a
different architecture or operating system returns an unsupported-platform
error. Windows/Linux auto-selection remains unchanged. The NVAPI Rust crate is
still a compile-time dependency of the existing core, but the RM path never
initializes its runtime, nor NVML. No external probe executable or OGKM headers
are needed at runtime.

## Current command coverage

| Command | Behavior |
| --- | --- |
| `get-gpu-list` | GPU name, PCI address, NVOC ID, RM GPU ID, backend flags |
| `get-info` | RM version, PCI/VBIOS metadata, VRAM/BAR1/L2 sizes, class count |
| `get-status` | P-state and RUSD temperature, clocks, power, enforced limit, max fan RPM and GPU utilization; missing fields are `null` / `N/A` with reasons |
| `list` and offline `get-vbios --input` | Existing operations without GPU discovery |
| Other live queries and all settings writes | Refused before opening the driver |

Status exit code 0 means the query completed; per-field availability is
reported independently. Missing temperature, clocks, power, fan and utilization
values are never represented by zero. A valid zero RPM or zero utilization
reading is preserved. JSON clocks are kHz, power/limits are mW, temperatures
are Celsius, utilization is percent, and fan RPM is the maximum of the shared
page's cooler slots. Human output uses MHz, W, Celsius and RPM. Optional
information-query failures preserve their RM status in `unavailable`;
connection, version or object-chain failures return
an error. Capabilities describe implemented backend surfaces, not a guarantee
that every query succeeds on every board.

The current transport selects the first matching NVAccel/NVRM service. It does
not enumerate all physical GPUs in a multi-NVIDIA system. `--gpu` still applies
the existing NVOC index / bus-derived ID selection to the returned inventory.
RM GPU IDs are recorded separately and are not treated as PCI bus numbers.

## Architecture

```text
nvoc-cli → TargetInventory::discover(BackendSet::MacosRm)
         → GpuTarget::macos_rm()
         → run(QueryMacosRmInfo / QueryMacosRmStatus)
         → core::macos_rm → native/macos_rm.c
         → NVOC nvx_xnu / nvx_rm → IOKit → NullMoth RM
```

The C bridge compiles against the official OGKM headers and exports a small
versioned snapshot ABI. Its layout is size-checked in C and Rust. It reuses
NVOC's own transport, not upstream driver source. Each snapshot opens a session,
checks RM 610.57.04, creates the RM object chain, issues GET controls, and frees
objects and connections on success or failure. A Rust mutex serializes the
transport's process-global fd table. Status refresh checks device identity
against the discovery snapshot. Discovery/information queries do not allocate
a shared page. Status queries allocate RUSD with CLOCK/PERF/POWER/THERMAL/FAN
polling, map its system memory read-only through the **control fd**, and use
acquire fences around sequence-checked reads. Polled fields must advance from
the first timestamp observed in the current session, within a 1.5 s wait budget;
non-polled fields require a valid even sequence. Unstable, unavailable or
non-advancing fields return null with a reason. RUSD timestamps/sequences are
available in JSON as `rusd_timestamps`; they are diagnostic, not a single
cross-field snapshot time. The mapping is explicitly removed before freeing
the shared object and RM chain. An exceptional unmap failure drops telemetry
and defers resource reclamation to connection teardown. No polling-interval
override or performance SET controls are sent.

Rust callers can use the common operation reporting API:

```rust,no_run
use nvoc_core::{BackendSet, QueryMacosRmInfo, QueryMacosRmStatus, discover_targets, run};

let inventory = discover_targets(BackendSet::MacosRm)?;
for target in inventory.targets() {
    let info = run(&target, QueryMacosRmInfo)?.output;
    let status = run(&target, QueryMacosRmStatus)?.output;
    println!("{:?}: {:?}", info.name, status.pstate);
}
# Ok::<(), nvoc_core::Error>(())
```

The existing NVAPI-shaped `QueryGpuInfo` / `QueryGpuStatus` output types are
unchanged. RM callers use the explicitly typed operations above. Python, GUI,
TUI, the optimizer and service are not yet integrated with this backend.

## Validation

```sh
cargo test -p nvoc-core -p nvoc-cli --all-targets
cargo clippy -p nvoc-core -p nvoc-cli --all-targets --no-deps -- -D warnings
cargo test -p nvoc-core --test macos_rm_readonly -- --ignored --nocapture
```

The last command requires the initialized GPU and only performs GET queries
and RM resource allocation/cleanup. It verifies identity, VRAM, refreshed
P-state, missing-data semantics, selection, repeated timestamp advancement, no fd
accumulation and absence of NVAPI initialization. CPU protocol checks can also
be run on the build host:

```sh
cc -std=c11 -Wall -Wextra -Werror -Icore/native \
  -I"$OGKM/src/common/sdk/nvidia/inc" core/tests/rusd_read.c -o /tmp/nvoc-rusd-read
/tmp/nvoc-rusd-read
```
Performance-write tests remain ignored. Raw test output belongs in workspace
`outputs/`, outside the repository.
