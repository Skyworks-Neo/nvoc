# Linux NVAPI/NVML 版本覆盖审计（610.57.04 vs 470.256.02）

- 日期：2026-09-10（610 样本）/ 2026-09-09（470 样本）
- 样本：`reverse/linux-drivers/NVIDIA-Linux-x86_64-{610.57.04,470.256.02}/`（.run 经 `-x` 解压，二进制不入库）
- 工具：idalib MCP + `reverse/qi_table.py`（QI 表解析）/ `reverse/pe_probe.py`；表 JSON 在 `reverse/woa-qi-tables/`
- 关联：`woa-arm64-coverage-audit.md`（WOA 616.00 侧）、`version-coverage-audit.md`（Windows 侧既有审计）

## 1. libnvidia-api.so.1 架构（610.57.04）——"转译层"假说证伪

nvapi-rs Linux 后端 dlopen 的 `libnvidia-api.so.1`（798 KB，x86-64 ELF，stripped）**不链接 libnvidia-ml**：导入表仅 libc/libstdc++（ioctl、open/mmap64、posix_spawn、pthread 等，GOT 0xC5048–0xC52D0）。它是**独立的 RM 客户端**，与 NVML 平行：

- `nvapi_QueryInterface` @ **0x8AA0**（0x3C 字节）：纯线性扫描，未命中返回 NULL，无日志。
- 主表 VA **0xC12A0**，16 字节/项 `{u64 fnptr; u32 id; u32 pad}`，fnptr==0 终止，共 **590 项**（唯一 588）。
- ioctl 包装 `sub_9A530` @0x9A530（大参数块走 `0xC01046D3`，NV magic 0x46）；设备 `/dev/nvidia%d`（0xACD30）、`/dev/nvidia-caps/…`（0xACF49）、`/usr/bin/nvidia-modprobe`（0xAD562）、API-mismatch 版本检查串（0xACD60）。
- 调用链：handler → `sub_997F0`@0x997F0（RM 重试/重连）→ `sub_996C0` → `sub_963C0`@0x963C0（RM 命令大 switch，0x2080xxxx 型 control 码）→ `sub_9C410`（设备 open/close 特例 533/534/3336 等）→ `sub_9A530` ioctl。
- **无** Windows 侧的硬拉黑 ID、`EnableRID*` 注册表门控（字节级确认 0x33C7358C / 0x593E8644 / 0xAD298D3F 在整个 .so 中缺失）。
- 表头两项即生命周期：id0 `0x0150E828` → 0x8D70 `nvapi_Initialize`（mutex+引用计数，首次打开设备）；id1 `0xD22BDD7E` → 0x8D00 反初始化。

> 推翻 archive `GPU-Support-Matrix` 的 "libnvidia-api.so → libnvidia-ml.so 转译层" 结论（wiki 已同步修订）。Linux 上 NVAPI 控制面**不经过 NVML**，直接走内核 RM escape/control。

## 2. 470 分支：libnvidia-api 不存在

470.256.02 解压根目录**没有** `libnvidia-api.so*`（v525 才引入）。因此 nvapi-rs 在 470 Linux 上 `dlopen("libnvidia-api.so.1")` 必然 `LibraryNotFound`——470 及更早的 Linux 控制面只有 NVML。470 `libnvidia-ml.so.470.256.02` 导出 **255** 个。

## 3. nvoc 关键 ID 在 Linux 的覆盖（610）

| ID | 名称（nvid.rs） | Linux 表 | handler VA | 结论 |
|---|---|---|---|---|
| 0x8B3E7343 | ClientTgpWattGetStatus | ✅ idx21 | 0x515B0 | RM 直转（版本块 372912，多版本转换 0x11F10/0x11F40/…） |
| 0xAFFC2279 | ClientTgpWattSetStatus | ✅ idx22 | 0x51D20 | RM 直转 → ioctl |
| 0xAD298D3F | GPU_PrivateLifecycleInit | ❌ 字节级缺失 | — | QI 返回 NULL → 与 4060 Laptop 实测 NO_IMPLEMENTATION 互证；nvoc 的吞错逻辑（gpu.rs:4377-4387）保持必要 |
| 0xBFF09E59 | TgpWattSetStatus 变体 | ❌ | — | WOA 同样缺失 |
| 0x1504FC3D | ClientDynamicBoostSetStatus | ❌ | — | Linux 无 Dynamic Boost 写面 |
| 0x67F31384 | ClientPowerPoliciesGetInfoPrivate | ✅ idx11 | 0x54250 | RM 直转 |
| 0x7B30AE0D | PerfPstatesGetInfoPrivate | ✅ idx318 | 0x7BCA0 | RM 直转 |
| 0x9962C97C | ClientPStateLimitStatus | ✅ idx381 | 0x1CBE0 | RM 直转 |

