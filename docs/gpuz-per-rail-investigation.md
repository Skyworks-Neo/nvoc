# GPU-Z 分路功耗/电压来源调查（归档）

> 调查结论：**已结案**。GPU-Z 的 Board/Chip/MVDDC/PWR_SRC/16-Pin 分路瓦特来自
> **GPU-Z 自带的 WinRing0 类内核驱动做直接 PCI 配置空间 / GPU BAR MMIO 读取**，
> 完全游离于 NVAPI + NVML 软件栈之外。nvoc（基于 NVAPI+NVML）无法在不引入签名
> 内核驱动的前提下复刻这条路径。
>
> 本文档归档：完整证据链、为何 nvoc 拿不到、直读 MMIO 的脆弱性与 GPU-Z 更新策略
> 之谜、3 个未知 NVAPI ID 的调用模式与封装价值评估。

---

## 1. 一句话结论

| 问题 | 答案 |
|---|---|
| GPU-Z 的分路瓦特从哪来？ | 自带 WinRing0 内核驱动，直接读 GPU 板上 INA3221 类电流采样芯片的寄存器（PCI 配置空间 / BAR MMIO） |
| 经过 NVAPI 吗？ | **否**。procexp 不加载 nvml.dll；nvapi.dll 仅动态加载用于非功耗 API |
| nvoc 能复刻吗？ | 不能。需要签名内核驱动 + 每款 GPU 的寄存器地址表，超出 NVAPI+NVML 抽象层 |
| nvoc 当前最佳替代？ | NVML `power_draw_w`（总板功耗），见 `power-w-nvml-fix.md` |

---

## 2. 完整证据链（静态 RE，`upx -d` 脱壳后 IDA）

二进制：`GPU-Z_unpacked.exe`（46MB，`upx -d GPU-Z.exe -o GPU-Z_unpacked.exe`）。

### 2.1 DeviceIoControl + IOCTL `0x800064A0` = WinRing0 PCI 读
- KERNEL32 `DeviceIoControl` **确实被导入**（IAT `0x6df520`）。早期 `bp kernel32!DeviceIoControl`
  "0 hits" 是 **WoW64 `.effmach` 延迟断点假象**，不是没调用——运行期
  `ntdll!NtDeviceIoControlFile` 以 IoControlCode `0x800064A0` **持续命中**。
- IOCTL 编码（`0x800064A0`）：DeviceType=`0x22`（FILE_DEVICE_UNKNOWN），Func=`0x192`/`0x193`
  （读/写），METHOD_BUFFERED —— **正是 WinRing0 的 PCI 配置空间读/写码**
  （`0x800064A0` 读，`0x800064A4` 写）。
- 封装在 `sub_5EF960`：`DeviceIoControl(handle, 0x800064A0, &InBuffer{bus,dev,fn,reg}, 8, &OutBuffer, 4, ...)`
  前后用互斥量 **`Global\Access_PCI`**（WinRing0 标志性互斥量名）。

### 2.2 驱动安装链
- ADVAPI32 导入：`CreateService` / `StartService` / `DeleteService` / `OpenSCManagerW`
  → 运行时安装并启动自己的内核驱动服务（经典 WinRing0 手法：`\\.\WinRing0_1_X_X` 类设备）。
- `sub_5EDAE0`（驱动打开）：设备路径（`CreateFileA` 的参数）、`LoadLibraryA` 的 DLL 名、
  `GetProcAddress` 的符号名**全部 XOR 加密**，运行时用
  `*v ^= len * (idx - stackoff) + 41` 解密 —— GPU-Z 故意隐藏这条内核驱动路径。

### 2.3 传感器寄存器表也是加密的
- INA3221 类电流/电压采样芯片的寄存器地址、各路标签（Board/Chip/MVDDC/PWR_SRC/16-Pin）
  **不以明文字符串出现**（加密的硬件数据库），按 PCI ID 分发不同寄存器映射。

### 2.4 负面证据（这些全都被上面的发现解释）
| 现象 | 原因 |
|---|---|
| procexp 看不到 nvml.dll | 根本不用 NVML |
| nvapi QueryInterface 刷新期不触发 | 分路不来自 nvapi；handlers 启动期已缓存 |
| 96 个已知 NVAPI 功耗/电压 ID 全测空 | 根本不是 NVAPI 来源（见 `per-rail-not-on-laptop-confirmed.md`） |
| 3 个未知 ID 是 VF 曲线/电压域描述符，非瓦特 | 与功耗无关（见 §5） |
| `bp kernel32!DeviceIoControl` deferred | WoW64 `.effmach` 位数不匹配，断点从未 arm |

