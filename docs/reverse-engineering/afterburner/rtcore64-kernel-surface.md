# RTCore64.sys 内核面全解 —— AB 的 ring0 驱动是什么、能干什么、对宽表抬窗意味着什么

- 日期：2026-10-19
- 样本：`reverse/afterburner2/RTCore64.sys`（40688B，2024-05-13 构建，sha256
  a4f44e26…，MD5 589fa10a…；两套安装包同 blob；PDB 路径 `0.16385.1\src\rtcoresys\amd64\RTCore64.pdb`）
- 方法：idalib 全量（全二进制仅 53 函数/.text 2KB，DriverEntry + 唯一分发器逐分支解码）
- 关联：[oow-mode-verification.md](oow-mode-verification.md)、
  `../../nvapi/regtab-fifoctx-verdict-591-86.md`（MMIO 伪造后继方向）

## 1. 总裁决

1. **AB 的 ring0 驱动 = RTCore64.sys（RivaTuner 核心驱动，REALiX 签名）**，设备
   `\Device\RTCore64`，SDDL `D:P(A;;GA;;;SY)(A;;GA;;;BA)`（仅 SYSTEM+Administrators）。
2. **AB 不做宽表抬窗，也没有等价物**。它的功率上限走 NVAPI 客户端族
   （ClientPowerPolicies/TgpWatt），oem 数据库只放宽 UI 量程；内核侧功率表从未被
   它触碰。AB 的"越信封"能力在**硬件寄存器级**（GPU BAR0 MMIO 直写），不在内核 RAM。
3. **RTCore 架构上就到不了宽表**：物理映射入口带"仅显卡 BAR + ROM 窗口"白名单门
   （见 §3.0）——内核池/内核镜像物理页不在白名单内。我们的 pmxdrv kmd 车道
   （任意物理读写 → 页表走查 → 功率表抬窗）是**nvoc 独有**的能力，AB 没有、
   RTCore 也不给。
4. 因此"借鉴作为备份"的答案分两半：**宽表抬窗的备份 ❌**（原语不等价）；
   **GPU 寄存器级（MMIO/IO端口/MSR/PCI-cfg）的备份传输 ✅**——恰好是 3060 判决
   文档"PCI MMIO 卡侧寄存器伪造"后继方向的现成签名传输层。

## 2. 驱动骨架

- `DriverEntry`（INIT 段）：`WdmlibIoCreateDeviceSecure(\Device\RTCore64, FILE_DEVICE_UNKNOWN,
  SDDL: SY+BA 全权)` + `\DosDevices\RTCore64` 符号链；IRP_MJ_CREATE/READ/
  DEVICE_CONTROL 全部指向同一分发器 `sub_11438`；`sub_11008` 为 unload。
- 无 IO 端口/MSR 导入（in/out 与 rdmsr/wrmsr 是内核侧内联指令）；HAL 导入仅
  `HalGetBusDataByOffset/HalSetBusDataByOffset/HalTranslateBusAddress`。

## 3. IOCTL 全表（0x80002000 族，输入走 `Parameters.DeviceIoControl.InBuffer`，
长度 = `OutputBufferLength` 字段校验；寄存器访问宽度按码分）

### 3.0 白名单门 `sub_11044(phys, size, bdf*)` —— 本驱动的灵魂

- 白名单一：物理 `0xC0000–0xE0000`（legacy ROM 窗口——读显卡 option ROM 用）直接放行。
- 其余必须命中**指定 PCI 设备的 BAR**：请求自带 bus/dev/func + BAR 索引；驱动经
  `HalGetBusDataByOffset` 读 cfg 0x0B 校验 class==3（显示控制器），再读
  `0x10+4*idx` 的 BAR 原值，要求 `[phys, phys+size) ⊆ [BAR&~0xFF, BAR+0x100_0000)`。
- 结论：**任意物理内存访问不存在**；内核 RAM、其他设备 MMIO、低 1MB 全部拒之门外。
  （这也是它与现代"漏洞驱动"滥用面的本质区别：滥用者要任意 physmem，RTCore 给不了。）

### 3.1 用户态 MMIO（AB `MMIOUserMode=1` 的底座）

| IOCTL | 语义 |
|---|---|
| `0x80002000` | `ZwOpenSection(\Device\PhysicalMemory)` + `ZwMapViewOfSection`（PAGE_NOCACHE|RW|SEC_COMMIT）把 **(busAddress, size) 映进调用进程用户空间**，先过 HalTranslateBusAddress 与白名单门；返回用户 VA。之后读写零系统调用 |
| `0x80002004` | `ZwUnmapViewOfSection(-1, va)` 解除 |

### 3.2 内核跟踪映射（MMIOUserMode=0 的路径）

| IOCTL | 语义 |
|---|---|
| `0x80002040` | `MmMapIoSpace(MmNonCached)` 内核映射，登记 0x100 槽表（va/size），返回槽号 |
| `0x80002048` | 读：槽号校验 + `offset+width ≤ size` 界检 → u8/u16/u32 |
| `0x8000204C` | 写：同上门检 → u8/u16/u32 |
| `0x80002044` | `MmUnmapIoSpace` + 表注销 |

### 3.3 MSR（含黑名单）

| IOCTL | 语义 |
|---|---|
| `0x80002030` | `__readmsr(idx)`，12B 请求/响应 |
| `0x80002034` | `__writemsr(idx, value)` |
| 黑名单 | `0x174–0x176`（SYSENTER CS/ESP/EIP）与 `0xC0000080–0xC0000084`（EFER/STAR/LSTAR/CSTAR/FMASK）拒绝——反 TCB 摧毁但**不防 Ring0 提权原语滥用**（如 IA32_DEBUGCTL 未在名单内） |