nvoc 关键字家族（Pstate/Clock/Perf/Power/ClkVolt/Vf）在 590 项中命中 **191**；抽查的 18 个 ID 全部走同一条 RM 直转路径（明细见 JSON）。

## 4. 三方差集（Linux 590 × WOA 2345 × nvid.rs 2188）

- 交集 587；**Linux 独有 3**（2026-09-10 已注册入 `nvid.rs` Linux-only 段）：
  - `0xBF75A81E` = `NvAPI_SYS_GetDriverAndBranchVersionEx`（表 idx 3，handler @0x88C00；Windows 全系缺）
  - `0x4E0B05ED`（idx 112，handler @0x5FFB0）：GET 型，out V1/52B（2×u64+3×u32），888B RM 块走内部分发 selector **0xC4** —— 610 dispatcher（sub_963C0）**无此 case**，落 default → 恒 `NVAPI_NOT_SUPPORTED(-23)`；全库仅此一个调用点
  - `0x7B9681DA`（idx 211，handler @0x12D10）：SET 型伴随（in V1/8B，flag→{1,2} 枚举，12B 块走 selector **0xC0**）——同样无 case 恒 NOT_SUPPORTED
  - **定族**：两者落在已实现 selector 0xB3–0xD2 簇的中间，该簇说 GPU inforom/VPR（0x20800156/0x2080016B）、PERF_GET_POWERSTATE（0x2080205A）、NDA 内部组 0xA0xx（0x2080A084/0x2080A091）+ 0xA7xx（0x2080A707）、BUS C2C 低功耗（0x20801832/0x20801836）、FB carveout（0x20801360）——即**设备/平台管理（NDA 0xA0/0xA7 内部族）**，与 nvoc 驱动的 clock/perf/power/fan 族无关；open-gpu-kernel-modules 头文件缺 0xA0/0xA7 组（NDA），open-gpu 已稀疏克隆至 `reverse/open-gpu-kernel-modules/` 供后续内核侧夹击验证
- **WOA/Windows 独有 1756**（D3D/DISP/DRS/CUDA 图形栈大头）。
- nvid.rs 已注册 ID 中在 WOA 两表（x64=ARM64）**双双缺失 103 个**（D3D12 shading-rate 族、DISP/DRS/DIAG 部分面）→ 这些在 WOA 必然 NoImplementation；清单见 `reverse/woa-qi-tables/nvid-rs-reconcile.json`。
- nvoc 控制面核心（TGP/pstate/clkvolt/power）在三方全部覆盖或按上表明确缺失。

## 5. Linux NVML 导出面（对照 Windows）

- 470 = **255** 导出；610 = **417** 导出（恰等于 Windows R550 nvml.dll 的 417，见 `nvml/export-coverage-audit.md`）。
- 470→610：新增 163 / 移除 1（`nvmlRetry_NvRmControl`）；新增中 **149 个为 Set*/Get* 控制类**（ConfCompute、Capabilities、ClockOffsets 等）。
- 两条 RM 客户端平行性：libnvidia-ml.so 导入亦不依赖 libnvidia-api（见 §1），Linux 上 NVAPI/NVML 是**两条独立通道**打到同一内核 RM；Windows 上 nvapi64 与 nvml.dll 同样并存但共享内核面，一致性与 Linux 相同。
- NVML 控制函数的 ioctl/RM 命令证据链：见 §7（610 抽样）。

## 6. 对 nvapi-rs / nvoc 的直接影响