---

## 3. 为何 nvoc 拿不到（技术分层）

```
应用层   GPU-Z.exe  ──┐
                     │ 自带 WinRing0 驱动 (签名/测试签名)
内核层   WinRing0.sys ─┼──► 直接 PCI 配置空间 R/W / GPU BAR MMIO R/W
                     │     (IOCTL 0x800064A0, mutex Global\Access_PCI)
硬件层   GPU 板上 INA3221 类电流采样芯片 ◄── 寄存器地址随 GPU 型号不同
                     │
─────────────────────┼─────────────────────────────────────────
                     ╳ nvoc 的世界在这里之下，够不到
应用层   nvoc-cli    ──► NVAPI (nvapi64_impl.dll) + NVML ──► \\.\NvDevice
                                                     │
                                          只暴露"总板功耗 power.draw"
                                          (NVML power_usage / NVAPI 拓扑)
```

NVAPI/NVML 是 NVIDIA 官方提供的**抽象层**，有意只暴露聚合值（总功耗、总电压、
温度、频率），不暴露分路瓦特。分路瓦特是板载硬件芯片的原始采样，必须由内核驱动
直读 PCI/MMIO——这正是 LibreHardwareMonitor / OpenHardwareMonitor 这类项目维护
自己的驱动 + 每型号寄存器表的原因。仓库根的 `LibreHardwareMonitor-master/` 即是
该路线的开源实现。

**nvoc 若要分路瓦特，最小代价：**
1. 一个签名内核驱动（WinRing0 类；正式签名昂贵，或测试签名模式 + bcdedit）；
2. 直接 PCI/MMIO 读写代码；
3. 每款目标 GPU 的 INA3221 寄存器映射表（GPU-Z 维护且加密了它）。

这是一个独立的内核驱动项目，**超出 nvoc 当前范围**。结论：nvoc 的 NVML
`power_draw_w`（总板功耗）是 NVAPI+NVML 抽象层内能拿到的最好结果。

---

## 4. 直读 MMIO 为何"脏"——以及 GPU-Z 更新策略之谜

### 4.1 你说得对：直读 MMIO 的脆弱性
NVAPI/NVML 是**契约化抽象**：NVIDIA 在驱动更新时会维持接口一致性
（结构体版本号向前兼容、语义文档化、跨型号统一）。而直读 MMIO/PCI 配置空间是
**绕过抽象、直接戳硬件**，其脆弱性有三层：

1. **每 GPU PCI ID 一张写死的寄存器表**：不同 GPU（甚至同代不同 PCB/供电设计）
   的 INA3221 / INA3221 兼容芯片挂在不同 I²C 地址、不同寄存器偏移、不同分流电阻
   值。GPU-Z / LibreHardwareMonitor 维护着庞大的"PCI ID → 寄存器地址 + 分流系数"
   硬件数据库。换一张它不认识的卡，那一路就是 0 或乱码。
2. **驱动版本可能改动寄存器映射/初始化**：极少，但 NVIDIA 驱动理论上可在初始化时
   重配 INA3221 的通道/采样配置，使旧寄存器偏移失效。
3. **供电拓扑硬件层变化**：同型号 GPU 在笔记本/桌面/AIC 非公版上供电设计不同，
   表单需按"板 ID"而非仅"GPU ID"细分。

### 4.2 那为什么 GPU-Z 更新不像驱动那样频繁，且旧版经常完全能用？

这正是 MMIO 直读的**反直觉优势**，关键在于**它绕过了驱动**：

- **读的是硬件芯片，不是驱动数据结构**。GPU-Z 通过内核驱动读的是 INA3221 芯片
  寄存器里的**电流/电压采样值**，这些寄存器是**硅片固定的**（由芯片 datasheet
  决定），与 NVIDIA 驱动版本**无关**。NVIDIA 驱动更新几乎从不重配这些芯片的
  寄存器映射——因为那是板级硬件初始化（VBIOS/EC 固件做的），驱动只是用户态的
  旁观者。**所以驱动一更新就把直读全废掉的风险其实很低**。
