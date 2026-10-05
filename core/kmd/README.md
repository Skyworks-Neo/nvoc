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

  IOCTL 面按构建代际分两套码表(请求布局两代相同,连接时用无副作用的
  LAST_ERROR 探针自动探测,`PmxDrv::build()` 返回代际):

  | 原语(请求布局,经 16 字节指针转发) | `Intel2019` 码 | `Paiptac` 码 |
  |---|---|---|
  | MAP_PHYS `{tag=24, u64 pa@4(≠0), u32 pages@12, u64 va_out@16}` | `0x222AB8` | `0x222878` |
  | UNMAP_PHYS `{tag=24, u64 va@16}` | `0x222ABC` | `0x22287C` |
  | LAST_ERROR(驱动写 u32 错误码到请求 `+4`) | `0x222AD0` | `0x222890` |
  | PORT_IO `{tag=16, op@4(1/2/3 in b/w/d, 4/5/6 out), u16 port@8, val@12}` | `0x222AA4` | `0x222864`(未接线) |
  | PCI_CFG `{tag=28, mode@4, bdf/off@8, bus@10, and@12, or@16, old@20, val@24}` | `0x222AA8` | `0x222868`(未接线) |
  | 事件环 17168 字节日志缓冲(xOCD 遥测) | `0x222A80` | `0x222840`(未接线) |

  Paiptac 构建还有 16 个其余码(0x222844..0x2228A4 面共 22 码,含 MSR/cpuid/
  虚拟读写等),未接线。

- **调用形态**(也是它被归为"漏洞驱动"的原因):DeviceIoControl 输入恒
  16 字节 = `[u64 用户态请求指针][u32 aux<=0x3F][u32 pad]`,驱动**不加探测
  直接解引用该指针**——所以请求结构就是本进程的普通堆内存;
  `MAP_PHYS` 经 `\Device\PhysicalMemory` 节对象 `ZwMapViewOfSection` 到
  **当前进程**,返回的 `va_out` 是本进程用户态地址,读完 `UNMAP_PHYS` 即可。
  映射页保护是 PAGE_READWRITE:传输层天然可写,本车道只读、写路径继续封存。
  映射失败的哨兵:va_out = 0 或 `0xBEEF`(32 位分支另有 `0xDEAD` = VA 超 32 位)。
- **坑(已踩,后记修正)**:对 2019 款,IDA 反编译器把分发器 IOCTL 基址
  折叠成 0x222840 族(实测 0x2228 族全数 win32 87 拒绝),汇编实证真基址是
  `0x222A80`(`sub edx, 222A80h`)。**但 0x222840 族并非虚构**——它恰是
  PAIPTAC 重建款的真码表(idalib case 表 + map/unmap handler 调用图实证,
  见下节):两代构建功能号排布互换,所以本通道按构建分键双码表。
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

## PawnIO 车道(已清退,结论保留)

2026-10-05 的早期路线:直连 `\Device\PawnIO`(g-helper 同款,不经
PawnIOLib.dll),装载官方签名模块跑 `ioctl_` 函数。实测结论:

1. **白手套调用层零签名工作**:官方签名模块(Echo)装载+执行全通;
2. **官方 23 枚签名模块没有任意物理内存读**(Nvidia.bin=BAR0 MMIO 温度、
   IntelMCHBAR=MMIO 窗口、MSR 族=MSR……`virtual_read_*`/`physical_read_*`
   原语只在驱动内,没有模块把它们透出成公开函数);
3. 自研 PhysMem 模块被签名门拒绝;等上游 PawnIO.Modules 收编签名 = 不可控
   等待,**用户裁决此路不通,代码与工件已清退**(传输层、模块源码/blob、
   探针测试均删除,git 历史可考古);
4. 附带发现:2.2.0 安装器的 `-unrestricted` 版与官方版**代码逐字节相同**
   (仅签名排布不同,`PAWNIO_UNRESTRICTED` 的 DbgPrint 串两枚都没有)——
   安装器开关在 2.2.0 不改代码,期望它开门本来就是死路。

驱动侧 `physical_read_*`(`MmGetVirtualForPhysical` + `__try/__except`)在位;
若未来 PawnIO.Modules 收编物理读模块,从 git 历史恢复 pawnio.rs 即可换回。

## 后继构建考证(2026-10-06,`reverse/kmd-driver-exploit-candidate/ANALYSIS.md`)

Eclypsium 2019-11《Mother of All Drivers》即本文档主角:PMxDrv=能力超集,
Intel 于 2019-11-12 发过修复版。实测 **PAIPTAC 重建版(`pmxdrv_new.sys`,
`CN=PAIPTAC Driver`,PDB `pmx-pai-built-source`)漏洞原样保留**——prologue
无探测用户指针解引用、create 空桩、`\Device\PhysicalMemory` 映射进调用进程
逐点同构,且 IOCTL 面膨胀到 22 码(**0x222840 族**,与 2019 版 0x222A80 族
不同代;双版兼容传输层需按构建分键码表)。同目录微软 WHQL 的 `KslD` 是
Defender TDT 传感器驱动(Rust,tdt_driver_lib),非物理内存 provider,排除。
本车道主用 Intel 1.0.0.1003(哈希钉死);**2026-10-06 起 pmxdrv.rs 双码表
兼容 PAIPTAC 重建款**(连接时 LAST_ERROR 探针自动分代,`PmxDrv::build()` 可查;
map/unmap/lasterror 布局两代逐字段相同,仅 map 多 `pa≠0` 校验,本通道不受影响)。

## 同类替代品盘点(2026-10-05)

| 驱动 | 出品 | 原语 | 状态 |
|---|---|---|---|
| **pmxdrv.sys** | Intel CSE/ME(2019) | 物理 map/unmap、端口 IO、PCI cfg | ✅ 本车道主用,签名有效,本机可装载 |
| WinRing0x64.sys | OpenLibSys/各 OEM | 物理 map、MSR、端口 IO | 本仓 per-rail 车道已用其接口(GPU-Z 同款);多处 OEM 改名变体,微软黑名单覆盖其部分哈希 |
| RTCore64.sys | MSI Afterburner(reverse/MSIAfterburnerSetup467Beta2 可提取) | 物理 r/w、MSR、端口 | 签名有效但黑名单常拦,且 nvoc 本身就是 NVAPI 生态,无需引入 |
| InpOutx64.sys | Phil Gibbons(InpOut32) | 端口 IO + `MapPhysToLin` | 可用,老,无增量 |
| gdrv.sys | Gigabyte | 物理 r/w(漏洞著名) | 黑名单钉死,排除 |
| PawnIO(官方版) | namazso | 官方签名模块的白名单原语 | 见上节:官方模块集无任意物理读,自研模块等签名=不可控等待,已清退 |

选型结论:pmxdrv 是唯一「合法 Intel 签名 + 任意物理 map + 本机实证可装载」
的组合;风险口径是 BYOVD(自带漏洞驱动)——传输层天然可写、无探测解引用,
本车道代码只读并只触碰 RAM 范围(低内存/页表帧)。

## 安全模型

- 走查限内核指针(`>= 0xFFFF8000_00000000`),单次虚拟读 ≤64 KiB,
  低内存扫描 255 页固定范围;
- 读错物理页的后果是读到垃圾(探针报活体/磁盘不一致),不会写坏;
- 唯一的写风险在显式接写路径之后——当前 Rust 层无任何写调用。
