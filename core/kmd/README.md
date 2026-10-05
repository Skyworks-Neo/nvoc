# nvoc kmd 通道(内核驱动物理读 + nvlddmkm 地址空间走查)

内核态驱动读写实验的落地目录。**当前只读**:物理读经已签名内核驱动转发,
在 [nvoc-core/src/kmd](../src/kmd/) 里复刻 xOCD 2.0.0 `NvidiaKernelReader` 的
页表走查算法,用于直接读取 `nvlddmkm.sys` 的内核虚拟地址空间。写路径
(驱动原语天然可写)留给后续能力位实验。

## 主传输层:Intel PMxDrv(`pmxdrv.sys`,pmxdrv.rs)

2026-10-05 经用户裁决定下的内核通道,即 xOCD 同款的「Intel ME 驱动路子」:

- **来源**:`reverse/xocd/xOCD.exe` 内嵌资源(xOCD.exe+0x38E5F4,43632 字节,
  SHA256 `B1A8EE1222EEA5F199028D90B9B77C2ACF46D6D84A9E125403B2888C6F681C72`);
  PDB 路径 `C:\MyProjects\git\pmx-cse-new\x64\Release\pmxdrv.pdb` —— CSE 即
  Intel Converged Security Engine(ME 的现代名),驱动自称
  `PMxDrv32e - IA32e Protected Mode Execution MP WinNT Driver`,2019-07-07 构建,
  Authenticode 签名 `Intel(R) Embedded Subsystems and IP Blocks Group`(有效,
  时间戳服务器 timestamp.intel.com)。驱动本体不入库(Intel 专有),留在
  `reverse/xocd/pmxdrv.sys`。
- **加载**:管理员 `sc create PMXDRV type= kernel start= demand binPath=<sys 绝对路径>`
  + `sc start PMXDRV`。本机实测(Win11 26200,HVCI 关):装载成功;
  微软漏洞驱动黑名单未拦(注册表 `VulnerableDriverBlocklistEnable` 未设)。
- **接口**(idalib 全量逆向 + 实机差分实证):设备 `\Device\Pmxdrv`,
  符号链接 `\DosDevices\PMXDRV`(用户态开 `\\.\PMXDRV`),全部
  `CTL_CODE(FILE_DEVICE_UNKNOWN, fn, METHOD_BUFFERED, FILE_ANY_ACCESS)`:

  | IOCTL | 值 | 用途 / 请求布局(经 16 字节指针转发) |
  |---|---|---|
  | MAP_PHYS | `0x222AB8` | `{tag=24, u64 pa@4, u32 pages@12, u64 va_out@16}` |
  | UNMAP_PHYS | `0x222ABC` | `{tag=24, u64 va@16}` |
  | LAST_ERROR | `0x222AD0` | 驱动写 u32 内部错误码到请求 `+4` |
  | PORT_IO | `0x222AA4` | `{tag=16, op@4(1/2/3 in b/w/d, 4/5/6 out), u16 port@8, val@12}`(未接线) |
  | PCI_CFG | `0x222AA8` | `{tag=28, mode@4, bdf/off@8, bus@10, and@12, or@16, old@20, val@24}`(未接线) |
  | 事件环 | `0x222A80` | 17168 字节日志缓冲(xOCD 遥测,未接线) |

- **调用形态**(也是它被归为"漏洞驱动"的原因):DeviceIoControl 输入恒
  16 字节 = `[u64 用户态请求指针][u32 aux<=0x3F][u32 pad]`,驱动**不加探测
  直接解引用该指针**;`MAP_PHYS` 经 `\Device\PhysicalMemory` 节对象
  `ZwMapViewOfSection(NtCurrentProcess)` 把物理页映射进**调用进程的用户空间**
  (PAGE_READWRITE),返回的 `va_out` 直接当用户指针读,读完 `UNMAP_PHYS`。
  映射失败的哨兵:va_out = 0 或 `0xBEEF`(32 位分支另有 `0xDEAD` = VA 超 32 位)。
- **坑(已踩)**:IDA 反编译器对分发器 `sub edx,imm / jz` 链的常量折叠会
  给出**错 0x240 的 IOCTL 基址**(0x222840 族)——汇编实证基址是
  `0x222A80`(`sub edx, 222A80h`);0x2228 族全数 win32 87 拒绝。
- **实机复现**(本机 K4000/582.41):两段探针全绿 ——
  `probe_pmxdrv_transport_maps_low_memory`(map/read/unmap 冒烟)+
  `probe_kernel_walk_reads_nvlddmkm_header`(255/255 低内存页可读,
  low-stub 唯一根 `0x1AE000`,活体 nvlddmkm 头 timestamp/SizeOfImage/
  256 字节前缀对磁盘指纹三项全匹配):

  ```
  # 管理员
  sc create PMXDRV type= kernel start= demand binPath= D:\git-repo\nvoc\reverse\xocd\pmxdrv.sys
  sc start PMXDRV
  cargo test -p nvoc-core --test kmd_pmxdrv_probe_live -- --ignored --nocapture
  sc stop PMXDRV && sc delete PMXDRV
  ```

## 后继构建考证(2026-10-06,`reverse/kmd-driver-exploit-candidate/ANALYSIS.md`)

Eclypsium 2019-11《Mother of All Drivers》即本文档主角:PMxDrv=能力超集,
Intel 于 2019-11-12 发过修复版。实测 **PAIPTAC 重建版(`pmxdrv_new.sys`,
`CN=PAIPTAC Driver`,PDB `pmx-pai-built-source`)漏洞原样保留**——prologue
无探测用户指针解引用、create 空桩、`\Device\PhysicalMemory` 映射进调用进程
逐点同构,且 IOCTL 面膨胀到 22 码(**0x222840 族**,与 2019 版 0x222A80 族
不同代;双版兼容传输层需按构建分键码表)。同目录微软 WHQL 的 `KslD` 是
Defender TDT 传感器驱动(Rust,tdt_driver_lib),非物理内存 provider,排除。
本车道维持 Intel 1.0.0.1003(哈希钉死)不变。

