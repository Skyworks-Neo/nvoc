# GPU Compatibility Matrix

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

> **Canonical source**: the per-backend compatibility table is maintained in [cli/README.md → "Compatibility Overview"](https://github.com/Skyworks-Neo/nvoc/blob/main/cli/README.md#compatibility-overview-interface--gpu-generation--basic-functions); this page mirrors it. Where the two differ, `cli/README.md` wins.

### Supported GPU Generations

| Generation | Codename | Mode | Notes |
|---|---|---|---|
| RTX 50-series (Blackwell) | `GB` | VF Curve | Default aggressive BSOD recovery; Max-Q step calibration |
| RTX 40-series (Ada Lovelace) | `AD` | VF Curve | — |
| RTX 30-series (Ampere) | `GA` | VF Curve | — |
| RTX 20-series (Turing TU10x) | `TU10` | VF Curve | Minimal light/heavy load variation |
| GTX 16-series (Turing TU11x) | `TU11` | VF Curve | Minimal light/heavy load variation |
| GTX 10-series (Pascal) | `GP1` | VFP Curve (79 pts) | Minimal light/heavy load variation; recommended: `fix_result` |
| GTX 9-series (Maxwell) | `GM` | **Legacy Global Offset** | No per-point VFP; scanned via `autoscan-vfp-legacy` |
| Volta (compute) | `GV` | Legacy | Same as above |

> **Mobile GPUs** (name contains `Laptop`) cannot modify TDP/thermal limit/VDDQ boost; the tool auto-skips these.

> **Legacy vBIOS decode (read-only, landed 2026-09)**: `nvoc-cli get-vbios` reads the vBIOS image and decodes BIT-family tables (perf/boost/vmap/boost-ladder); `get-vbios --maxwell-vftable-decode` dumps the Maxwell GPU Boost 2.0 V/F ladder (79 GPC points) as a read-only Id/V/F table (commits `e19ea70` / `a19cc15`).

### NVAPI — Desktop Consumer GPUs

| Feature | RTX 50 | RTX 40 | RTX 30 | RTX 20 | GTX 16 | GTX 10 | GTX 9 |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| VF curve edit+export | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| autoscan | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| autoscan-vfp-legacy | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Core frequency offset | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Memory frequency offset | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Power limit | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Thermal limit | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Fan control (NVAPI) | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Fan reset (NVAPI) | ✅ (Linux❓) | ✅ (Linux❓) | ✅ (Linux❓) | ✅ (Linux❌) | ✅ (Linux❌) | ✅ (Linux❌) | ✅ (Linux❌) |
| Voltage point lock | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| Voltage point unlock | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| Core freq range lock | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Core freq range unlock | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Memory freq range lock | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | Partial |
| Memory freq range unlock | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | Partial |
| Voltage Boost | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| Overvolt (--voltage-delta) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ |

### NVML — Desktop Consumer GPUs

| Feature | RTX 50 | RTX 40 | RTX 30 | RTX 20 | GTX 16 | GTX 10 | GTX 9 |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| VF curve edit+export | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| autoscan | TODO | TODO | TODO | TODO | TODO | TODO | ❌ |
| autoscan-vfp-legacy | TODO | TODO | TODO | TODO | TODO | TODO | TODO |
| Core frequency offset | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Memory frequency offset | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Power limit | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Thermal limit | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Fan control | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Fan reset | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| Voltage point lock | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Core freq range lock | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❓ |
| Core freq range unlock | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❓ |
| Memory freq range lock | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ |
| Memory freq range unlock | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ |
| App freq range lock | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| App freq range unlock | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Voltage Boost | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Overvolt (--voltage-delta) | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

### Server-Class GPUs (NVAPI)

| Feature | Blackwell | Hopper | Ampere | Turing | Volta | Pascal |
|---|:---:|:---:|:---:|:---:|:---:|:---:|
| Power limit | ❓ | ❓ | ✅ | ❓ | ❓ | ✅ |
| Core freq range lock | ❓ | ❓ | ✅ | ❓ | ❓ | ✅ (Win❌) |
| Memory freq range lock | ❓ | ❓ | ❌ Crashes GPU! | ❓ | ❓ | ❌ |

### Workstation-Class GPUs (NVAPI)

| Feature | Blackwell | Ada | Ampere | Turing | Pascal |
|---|:---:|:---:|:---:|:---:|:---:|
| VF curve edit+export | ✅ | ✅ | ✅ | ❓ | ❓ |
| autoscan | ✅ | ✅ | ✅ | ❓ | ❓ |
| Core frequency offset | ✅ | ✅ | ✅ | ❓ | ❓ |
| Memory frequency offset | ✅ | ✅ | ✅ | ❓ | ❓ |
| Power limit | ✅ | ✅ | ✅ | ✅ | ✅ |
| Thermal limit | ✅ | ✅ | ✅ | ✅ | ✅ |
| Fan control | ✅ | ✅ | ✅ | ✅ | ✅ |
| Voltage point lock | ✅ | ✅ | ✅ | ❓ | ❓ |
| Core freq range lock | ✅ | ✅ | ✅ | ✅ | ✅ |
| Memory freq range lock | ✅ | ✅ | ✅ | ❓ | ❌ |
| Voltage Boost | ❌ | ❌ | ❌ | ❌ | ❌ |

### Platform Notes

#### Windows
- Full NVAPI + NVML support
- Driver model TDR provides automatic crash recovery

#### Linux
- NVAPI is essentially a `libnvidia-api.so` → `libnvidia-ml.so` translation layer
- Only NVML interface actually exists, but NVAPI GPU ID indexing works better for professional cards
- NVML frequency range lock maps to voltage range lock under the hood
- Unstable overclock recovery ceiling is generally lower than Windows TDR

#### Mobile / Laptops
- Boost voltage, fan control, thermal limit, and power limit control not supported
- Tool auto-detects mobile GPUs and skips these settings

---

<a id="chinese"></a>

## 中文

> **权威来源**：各后端的兼容性矩阵统一维护在 [cli/README.md → "Compatibility Overview"](https://github.com/Skyworks-Neo/nvoc/blob/main/cli/README.md#compatibility-overview-interface--gpu-generation--basic-functions)，本页面是其镜像。两者不一致时，以 `cli/README.md` 为准。

### 支持的 GPU 世代

| 世代 | 代号 | 模式 | 说明 |
|---|---|---|---|
| RTX 50 系（Blackwell） | `GB` | VF 曲线 | 默认 aggressive BSOD 恢复；支持 Max-Q 阶梯标定 |
| RTX 40 系（Ada Lovelace） | `AD` | VF 曲线 | — |
| RTX 30 系（Ampere） | `GA` | VF 曲线 | — |
| RTX 20 系（Turing TU10x） | `TU10` | VF 曲线 | 轻重载差异小 |
| GTX 16 系（Turing TU11x） | `TU11` | VF 曲线 | 轻重载差异小 |
| GTX 10 系（Pascal） | `GP1` | VFP 曲线（79 点） | 轻重载差异小；推荐执行 fix_result |
| GTX 9 系（Maxwell） | `GM` | **Legacy 全局偏移** | 不支持逐点 VFP；通过 `autoscan-vfp-legacy` 扫描 |
| Volta 计算卡 | `GV` | Legacy | 同上 |

> **移动端 GPU**（名称含 `Laptop`）无法修改 TDP/温度墙/VDDQ boost，工具会自动跳过。

> **Legacy vBIOS 解码（只读，2026-09 落地）**：`nvoc-cli get-vbios` 可读取 vBIOS 镜像并解码 BIT 族表（perf/boost/vmap/boost-ladder）；`get-vbios --maxwell-vftable-decode` 以只读方式输出 Maxwell GPU Boost 2.0 V/F 阶梯表（79 个 GPC 点）的 Id/V/F 表（提交 `e19ea70` / `a19cc15`）。

### NVAPI 接口 — 桌面消费级 GPU

| 功能 | RTX 50 | RTX 40 | RTX 30 | RTX 20 | GTX 16 | GTX 10 | GTX 9 |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| VF 曲线编辑+导出 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| autoscan | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| autoscan-vfp-legacy | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 核心频率偏置 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 显存频率偏置 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 功耗墙 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 温度墙 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 风扇控制（NVAPI） | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 风扇重置（NVAPI） | ✅ (Linux❓) | ✅ (Linux❓) | ✅ (Linux❓) | ✅ (Linux❌) | ✅ (Linux❌) | ✅ (Linux❌) | ✅ (Linux❌) |
| 电压点锁定 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| 电压点解锁 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| 核心频率范围锁定 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 核心频率范围解锁 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 显存频率范围锁定 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 部分支持 |
| 显存频率范围解锁 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | 部分支持 |
| Voltage Boost | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| Overvolt（--voltage-delta） | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ |

### NVML 接口 — 桌面消费级 GPU

| 功能 | RTX 50 | RTX 40 | RTX 30 | RTX 20 | GTX 16 | GTX 10 | GTX 9 |
|---|:---:|:---:|:---:|:---:|:---:|:---:|:---:|
| VF 曲线编辑+导出 | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| autoscan | TODO | TODO | TODO | TODO | TODO | TODO | ❌ |
| autoscan-vfp-legacy | TODO | TODO | TODO | TODO | TODO | TODO | TODO |
| 核心频率偏置 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 显存频率偏置 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 功耗墙 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 温度墙 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 风扇控制 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 风扇重置 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |
| 电压点锁定 | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| 核心频率范围锁定 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❓ |
| 核心频率范围解锁 | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❓ |
| 显存频率范围锁定 | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ |
| 显存频率范围解锁 | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ |
| App 频率范围锁定 | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| App 频率范围解锁 | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Voltage Boost | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Overvolt（--voltage-delta） | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |

### 服务器级 GPU（NVAPI）

| 功能 | Blackwell | Hopper | Ampere | Turing | Volta | Pascal |
|---|:---:|:---:|:---:|:---:|:---:|:---:|
| 功耗墙 | ❓ | ❓ | ✅ | ❓ | ❓ | ✅ |
| 核心频率范围锁定 | ❓ | ❓ | ✅ | ❓ | ❓ | ✅ (Win❌) |
| 显存频率范围锁定 | ❓ | ❓ | ❌ 会导致 GPU 崩溃！ | ❓ | ❓ | ❌ |

### 工作站级 GPU（NVAPI）

| 功能 | Blackwell | Ada | Ampere | Turing | Pascal |
|---|:---:|:---:|:---:|:---:|:---:|
| VF 曲线编辑+导出 | ✅ | ✅ | ✅ | ❓ | ❓ |
| autoscan | ✅ | ✅ | ✅ | ❓ | ❓ |
| 核心频率偏置 | ✅ | ✅ | ✅ | ❓ | ❓ |
| 显存频率偏置 | ✅ | ✅ | ✅ | ❓ | ❓ |
| 功耗墙 | ✅ | ✅ | ✅ | ✅ | ✅ |
| 温度墙 | ✅ | ✅ | ✅ | ✅ | ✅ |
| 风扇控制 | ✅ | ✅ | ✅ | ✅ | ✅ |
| 电压点锁定 | ✅ | ✅ | ✅ | ❓ | ❓ |
| 核心频率范围锁定 | ✅ | ✅ | ✅ | ✅ | ✅ |
| 显存频率范围锁定 | ✅ | ✅ | ✅ | ❓ | ❌ |
| Voltage Boost | ❌ | ❌ | ❌ | ❌ | ❌ |

### 平台说明

#### Windows
- 完整 NVAPI + NVML 支持
- 驱动模型 TDR 提供自动崩溃恢复

#### Linux
- NVAPI 本质是 `libnvidia-api.so` → `libnvidia-ml.so` 转译层
- 实际只存在 NVML 接口，但 NVAPI 的 GPU ID 索引对专业卡支持更好
- NVML 的频率范围锁定底层是电压范围锁定
- 不稳定超频恢复限度一般低于 Windows TDR

#### 移动端 / 笔记本
- 不支持 Boost 电压、风扇控制、温度墙、功耗墙控制
- 工具检测到移动端 GPU 会自动跳过这些设置
