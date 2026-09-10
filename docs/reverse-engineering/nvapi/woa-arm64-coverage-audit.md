# WOA (Windows on ARM) NVAPI 体系审计 —— 616.00 Developer Preview

- 日期：2026-09-09/10
- 样本：`reverse/616.00_DeveloperPreview_win11_arm64_International/Display.Driver/`（未安装，静态解包）
- 工具：idalib MCP + `reverse/qi_table.py` / `reverse/pe_probe.py`；表 JSON 在 `reverse/woa-qi-tables/`
- 关联：`linux-version-coverage-audit.md`、`reverse/woa-nvoc-fix-proposal.{patch,md}`

## 1. 三架构 × shim/impl 双层封装

| 文件 | PE machine | 大小 | 角色 | ARM64X 标记 |
|---|---|---|---|---|
| nvapi.dll | x86 | 443 KB | shim | — |
| nvapi_impl.dll | x86 | 5.1 MB | impl | — |
| nvapi64.dll | x64 | 817 KB | shim（双视图） | `.a64xrm` + `#__os_ar`×2 + `.hexpthk` |
| nvapi64_impl.dll | x64 | 6.0 MB | impl（双视图） | `.a64xrm` + `.hexpthk` |
| nvapia64.dll | ARM64 | 632 KB | shim（纯） | — |
| nvapia64_impl.dll | ARM64 | 5.9 MB | impl（纯） | — |
| nvml.dll | ARM64 | 4.4 MB | NVML 本体（纯） | — |
| nvml_arm64ec.dll | x64(ARM64EC) | 2.4 MB | NVML x64/EC 视图 | `.a64xrm` + `.hexpthk`，导出名仍是 `nvml.dll` |
| nvml_loader.dll | ARM64 | 5.5 MB | NVML 定位器 | `.a64xrm`，430 导出（少 1） |

三个 shim 都只导出 `nvapi_QueryInterface` + `nvapi_Direct_GetMethod`，仅导入 KERNEL32 → impl 为运行时解析。

## 2. ARM64X 关键事实：静态基视图是 ARM64

- nvapi64.dll 导出 `nvapi_QueryInterface` @0x1800A5010 是 0xE 字节 x64 thunk（`mov rax,rsp; mov [rax+20h],rbx; push rbp; pop rbp; jmp rel32`）→ **0x180011B20 原始字节是 ARM64 代码**（`stp x19,x28,[sp,#-0x20]!` / `mov w19,w0`）。
- nvapi64_impl.dll 同构：导出 @0x1804B3010 → thunk → **0x18014BE70 原始字节为 ARM64**。
- 结论：这些 ARM64X 文件的**原始字节 = ARM64 视图，x64 视图由 loader 依 `.a64xrm` 在加载期合成**（与 MS 文档"x64 为基底"的直觉相反，NVIDIA 把 ARM64 当基底）。IDA 直接开原始文件会得到嵌合体，分析请用纯 ARM64 的 `nvapia64*.dll`。

## 3. shim → impl 转发机制（nvapia64.dll 逆向）

- `nvapi_QueryInterface` @0x18000EDD8：临界区内扫 shim 本地表（`{fn@0, id@8}` 16B 步长，fn==0 终止）→ 未命中调 `sub_18000ED30` 加载 impl → `GetProcAddress(impl, "nvapi_QueryInterface")` 存入 `off_1800910B0` 兜底转发（`nvapi_Direct_GetMethod` 同理 → 0x1800910B8）。
- shim 本地表仅 **4 项**（x64 视图 @raw 0xB7E30 布局反转 `{id@0, fn@8}`；ARM64 视图 @0x8EE30）：
  `0x0150E828`(Initialize)、`0xAD298D3F`(GPU_PrivateLifecycleInit)、`0xD22BDD7E`(Unload)、`0xD7C61344`(InternalUnload)。
- 无 `_impl` 字面字符串：impl 路径由自身模块名运行时拼出，且 nvapi64.dll 内含 `SetupGetInfDriverStoreLocationW` → **impl 实际从 DriverStore 解析**（INF 只把 `*_impl.dll` 落 DriverStore）。

## 4. impl 主表（2345 项）与门控