- **真正驱动 GPU-Z 更新的，是新 GPU 发布**，不是驱动版本。新卡带来新的 PCI ID、
  新的供电芯片型号、新的寄存器地址——这才是 GPU-Z 更新硬件数据库的触发条件。
  所以 GPU-Z 的发布节奏跟 **GPU 产品周期**（约半年一代）走，而非驱动周期
  （月度 Game Ready）。旧版 GPU-Z 在老卡上完全能用，因为老卡的 INA3221 寄存器
  表从该卡发布起就冻结了，永不过期。
- **封装代价的转移**：NVIDIA 把"跨型号兼容"的维护成本内化进了 NVAPI/NVML（用户
  看不到）；GPU-Z 把同样成本外化成了**它自己的硬件数据库维护**。所以 GPU-Z 的
  更新本质是在补这张表，而不是在追 NVIDIA 驱动。

**对比 nvoc：** nvoc 走 NVAPI/NVML，零硬件数据库维护成本，代价是拿不到分路瓦特。
这是抽象层的经典取舍——**通用性 vs. 深度**。GPU-Z 选了深度（直读硬件），nvoc 选了
通用性（官方抽象层）。两条路都对，看项目目标。

> 一句话：**直读 MMIO 不脆弱在"驱动更新"，而脆弱在"换卡/换板"——而换卡频率远低于
> 驱动更新频率，所以 GPU-Z 的更新周期比驱动慢、且旧版常能用。**

---

## 5. 3 个未知 NVAPI ID 的调用模式与封装价值

### 5.1 候选 ID `0x7457cab5`（计划主角）—— **已实测排除**

详见 `docs/gpuz-nvapi-runtime-windbg.md` §3 与 `gpuz-power-rails-id-7457cab5.md`：
- handler `nvapi64_impl!0x180238CC0`，结构体版本魔数 `65608`（v1, sz72），与已知
  功耗拓扑 handler `0x180235F10`（`0xedcf624e`）**同族**（同 `0x60B30` RM 缓冲、
  同 RM 分发器 `sub_180389320`），但 RM 子命令是 `0x20882CF9`（功耗是 `0x20880B33`）。
- **实测不是实时读**：重复读取 + 负载变化下，返回的 32 字节**完全不变**；
  非管理员返回 `NVAPI_INVALID_USER_PRIVILEGE`。
- 结论：**确定性、特权、非实时的 blob**（capability/key/descriptor），**不是**
  GPU-Z 的分路实时读数。教训——**结构相似 ≠ 语义相同**。

### 5.2 三个未知 ID：`0x0bc8163d` / `0xc12eb19e` / `0xf40238ef`

这三个 ID 是 GPU-Z 启动期解析、但**刷新期不再触发**的 QueryInterface ID。经 IDA
逐 handler 反编译（见 §5.3），它们的真实身份与早先的猜测（VF 曲线/电压域）**不同**：

| ID | handler | 真实类别 | 是否实时读 |
|---|---|---|---|
| `0x0bc8163d` | `0x1801E0BC0` | **热通道状态**（`ThermChannelGetStatus`） | 否（拓扑快照） |
| `0xc12eb19e` | `0x180258170` | **富分路功耗描述符**（power RMCTRLS 族，同 GetPowerTopology） | 否（描述符表） |
| `0xf40238ef` | `0x18024D4E0` | **未实现 stub**，恒返回 `-104` | N/A（死路） |

**三者都不是 GPU-Z 的分路实时瓦特来源。** 调用模式共性：
- 一次性表/能力查询（**无** `0x7457cab5` 那种 100ms×2 重试轮询循环）；
- 调用者传入带版本魔数的结构体，handler 写回**拓扑/描述数据**（通道数 + 每通道
  类型/能力），而非随负载跳动的实时采样；
- 刷新期不触发（GPU-Z 只在启动期解析一次），证明与分路瓦特无关。

> 重要纠偏：早先记忆把这三者记为"VF 曲线/电压域描述符"。IDA 精确分析表明其中
> `0x0bc8163d` 是**热通道**、`0xc12eb19e` 是**功耗分路描述符**（power 族）、
> `0xf40238ef` 是 **stub**。均非电压域/VF 曲线。

### 5.3 精确 handler 分析（IDA session `79346bbf`，逐 ID）