1. nvapi-rs Linux 库名 `libnvidia-api.so.1`（sys/src/nvapi.rs:15-16）正确，但**v525 以下驱动必然 LibraryNotFound**——值得在文档/报错信息中标注驱动版本下限。
2. 0xAD298D3F 缺失是字节级事实，nvoc 现有"吞 NoImpl"逻辑即为正确长期行为，不需要探测。
3. TGP Get/Set、PowerPolicies、Pstates、PStateLimit 在 Linux 走 libnvidia-api 的 RM 直转**理论可用**（与 4060 Laptop 实测 TGP 可用一致）。
4. Dynamic Boost 在 Linux 无写面（0x1504FC3D 缺失）——GUI/TUI 对应按钮在 Linux 应隐藏而非报错。

## 7. NVML 控制链证据（610，idalib 逆向抽样）

libnvidia-ml.so.610.57.04 与 libnvidia-api 一样是**独立 RM 客户端**：导入表 198 个符号全为 GLIBC（ioctl/open/mmap/pthread/dlsym），零 NVIDIA 库依赖；设备面 `/dev/nvidiactl`、`/dev/nvidia%d`、`/dev/nvidia-uvm`、`/dev/nvidia-nvswitchctl`。内嵌构建路径 `/dvs/p4/build/sw/rel/gpu_drv/r610/r610_85/apps/nvml/dmal`。

内核通道（与 nvapi-rs RM escape 地图同族）：

- RM CONTROL 总出口 `sub_144640`：构造 32B `nv_ioctl_rm_control_t`{hClient,hObject,cmd,params,size,rmStatus} → ioctl **`0xC020462A` = _IOWR(0x46,0x2A,32)**（NV_ESC_RM_CONTROL；常量 @0x145116，填充 @0x144874-0x1448BE）。特殊 cmd 0x2080212E 走 `0xC00846D5`。
- RM ALLOC `sub_143A80`：ioctl **`0xC030462B` = _IOWR(0x46,0x2B,48)**（NV_ESC_RM_ALLOC，@0x143B3E）；版本检查可被 env `__RM_NO_VERSION_CHECK` 跳过（sub_1431F0）。
- 中央 RmControl helper `sub_9E560`（~470 个调用者）；subdevice 句柄经 class **0x2080** 获取（0xC6C90 等）。

代表性控制/遥测链（nvoc 相关）：

| 函数 | VA | 内层 RM cmd | 备注 |
|---|---|---|---|
| nvmlDeviceSetGpcClkVfOffset | 0x6F990 | **0x2080F031**（params 0x608，无版本字段的老式清零式 NV2080 cmd） | 调用点 0x108BF6；wrapper→sub_6F920→cDeviceSetGpcClkVfOffset(0x108930) |
| nvmlDeviceGetClockOffsets | 0x51110 | — | NVML 结构层魔数 **0x1000018 = v1<<24 \| 24**（@0x5109F）；RM 层双字段式 version=2/size=0xDA40（0xBDC8D/0xBDC94） |
| nvmlDeviceGetPowerUsage | 0x7BEC0 | 0x2080C637（params 0x17718） | nvoc 在用；RM 调用点 0xF9FD2 |
| nvmlDeviceGetFanSpeed | 0x43E50 | 0x2080852F（cooler 表，13 dwords/风扇，16.16 定点×100） | nvoc 在用；`mov ecx,0x2080852F` @0xB628D |
| nvmlDeviceSetPowerManagementLimit | 0x7D130 | — | 470/610 均有 |

470 vs 610 导出存在性（.dynsym 实测）：`SetGpcClkVfOffset`、`GetClockOffsets`、`GetNumFans` **仅 610**（470 上 nvoc 的 GetNumFans 遥测不可用）；`Set/GetPowerManagementLimit`、`GetPowerUsage`、`GetFanSpeed`、`GetEnforcedPowerLimit` 两代都有；`nvmlDeviceGetPerformanceStates` **两代都没有**（PowerState 读取须走 NVAPI 侧，与 nvoc 的 NVAPI-pstate 主路径一致）；`nvmlRetry_NvRmControl` 仅 470。

## 8. 复核指引

- 表解析：`python reverse/qi_table.py <so/dll> --va <表VA>`（种子已修正为 0x0150E828/0xD22BDD7E/0x33C7358C/0x593E8644）。
- JSON：`reverse/woa-qi-tables/linux610-libnvidia-api.json`（590 项）、`win610-x64-impl.json`、`win610-arm64-impl.json`（2345 项）、`nvid-rs-reconcile.json`、`linux-nvml-exports.json`。