- nvapia64_impl：QI @0x180125550，主表 VA **0x180537000**，2345 项（唯一 2343；重复 2 项 0xD3B24D2D、0xCCFFFC10 为真实属性，两视图同位重复）。
- nvapi64_impl（ARM64X）：原始数据含**两份 2345 项表**——VA 0x180569000 与 0x1805737E0，ID 序列逐项相同，2336/2345 fn 指针不同 → 分属 x64/EC 视图与 ARM64 视图；x64 视图 fn 由加载期重定位合成。
- **x64 vs ARM64 覆盖差集 = 0**（同源编译，双视图对外暴露同一接口面）。
- QI 门控（Windows 特有，Linux 无）：
  - 全局标志 @0x180594D00 bit3==0 时，两个 ID 直接返回 NULL：**0x33C7358C、0x593E8644**（注意 0x33C735CC 是笔误；两者都真实在表中，即"已实现但被门控"）。
  - 注册表 RID 门控 `L"EnableRID74954"` → 旁路派发路径（sub_180122928 + sub_1801256B8）。
  - 未命中 → trace 日志（sub_180046A38, event 21）→ 返回 0。

## 5. INF 安装映射（nv_surface_woa.inf，10 个 Section 全 arm64-only）

- **System32**（dirid 11，`[nv_system32_copyfiles]` 2080-2100）：`nvapi64.dll`(ARM64X, 行 2086)、`nvapia64.dll`(行 2087)、**`nvml.dll,nvml_loader.dll,,0x00004000`（行 2096 —— System32 的 nvml.dll 是改名的 nvml_loader.dll，纯 ARM64！）**。
- **DriverStore**（dirid 13）：全部 `*_impl.dll`（1817-1819 等）+ `nvml_arm64ec.dll`（1855/1953/2053-2054）——**不落 System32、不改名**。
- **SysWOW64**（2114-2132）：x86 `nvapi.dll`（2118/2128）。
- **未证明**存在任何让 x64 进程以标准名 `nvml.dll` 拿到可加载 NVML 的机制。

## 6. nvoc-cli WOA 故障矩阵与根因

枚举链路：`get-gpu-list` → `discover_targets`（cli/src/lib.rs:2528-2531）→ **NVAPI 先行**（core/src/target.rs:241-289），NVML 为 best-effort/增强；NVML 解析链 core/src/dll_path.rs:37-98（env → 默认搜索 → System32\nvml.dll → NVSMI）。

| nvoc 构建 | nvapi-rs 加载 | NVML（nvml_wrapper / libloading） | 判定 |
|---|---|---|---|
| x86 | SysWOW64 `nvapi.dll` ✅（GPU-Z 同款路径） | WOA 无 32 位 nvml → **os error 126** | 枚举可用，NVML 字段缺 |
| **x64（默认构建）** | `nvapi64.dll` ARM64X x64 视图理论可载；impl 从 DriverStore 解析（模拟进程下未证）；任何 LoadLibrary 失败被 nvapi-rs 掩盖为 `LibraryNotFound`（sys/src/nvapi.rs:81-83） | System32\nvml.dll=纯 ARM64 → **必现 os error 193 BAD_EXE_FORMAT**；候选全败 | NVML 必坏；NVAPI 面若也坏则整体硬错误 |
| **aarch64-msvc（实测炸）** | `nvapi64.dll` ARM64X ARM64 视图加载成功（库名没错），但 impl 解析失败时 shim 兜底指针为 NULL → **所有非本地 QI 一律返回 0 → nvapi-rs 优雅报 NoImplementation** | nvml_loader.dll 本职即定位 DriverStore 真 nvml → ✅（实测"WOA 上 nvml 能用"吻合） | **实际故障形态**：枚举优雅失败，非段错误 |

### arm64 原生实测故障的机制链（2026-07-20 群聊现象回溯）

实测现象（第三方 Dolphin 工具 + nvoc-cli arm64）：`EnumPhysicalGPUs` 优雅失败、nvml 正常、疑似"offset 全变了"。