> 分发表 `off_1804DE000` 实测为 **12 字节条目** `[4B id][4B pad][4B handler ptr]`。
> 校准基准：已知功耗拓扑 handler 使用 RM ioctl `0x07000046`、escape `0x2080A613`、
> 版本魔数 `65608`（v1, sz72）。

#### `0x0bc8163d` → handler `0x1801E0BC0` — **热通道状态**
- **身份铁证**：辅助函数 `sub_1801D81C0` 内嵌错误串
  **`"NvAPI_GPU_ThermChannelGetStatus received version..."`**（`0x180485cf0`）；
  RTTI `_NV_ESC_NVAPI_GPU_THERMAL_RMCTRLS`。
- **原型**：`__int64 __fastcall(int gpuHandle, __int64 structPtr)`（2 参）。
- **输入**：结构体首 DWORD = 版本魔数。接受值 `65596`(v1,sz60) / `131240`(v2,sz168) /
  `210120`(v3,sz13512)；不匹配返回 `-9`。
- **RM**：分配 `0xC9E0`(51680)B 缓冲；dispatcher `sub_180389320(117440911,…)` →
  RM ioctl **`0x0700018F`**（热控 ioctl，**非** `0x07000046`）；escape 写入
  `buf[13] = 0x2080853B`。
- **输出**：遍历 `i=0..0xFE`（最多 255 个热通道），对每个置位的 bit 写入 per-channel
  记录 `v22=&v8[35*i]`：`v22[18]`(+72)=控制器类型（经 `sub_1801DA7E0` 映射 1..5）；
  type==3 时额外写 `v22[28]`(+112)、+180 处 1 字节、+116..+151 处 10 个 dword。
- **分类**：一次性拓扑/状态快照（无 `Sleep`）。

#### `0xc12eb19e` → handler `0x180258170` — **富分路功耗描述符**（power RMCTRLS 族）
- **身份**：RTTI `qword_180504AC0 → _NV_ESC_NVAPI_GPU_POWER_RMCTRLS`（与功耗拓扑、
  实验性 power-rails 同族）。
- **原型**：`__int64 __fastcall(uint gpuHandle, _DWORD *structPtr)`（2 参）。
- **输入**：接受版本魔数 `65928`/`66972`/`69408`/`74968`（大型多轨功耗结构体，392B→
  ~9432B 多版本），外加内部 `336752`；不匹配 `-9`。
- **RM**：分配**两个** `0x60B30`(396080)B 缓冲（典型双缓冲）+ 一个 `0x12370`(74608)B
  调用方缓冲 `v6`。
  - 第一段 escape（probe/GetInfo）：`sub_1803894A0(117440582,…)` → ioctl `0x07000046`，
    escape `0x2080A612`。
  - 第二段 escape（实读）：`sub_180389320(117440582,…)` → ioctl `0x07000046`，escape
    **`0x2080A613`**（**与 GetPowerTopology 同一 escape**）。
- **输出**：两段解包——先从 `v3[126..254]` 抽 128B（8 个 OWORD）按轨散布到 `v6`；
  再遍历 `j=0..0xFE`（255 轨），对每个置位 bit 从 `v10[54…]` 拷一条 **150-dword(600B)
  per-rail 记录**进 `v6`（含 +88/+96/+104/+120 处 QWORD，type 4/5/7 经
  `sub_18022DC20`/`sub_18022DB50` 写 19B 子记录），最后 `v6[2]=v10[23]`。
- **分类**：**描述符/能力表**（无 `Sleep`、无重试）——枚举"存在哪些轨/类型/能力"，
  **不**返回随负载跳动的瓦特。是 DLL 里最富的分路功耗描述符，但本质是拓扑/能力。

#### `0xf40238ef` → handler `0x18024D4E0` — **未实现 stub**
- **原型**：`__int64 (void)`（**无参数**）。
- **函数体**：仅 `if (loglevel>=4){log(608);log(609);} return 4294967192;`（= `-104`
  `NVAPI_NVIDIA_DEVICE_NOT_FOUND`）。
- 无 RM、无 escape、无版本校验、无输出。**死路。**

#### 旁证：唯一带"实时轮询特征"的功耗族 handler 仍是 `0x7457cab5`
功耗 RMCTRLS 族里**唯一**带 **2× `Sleep(100ms)` 重试循环** + escape `0x2080E639`
+ 版本魔数 `65608` 的 handler 就是 `0x180238CC0`（ID `0x7457cab5`）。即"实时读"
特征点单独指向它——但已实测它是确定性非实时 blob（见 §5.1）。**结论：功耗族内
没有任何 handler 在本机返回随负载变化的分路瓦特。**

