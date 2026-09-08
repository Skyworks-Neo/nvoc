# Container Usage

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

`nvoc-cli` can run inside an NVIDIA Container Toolkit container when the host driver is visible and the built binary is bind-mounted from `target/`. For the full command list, see [[CLI-Guide]]; for recovery practices, see [[Safety-and-Recovery]].

The examples below use the local release binary and a Kali rolling image. They do not require a project Dockerfile.

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

Some GPU/driver combinations may still report `NotSupported` for specific NVML features, for example auto-boost.

## NVAPI in containers

The NVIDIA runtime mounted `libnvidia-ml.so.1` automatically in testing, but it did not mount `libnvidia-api.so.1`. Without that library, NVAPI commands failed with:

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

With this mount, read-only NVAPI commands such as `get-info` and `get-status` worked.

## Write privileges

Normal `--gpus all` containers can read, but write commands failed with `NoPermission` or `NVAPI_INVALID_USER_PRIVILEGE`.

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

Non-root container users could read NVML data, but non-root plus `--cap-add SYS_ADMIN` still failed NVML power writes in testing.

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

Container isolation does not isolate the GPU's hardware state. Any write from inside the container applies to the host GPU and can affect host workloads, display stability, clocks, fans, power limits, and driver state.

Prefer read-only probes first. For write tests, use small reversible values, record the original state, restore immediately, and run final readback checks.

---

*Maintained from: `docs/wiki/Container-Usage.md` — container investigation on `kalilinux/kali-rolling:latest`, NVIDIA Container Toolkit, RTX 4070 SUPER, driver `610.43.02`.*

<a id="chinese"></a>

## 中文

在宿主机驱动可见、且编译好的二进制从 `target/` 以 bind-mount 方式挂入的前提下，`nvoc-cli` 可以在 NVIDIA Container Toolkit 容器内运行。完整命令列表见 [[CLI-Guide]]；恢复实践见 [[Safety-and-Recovery]]。

以下示例使用本地 release 二进制和 Kali rolling 镜像，不需要项目自带的 Dockerfile。

## 基线命令

先在宿主机上编译 `nvoc-cli`，然后把 `target/` 挂载进容器：

```bash
sudo docker run --rm --gpus all \
  -v "$PWD/target:/target:ro" \
  kalilinux/kali-rolling:latest \
  /target/release/nvoc-cli --help
```

只读 NVML 命令使用相同的基本形态即可工作：

```bash
sudo docker run --rm --gpus all \
  -v "$PWD/target:/target:ro" \
  kalilinux/kali-rolling:latest \
  /target/release/nvoc-cli --output json --nvml get-power-watt
```

已确认可用的只读命令包括：

- `list-gpus`
- `get-power-watt`
- `get-fan-info`
- `get-pstates`
- `get-temperature-thresholds`
- `get-throttle-reasons`
- `get-clock-offset-mhz`

某些 GPU/驱动组合对特定 NVML 功能仍可能报告 `NotSupported`，例如 auto-boost。

## 容器中的 NVAPI

在测试中，NVIDIA runtime 自动挂载了 `libnvidia-ml.so.1`，但没有挂载 `libnvidia-api.so.1`。缺少该库时，NVAPI 命令会失败并报错：

```text
NvAPI_EnumPhysicalGPUs failed: LibraryNotFound
```

使用 `--nvapi` 时，需要以只读方式挂载宿主机的 NVAPI 库：

```bash
sudo docker run --rm --gpus all \
  -v "$PWD/target:/target:ro" \
  -v /usr/lib/x86_64-linux-gnu/libnvidia-api.so.1:/usr/lib/x86_64-linux-gnu/libnvidia-api.so.1:ro \
  kalilinux/kali-rolling:latest \
  /target/release/nvoc-cli --output json --nvapi get-info
```

挂载之后，`get-info`、`get-status` 等只读 NVAPI 命令可以正常工作。

## 写入权限

普通的 `--gpus all` 容器可以读取，但写入命令会以 `NoPermission` 或 `NVAPI_INVALID_USER_PRIVILEGE` 失败。

在受测主机上，写入路径需要：

- 在容器内以 root 运行
- `--cap-add SYS_ADMIN`
- NVAPI 写入还需要 `libnvidia-api.so.1` 的 bind mount

NVML 写入示例：

```bash
sudo docker run --rm --gpus all --cap-add SYS_ADMIN \
  -v "$PWD/target:/target:ro" \
  kalilinux/kali-rolling:latest \
  /target/release/nvoc-cli --output json --nvml set-power-watt 241
```

NVAPI 写入示例：

```bash
sudo docker run --rm --gpus all --cap-add SYS_ADMIN \
  -v "$PWD/target:/target:ro" \
  -v /usr/lib/x86_64-linux-gnu/libnvidia-api.so.1:/usr/lib/x86_64-linux-gnu/libnvidia-api.so.1:ro \
  kalilinux/kali-rolling:latest \
  /target/release/nvoc-cli --output json --nvapi --pstate P0 set-core-offset-mhz 1
```

在测试中，非 root 容器用户可以读取 NVML 数据，但非 root 加 `--cap-add SYS_ADMIN` 仍无法完成 NVML 功耗写入。

## 已测试的写入路径

以下保守的非默认写入操作均经过测试并随后恢复：

- `--nvml set-power-watt 241`，恢复为原始的 `242 W`
- `--nvml --pstate P0 set-core-offset-mhz 1`，恢复为 `0`
- `--nvml --fan 0 --policy manual set-fan-percent 31`，用 `reset-fan` 恢复
- `--nvml --domain core set-locked-clocks-mhz 210 405`，用 `reset-locked-clocks` 恢复
- `--nvapi --pstate P0 set-core-offset-mhz 1`，恢复为 `0`

写入测试完成后，请用只读命令核对最终状态：

```bash
/target/release/nvoc-cli --output json --nvml get-power-watt
/target/release/nvoc-cli --output json --nvml --domain core --pstate P0 get-clock-offset-mhz
/target/release/nvoc-cli --output json --nvml get-fan-info
```

## 安全注意事项

容器隔离并不能隔离 GPU 的硬件状态。容器内的任何写入都会直接作用于宿主机 GPU，可能影响宿主机的负载、显示稳定性、频率、风扇、功耗限制和驱动状态。

请优先使用只读探测。写入测试时，使用小且可逆的数值，记录原始状态，立即恢复，并执行最终的回读检查。

---

*维护来源：`docs/wiki/Container-Usage.md` —— 基于 `kalilinux/kali-rolling:latest`、NVIDIA Container Toolkit、RTX 4070 SUPER、驱动 `610.43.02` 的容器实测。*