- **静态排除 ID 缺失**：0xE5AC921F（EnumPhysicalGPUs）、0x264C5763（EnumPhysicalGPUsInternal）在 WOA ARM64 表（2343 唯一 ID）中都在——不是"接口没有"。
- **真正机制**：shim 依赖 **DriverStore 解析 impl**（两个 shim 都内嵌 `SetupGetInfDriverStoreLocationW` 串 + `GetModuleFileNameW`/wstring 拼接，impl 路径由自身模块名运行时构造；`*_impl.dll` 只落 DriverStore）。任何一环失败（ARM64X 视图合成、DriverStore 查询、包内 impl 缺失）→ 转发指针 NULL → 全部 QI 走 NULL 出口——表象是"接口全没了"，代码从未执行，所以**必然优雅报错而非段错误**。
- **"未公开的 offset 全变了"的真相**：nvapi64.dll 的原始字节=ARM64 视图（§2），按 x64 基址/模式做 RE 或热补丁看到的一切 offset 自然全变——这不是驱动私改，是 ARM64X 的本相。
- **与 Linux ARM64 为何无此问题**：Linux 每架构独立包、ELF 无双视图机制，`libnvidia-api.so.1` 的 ID 表是源级数据（x64 实测 590 项；aarch64 包应为同源同表）——每个组件都是原生单视图，不存在 ARM64X/DriverStore 这类加载期合成环节。

### arm64 故障定位清单（上机排查用）

1. `LoadLibraryW("nvapi64.dll")` 是否成功 → 再 `LoadLibraryW("nvapia64.dll")`（纯 ARM64，System32 行 2087）对照。
2. QI(0xE5AC921F) 返回 NULL 则 impl 解析失败：查该机 DriverStore 是否含 `nvapi64_impl.dll`/`nvapia64_impl.dll`（`pnputil /enum-drivers` 或 DriverStore FilePaths）。
3. nvapi-rs 建议诊断模式透传 GetLastError（现状 126/193/其他全被折叠成 `LibraryNotFound`，sys/src/nvapi.rs:81-83）。

## 6b. 根因排序（综合 x64 与 arm64 实测）

1. **arm64 原生**：nvapi64 ARM64X 视图 shim 加载成功但 impl（DriverStore）解析失败 → 全量 QI 返回 NULL → 优雅 NoImplementation（与 2026-07-20 实测完全吻合，静态表已排除 ID 缺失）。
2. **x64 转译**：NVML init 必报 os error 193（INF 行 2096 改名 + dll_path.rs:38 候选全是 ARM64 DLL，确定性）；NVAPI 侧视 x64 视图合成情况待上机验证。
3. **x86**：NVML 126（仅字段缺失，枚举不坏）。
4. 通用：nvapi-rs 把所有加载失败折叠成 `LibraryNotFound`，掩盖真实错误码（126/193 无法区分）——建议透传 GetLastError（sys/src/nvapi.rs:81-83、92-93）。

- GPU-Z 正常的原因：x86 进程走 SysWOW64 的 x86 `nvapi.dll`，绕开全部 ARM64X/loader 复杂度。

## 7. 修复补丁（2026-09-10 已应用，待 WOA 实机验证）

- `reverse/woa-nvoc-fix-proposal.patch`：nvapi-rs aarch64 → `nvapia64.dll`；core/src/dll_path.rs NVML 候选新增 DriverStore `nvml_arm64ec.dll` 探测 + WOA 布局文档 + 测试同步。**已 git apply 落地**（core 主仓 + nvapi-rs 子模块各一处修改，未提交）。
- **GetLastError 完全透传（同日实现）**：sys 层新增 `LoadError{call, os_code, message}` 侧信道（`sys::nvapi::last_load_error()`），Windows 捕获 `GetLastError`（193/126/5 各归其位，`io::Error` 带系统消息文本）、Linux 捕获 `dlerror`；safe 层 `NvapiError` Display 在 `LibraryNotFound` 时追加该真实错误；nvoc `ensure_nvapi_initialized` 改用 Display 输出（原 `{e:?}` 的 Debug 派生会把它丢掉）。
- `git apply --check` 原始校验 + 应用后 rustfmt/clippy（主仓 workspace -D warnings）/nvapi-rs 与 nvoc-core 测试全过；子模块 `sys/src/gpu/clock.rs` 存在**既有** clippy lint（vfcurve 工作线，与本补丁无关）。
- 改动↔矩阵格映射与 4 项实机验证项见 `reverse/woa-nvoc-fix-proposal.md`。

## 8. 复核指引

- 表 JSON：`reverse/woa-qi-tables/{win610-arm64-impl,win610-x64-impl,win610-shims,nvid-rs-reconcile,linux-nvml-exports}.json`。
- nvid.rs 注册 2188 ID 中 103 个在 WOA 双表缺失（必然 NoImplementation，主要是 D3D12 shading-rate/DISP/DRS/DIAG），清单见 nvid-rs-reconcile.json。