### 5.4 封装价值评估

**对"分路瓦特"目标：零价值。** 这 3 个 ID（连同已排除的 `0x7457cab5`）都不是
实时瓦特来源，封装它们对补齐 GPU-Z 那几路分路功耗毫无帮助——分路瓦特根本不在
NVAPI 抽象层里（见 §3）。

**对"丰富 nvoc 只读监控维度"的逐 ID 价值：**

| ID | 封装价值 | 理由 |
|---|---|---|
| `0x0bc8163d`（热通道） | **低** | 与已封装的 `NvAPI_GPU_GetThermalSettings` 重复；多 255 通道拓扑对 nvoc 用例冗余 |
| `0xc12eb19e`（功耗分路描述符） | **中（仅作伴随）** | 单独只返回"存在哪些轨/类型"的能力表，不含实时值；仅当将来真要驱动某实时读时，可先用它枚举拓扑。当前无实时读可配，单独封装无意义 |
| `0xf40238ef`（stub） | **零** | 恒返回 `-104`，死路 |
| `0x7457cab5`（已排除） | **零** | 已实测为确定性非实时 blob |

共同的拦路问题：
- 语义需 WinDbg 动态确认（静态只能定布局，定不了语义）；
- 多数笔记本 GPU 上这些接口返回空/NotSupported（见
  `per-rail-not-on-laptop-confirmed.md`），实际可用面窄；
- `0xc12eb19e` 的版本魔数族（`65928`/`66972`/`69408`/`74968`）对应非常大的结构体
  （最大 ~9432B + 74608B 工作缓冲），封装成本高、收益不明。

**结论：不建议现在封装任何一个。** 封装的正确触发条件是"动态确认某 ID 在 nvoc
目标 GPU 上返回有价值的非空数据"。当前没有任何未知 ID 满足该条件——`0x7457cab5`
已实测非实时，其余三者要么是表/描述符、要么是 stub。维持 `nvid.rs` 现状
（`0x7457cab5` 已挂 `Unknown_` 并标"Do not wrap"，其余三者完全未注册）即可。

---

## 6. 相关产物与文件

| 文件 | 内容 |
|---|---|
| `GPU-Z_unpacked.exe` | 脱壳后的 GPU-Z（IDA 分析对象，46MB，未提交） |
| `docs/gpuz-nvapi-runtime-windbg.md` | WinDbg 动态定位 ID 的完整流程 + `0x7457cab5` 排除记录 |
| `nvapi64_impl_qi_table.txt` / `unknown_ids.txt` | nvapi64_impl 的 2337 条 QI 分发表静态导出 |
| `gpu_control_rm_unknown_ids.txt` | 25 个"与功耗 handler 同族"的候选 ID |
| `docs/gpuz-power-source-frida.js` | Frida 双相 ID 捕获脚本（WinRing0 发现后已作废，保留作参考） |
| `LibreHardwareMonitor-master/` | 同路线开源实现（含 INA3221 寄存器读取代码，可借鉴寄存器映射） |

### 建议加入 .gitignore（未提交的本地分析产物）
```
GPU-Z.exe
GPU-Z_unpacked.exe
*.id0 *.id1 *.id2 *.nam *.til *.i64
nvapi.dll nvapi64.dll nvapi64_impl.dll nvapi_impl.dll nvppe.dll
nvapi.dll.id0
subcommand_clusters.txt gpu_control_rm_unknown_ids.txt unknown_ids.txt nvapi64_impl_qi_table.txt
```

---

## 7. 相关记忆（`~/.claude/.../memory/`）

- `gpuz-per-rail-is-winring0-pci.md` — 本结论的决定性记录
- `gpuz-watts-not-in-known-ids.md` — 96 已知 ID 全测空（已由此文解释）
- `gpuz-power-rails-id-7457cab5.md` — 候选 ID 实测排除（确定性 blob）
- `per-rail-not-on-laptop-confirmed.md` — 本机 NVAPI 分路接口全空
- `gpuz-nvapi-power-channels.md` — GPU-Z 展示的更丰富分路通道
- `power-w-nvml-fix.md` — nvoc 总功耗改走 NVML 的修复