## 同类替代品盘点(2026-10-05)

| 驱动 | 出品 | 原语 | 状态 |
|---|---|---|---|
| **pmxdrv.sys** | Intel CSE/ME(2019) | 物理 map/unmap、端口 IO、PCI cfg | ✅ 本车道主用,签名有效,本机可装载 |
| WinRing0x64.sys | OpenLibSys/各 OEM | 物理 map、MSR、端口 IO | 本仓 per-rail 车道已用其接口(GPU-Z 同款);多处 OEM 改名变体,微软黑名单覆盖其部分哈希 |
| RTCore64.sys | MSI Afterburner(reverse/MSIAfterburnerSetup467Beta2 可提取) | 物理 r/w、MSR、端口 | 签名有效但黑名单常拦,且 nvoc 本身就是 NVAPI 生态,无需引入 |
| InpOutx64.sys | Phil Gibbons(InpOut32) | 端口 IO + `MapPhysToLin` | 可用,老,无增量 |
| gdrv.sys | Gigabyte | 物理 r/w(漏洞著名) | 黑名单钉死,排除 |
| PawnIO(官方版) | namazso | 官方签名模块的白名单原语 | 见下节:官方模块集无任意物理读,自研模块等签名=不可控等待 |

选型结论:pmxdrv 是唯一「合法 Intel 签名 + 任意物理 map + 本机实证可装载」
的组合;风险口径是 BYOVD(自带漏洞驱动)——传输层天然可写、无探测解引用,
本车道代码只读并只触碰 RAM 范围(低内存/页表帧)。

## PawnIO 车道(实验存档,pawnio.rs + PhysMem.p)

早期路线,调用形态与 g-helper(`reverse/g-helper-main/app/Pawn/PawnIOWrapper.cs`)
一致:直接打开 `\Device\PawnIO` 做 `LOAD_BINARY(0xA1B22084)` /
`EXECUTE_FN(0xA1B22104)` / `VERSION(0xA1B22184)`,每句柄装载一个 Pawn 模块。

实测结论(本机,驱动 2.2.0 Official):

1. **白手套调用层零签名工作**:官方签名模块(Echo)装载+执行全通,
   `ioctl_not(0x0123456789ABCDEF) -> 0xFEDCBA9876543210`;
2. **官方 23 枚签名模块没有任意物理内存读**(Nvidia.bin=BAR0 MMIO 温度、
   IntelMCHBAR=MMIO 窗口、MSR 族=MSR……`virtual_read_*`/`physical_read_*`
   原语只在驱动内,没有模块把它们透出成公开函数);
3. 自研 PhysMem 模块(源码与 blob 留档本目录)被签名门拒绝;等上游
   PawnIO.Modules 收编签名 = 不可控等待,**用户裁决此路不通**;
4. 附带发现:2.2.0 安装器的 `-unrestricted` 版与官方版**代码逐字节相同**
   (仅 WHQL cat 21816B vs 自签 cat 11328B 之差,恰等于 sys 大小差 10488B;
   `PAWNIO_UNRESTRICTED` 的 DbgPrint 串两枚都没有)——安装器开关在 2.2.0
   不改代码,期望它开门本来就是死路。

驱动侧 `physical_read_*`(`MmGetVirtualForPhysical` + `__try/__except`)在位,
PhysMem.p 上游就绪;若未来 PawnIO.Modules 收编,换回 pawnio.rs 传输层即可。

### 文件

| 文件 | 说明 |
|---|---|
| [PhysMem.p](PhysMem.p) | 自研 Pawn 模块源码:`ioctl_read_phys_qword` / `ioctl_read_phys_page`(512×u64)/ `ioctl_read_phys_dword` + 三个写 ioctl(未接线) |
| `PhysMem.amx` | pawncc 4.1.7152 编译产物(与上游 PawnIO.Modules CI 同工具链、同旗标) |
| `PhysMem.bin` | 装载 blob = `[u32 sig_len=0][amx]`——**未签名**,官方驱动拒收 |
| `Echo.bin` | 上游官方模块包 0.2.11 的签名模块 fixture(传输层对照,`ioctl_not`),LGPL-2.1-or-later |

构建(与上游 CI 完全一致的旗标):

```
pawncc PhysMem.p -i<PawnIO>/pawn/include -C64 -;+ -(+ -p
printf '\0\0\0\0' > PhysMem.bin && cat PhysMem.amx >> PhysMem.bin
```

注意上游 `Echo.p` 的注释:模块只引用一个 native 会触发解释器 bug;本模块
已经引用多个 `physical_*` native,`main()` 里另取一次 `get_version()` 双保险。

PawnIO 探针(管理员):`cargo test -p nvoc-core --test kmd_pawnio_probe_live -- --ignored --nocapture`;
环境变量 `NVOC_KMD_SIGNED_BLOB`(默认 `core/kmd/Echo.bin`)、
`NVOC_KMD_PHYS_MODULE`(默认 `core/kmd/PhysMem.bin`)。

## 安全模型(两条通道通用)

- 走查限内核指针(`>= 0xFFFF8000_00000000`),单次虚拟读 ≤64 KiB,
  低内存扫描 255 页固定范围;
- 读错物理页的后果是读到垃圾(探针报活体/磁盘不一致),不会写坏;
- 唯一的写风险在显式接写路径之后——当前两条通道的 Rust 层均无写调用。