### 3.4 IO 端口

| IOCTL | 语义 |
|---|---|
| `0x80002008/0C/10` | `__in{byte,word,dword}(port)` |
| `0x80002014/18/1C` | `__out{byte,word,dword}(port, val)`；唯一防护：当 `__indword(0xCF8)` 使能位为 1 且区间落 `[0x10,0x27]` 时拒绝写 `0xCFC`（防经 PCI 数据口改 BAR/配置头） |

### 3.5 PCI 配置空间

| IOCTL | 语义 |
|---|---|
| `0x80002050` | `HalGetBusDataByOffset(PCIConfiguration, bdf…)`，宽度 1–4B |
| `0x80002054` | `HalSetBusDataByOffset`，**offset 0x10–0x27（BAR 区）禁写** |

### 3.6 杂项

| IOCTL | 语义 |
|---|---|
| `0x8000202C` | 物理基址累加器（`0x80000000` = 沿用内部基址 + delta；配 ROM 窗口换算用） |
| `0x80002028` | 版本查询（返回 `{1, 8}`） |

## 4. RTHAL.dll 消费面（AB 实际用 RTCore 干什么）

RTCore 导入仅三条链，全部落在硬件抽象层，**与功率/时钟偏移 NVAPI 路径零交集**：

- `?GetHwShaderClock@CHALImplementation` → RTCore `GetShaderClk`：BAR0 MMIO 读 PLL
  测 shader 时钟（监控面）。
- `CHAL::WriteRegisterUlong`（虚表派发 `0x100878e8`）→ RTCore `WriteRegisterUlong/
  WriteRegisterUshort`：BAR0 寄存器写（legacy 风扇/电压等寄存器级控制）。
- `CHALVRMImplementation*`（24 款 VRM 芯片直写）走 NVAPI I2C 通道（0x283AC65A/
  0x4D7B0709），不经 RTCore。
- 配置对齐：`MMIOUserMode/MMIOPageSize=4(KB)/PCIAccessMode/PCIBusFilter=-10h`。

## 5. 与 nvoc kmd/pmxdrv 车道的对照

| 维度 | RTCore64.sys | 我们的 pmxdrv 车道（`core/src/kmd/`） |
|---|---|---|
| 物理映射 | 有，但 **BAR0+ROM 白名单门** | **任意物理**（MAP_PHYS 0x222AB8 / UNMAP 0x222ABC，无范围门） |
| 内核表抬窗 | 架构上不可能 | 宽表抬窗主路径（`set_power_wall_kmd`：register→chain walk→arm select→lease→verify） |
| 页表走查 | 不涉及 | `pagewalk.rs`（low-stub 寻根 + 四级走查 + 磁盘 PE 头比对） |
| IO 端口 | r/w b/w/d + 0xCFC 防护 | pmxdrv 有 PORT_IO 0x222864 **未接线** |
| PCI cfg | r/w + BAR 区禁写 | pmxdrv 有 PCI_CFG 0x222868 **未接线** |
| MSR | 读/写 + 8 项黑名单 | pmxdrv 家族未编目（0x222A80 起的 22 码族待清点） |
| 内核映射跟踪表 | 0x100 槽 + 界检 | 不需要（用户态映射后直接访问） |

## 6. "未涉及的内核态操作"差集（我们可补的清单）

1. **MSR 读写通道**（pmxdrv 22 码族里可能已有对应码——编目时按 RTCore 的黑名单
   语义对照）；用途：CPU/平台侧调试，GPU 工作暂无需求。
2. **pmxdrv PORT_IO / PCI_CFG 接线**（RTCore 表已给参照语义与防护设计）。
3. **GPU BAR 级寄存器伪造传输**——RTCore 恰好是"只打显卡、打不进内核"的现成签名
   载体，正合 3060 regtab 判决的 PCI MMIO 后继方向（PMAPH/卡侧寄存器实验）。

## 7. 备份价值裁定

| 用途 | RTCore 可用性 | 说明 |
|---|---|---|
| 宽表抬窗备份 | ❌ | BAR-only 门架构性排除；AB 无此能力可借鉴 |
| GPU MMIO 传输备份 | ✅ 可选 | 签名链干净（随 AB 在售构建）、SDDL admin-only；IOCTL 表全解可自建服务加载，或经 RTCore.dll（RTHAL 的用法）libloading |
| 风险 | 中 | 旧版 RTCore64 是知名滥用面（CVE-2017-12155 族），新构建未上 MS 推荐阻断清单（需实测 HVCI/WDAC 环境）；引入即引入第三方签名驱动审计面，须与 pmxdrv 平级对待（`--force` + 恢复计划 + 硬件门） |

**结论**：宽表抬窗我们已走在 AB 前面（AB 无此路径，RTCore 亦不可达）；RTCore 的
真正借鉴价值是给"卡侧寄存器伪造"方向提供一个**签名干净、门控清晰**的 MMIO 传输
备胎，以及给 pmxdrv 未接线的 PORT_IO/PCI_CFG 提供语义参照。

## 局限

- RTCore32.sys（35056B）同族 32 位实现，未逐字节复核（结构应对称）。
- MS 阻断清单/HVCI 兼容性为桌面常识推断，未在锁 HVCI 机器实测。
