# Container Usage

`nvoc-cli` can run inside an NVIDIA Container Toolkit container when the host
driver is visible and the built binary is bind-mounted from `target/`.

The examples below use the local release binary and a Kali rolling image. They
do not require a project Dockerfile.

## Baseline command

Build `nvoc-cli` on the host, then mount `target/` into the container:

```bash
sudo docker run --rm --gpus all \
  -v "$PWD/target:/target:ro" \
  kalilinux/kali-rolling:latest \
  /target/release/nvoc-cli --help
```

Read-only NVML commands work with the same basic shape:

```bash
sudo docker run --rm --gpus all \
  -v "$PWD/target:/target:ro" \
  kalilinux/kali-rolling:latest \
  /target/release/nvoc-cli --output json --nvml get-power-watt
```

Confirmed read-only paths include:

- `list-gpus`
- `get-power-watt`
- `get-fan-info`
- `get-pstates`
- `get-temperature-thresholds`
- `get-throttle-reasons`
- `get-clock-offset-mhz`

Some GPU/driver combinations may still report `NotSupported` for specific NVML
features, for example auto-boost.

## NVAPI in containers

The NVIDIA runtime mounted `libnvidia-ml.so.1` automatically in testing, but it
did not mount `libnvidia-api.so.1`. Without that library, NVAPI commands failed
with:

```text
NvAPI_EnumPhysicalGPUs failed: LibraryNotFound
```

Mount the host NVAPI library read-only when using `--nvapi`:

```bash
sudo docker run --rm --gpus all \
  -v "$PWD/target:/target:ro" \
  -v /usr/lib/x86_64-linux-gnu/libnvidia-api.so.1:/usr/lib/x86_64-linux-gnu/libnvidia-api.so.1:ro \
  kalilinux/kali-rolling:latest \
  /target/release/nvoc-cli --output json --nvapi get-info
```

With this mount, read-only NVAPI commands such as `get-info` and `get-status`
worked.

## Write privileges

Normal `--gpus all` containers can read, but write commands failed with
`NoPermission` or `NVAPI_INVALID_USER_PRIVILEGE`.

For the tested host, write paths required:

- running as root inside the container
- `--cap-add SYS_ADMIN`
- the `libnvidia-api.so.1` bind mount for NVAPI writes

Example NVML write-capable command:

```bash
sudo docker run --rm --gpus all --cap-add SYS_ADMIN \
  -v "$PWD/target:/target:ro" \
  kalilinux/kali-rolling:latest \
  /target/release/nvoc-cli --output json --nvml set-power-watt 241
```

Example NVAPI write-capable command:

```bash
sudo docker run --rm --gpus all --cap-add SYS_ADMIN \
  -v "$PWD/target:/target:ro" \
  -v /usr/lib/x86_64-linux-gnu/libnvidia-api.so.1:/usr/lib/x86_64-linux-gnu/libnvidia-api.so.1:ro \
  kalilinux/kali-rolling:latest \
  /target/release/nvoc-cli --output json --nvapi --pstate P0 set-core-offset-mhz 1
```

Non-root container users could read NVML data, but non-root plus
`--cap-add SYS_ADMIN` still failed NVML power writes in testing.

## Tested write paths

The following conservative non-default writes were tested and then restored:

- `--nvml set-power-watt 241`, restored to the original `242 W`
- `--nvml --pstate P0 set-core-offset-mhz 1`, restored to `0`
- `--nvml --fan 0 --policy manual set-fan-percent 31`, restored with `reset-fan`
- `--nvml --domain core set-locked-clocks-mhz 210 405`, restored with `reset-locked-clocks`
- `--nvapi --pstate P0 set-core-offset-mhz 1`, restored to `0`

After write testing, verify final state with read-only commands:

```bash
/target/release/nvoc-cli --output json --nvml get-power-watt
/target/release/nvoc-cli --output json --nvml --domain core --pstate P0 get-clock-offset-mhz
/target/release/nvoc-cli --output json --nvml get-fan-info
```

## Safety notes

Container isolation does not isolate the GPU's hardware state. Any write from
inside the container applies to the host GPU and can affect host workloads,
display stability, clocks, fans, power limits, and driver state.

Prefer read-only probes first. For write tests, use small reversible values,
record the original state, restore immediately, and run final readback checks.

---

*Maintained from: container investigation on `kalilinux/kali-rolling:latest`,
NVIDIA Container Toolkit, RTX 4070 SUPER, driver `610.43.02`.*
