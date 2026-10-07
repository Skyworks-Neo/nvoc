# nvoc kmd 通道(内核驱动物理读 + nvlddmkm 地址空间走查)

内核态驱动读写实验的落地目录。读路径:物理读经已签名内核驱动转发,
复刻 xOCD 2.0.0 `NvidiaKernelReader` 的页表走查算法,直接读取
`nvlddmkm.sys` 的内核虚拟地址空间。写路径已于 2026-10-06 **受控开放**:
仅限 [`power.rs`](power.rs) 的功率墙原子流(`set-power-command --kmd`,
见下文「功率墙原子流」节),其余通道保持只读。

## 主传输层:Intel PMxDrv(`pmxdrv.sys`,pmxdrv.rs)

2026-10-05 经用户裁决定下的内核通道,即 xOCD 同款的「Intel ME 驱动路子」:

- **来源**:`xOCD.exe` 内嵌资源(xOCD.exe+0x38E5F4,43632 字节,
  SHA256 `B1A8EE1222EEA5F199028D90B9B77C2ACF46D6D84A9E125403B2888C6F681C72`);
  PDB 路径 `C:\MyProjects\git\pmx-cse-new\x64\Release\pmxdrv.pdb` —— CSE 即
  Intel Converged Security Engine(ME 的现代名),驱动自称
  `PMxDrv32e - IA32e Protected Mode Execution MP WinNT Driver`,2019-07-07 构建,
  Authenticode 签名 `Intel(R) Embedded Subsystems and IP Blocks Group`(有效,
  时间戳服务器 timestamp.intel.com)`。
- **加载**:管理员 `sc create PMXDRV type= kernel start= demand binPath=<sys 绝对路径>`
  + `sc start PMXDRV`。本机实测(Win11 26200,HVCI 关):装载成功;
  微软漏洞驱动黑名单未拦(注册表 `VulnerableDriverBlocklistEnable` 未设)。
- **接口**(idalib 全量逆向 + 实机差分实证):设备 `\Device\Pmxdrv`,
  符号链接 `\DosDevices\PMXDRV`(用户态开 `\\.\PMXDRV`),全部
  `CTL_CODE(FILE_DEVICE_UNKNOWN, fn, METHOD_BUFFERED, FILE_ANY_ACCESS)`:

  IOCTL 码表(`0x222A80` 族,请求布局见行内):本机两代构建(Intel 2019 款与
  system32 在役 PAIPTAC 款)**实测同一码表**——PAIPTAC 款是超集(22 码 vs
  本通道使用的 6 码),map/unmap/lasterror 逐一同位,2026-10-06 在役 PAIPTAC
  构建实跑走查全绿:

  | 原语(请求布局,经 16 字节指针转发) | 码 | 状态 |
  |---|---|---|
  | MAP_PHYS `{tag=24, u64 pa@4(≠0), u32 pages@12, u64 va_out@16}` | `0x222AB8` | 已接线 |
  | UNMAP_PHYS `{tag=24, u64 va@16}` | `0x222ABC` | 已接线 |
  | LAST_ERROR(驱动写 u32 错误码到请求 `+4`) | `0x222AD0` | 已接线 |
  | PORT_IO `{tag=16, op@4(1/2/3 in b/w/d, 4/5/6 out), u16 port@8, val@12}` | `0x222AA4` | 未接线 |
  | PCI_CFG `{tag=28, mode@4, bdf/off@8, bus@10, and@12, or@16, old@20, val@24}` | `0x222AA8` | 未接线 |
  | 事件环 17168 字节日志缓冲(xOCD 遥测) | `0x222A80` | 未接线 |
  | (PAIPTAC 款另有 16 码至 `0x222AE4`,含 MSR/cpuid/虚拟读写等) | — | 未接线 |

- **调用形态**(也是它被归为"漏洞驱动"的原因):DeviceIoControl 输入恒
  16 字节 = `[u64 用户态请求指针][u32 aux<=0x3F][u32 pad]`,驱动**不加探测
  直接解引用该指针**——所以请求结构就是本进程的普通堆内存;
  `MAP_PHYS` 经 `\Device\PhysicalMemory` 节对象 `ZwMapViewOfSection` 到
  **当前进程**,返回的 `va_out` 是本进程用户态地址,读完 `UNMAP_PHYS` 即可。
  映射页保护是 PAGE_READWRITE:传输层天然可写,本车道只读、写路径继续封存。
  映射失败的哨兵:va_out = 0 或 `0xBEEF`(32 位分支另有 `0xDEAD` = VA 超 32 位)。
- **坑(已踩,真相反转)**:初版把反编译器 case 表的**十进制** 2239104
  手算成 0x222840,据此写出一套 0x2228 族错码、实测全数 win32 87 拒绝,又
  从汇编 `sub edx, 222A80h` 得出"IDA 折叠出错"的错误结论。精算后
  2239104 = **0x222A80**——反编译器与汇编从来一致,错的是我的进制换算。
  教训:CTL_CODE 常量一律用工具换算,不手算(见 nvapi-struct-magic-idioms
  同类教训)。
- **实机复现**(P100/582.41):两段探针全绿 ——
  `probe_pmxdrv_transport_maps_low_memory`(map/read/unmap 冒烟)+
  `probe_kernel_walk_reads_nvlddmkm_header`(255/255 低内存页可读,
  low-stub 唯一根 `0x1AE000`,活体 nvlddmkm 头 timestamp/SizeOfImage/
  256 字节前缀对磁盘指纹三项全匹配):

  ```
  # 管理员
  sc create PMXDRV type= kernel start= demand binPath= [pmxdrv.sys]
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
逐点同构,且 IOCTL 面膨胀到 22 码(同一 **0x222A80 族**的超集;初版分析的
"0x222840 族换代"说法是十进制换算错误,已撤,见上节坑注)。同目录微软 WHQL 的 `KslD` 是
Defender TDT 传感器驱动(Rust,tdt_driver_lib),非物理内存 provider,排除。
本车道主用 Intel 1.0.0.1003(哈希钉死);PAIPTAC 重建款与它**同码表线兼容**
(实机验证:system32 在役 PAIPTAC 构建跑走查全绿),但 **4060L 笔记本上
PAIPTAC 款被安全策略阻止装载、Intel 2019 款正常**(2026-10-06 用户实测)——
出发行继续钉死 Intel 款。

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

KDU考察结论(2026-10-06):**不采用**。其
pmxdrv provider 与本通道逐项相同(intel.cpp 同码 0x222AB8/ABC、同 low-stub
根发现、同 VtoP 走查——三方同源互证),`-map` 机制解决的是我们不需要的
"装载未签名驱动",且 KDU.exe 是 AV/EDR 摩擦最大的 BYOVD 工具、其 provider
同样吃黑名单(不解决 4060L 拦 PAIPTAC 那类问题)。已借用:providers.md 目录
与 provider 回退预案(备选第二传输=WinRing0/RTCore,走 `PhysicalMemory`
trait,不引入 KDU)。详见 `ANALYSIS.md` §5。

## 安全模型

- 走查限内核指针(`>= 0xFFFF8000_00000000`),单次虚拟读 ≤64 KiB,
  低内存扫描 255 页固定范围;
- 读错物理页的后果是读到垃圾(探针报活体/磁盘不一致),不会写坏;
- 写路径受控开放:仅 [`power.rs`](power.rs) 功率墙原子流,其余只读。

## 跨代静态布局探测(`layout_probe.rs`,2026-10-06)

对任意一代 x64 nvlddmkm **磁盘镜像**,纯静态推导 RM 电源策略对象链的
全部偏移(fail-closed,零硬件):DriverGlobal 槽 → GPU 表(count/ID/Major)
→ Major→root → root 七字段(init/elig/amountActive/base/amount/key/LOWER/
UPPER),外加 RM 命令 0x2080A61A/0x2080E61B 的分派表项与 handler RVA。

锚点链(全部跨代实证,576.02/610.74/616.92 三代):

1. **F7 记录写签名** `66 C7 44 24 ?? F7 03`(字节级稳定)→ int3 边界回溯
   得生成器 → 反汇编提 root 字段:结构间距分类(byte 对 {x,x+1} + 五 dword
   {x+4..x+14}),绝对偏移即出。注意 init(x) 本身可能不被生成器访问
   (610 实测只摸 elig/amountActive/key),从 elig 候选回退推 x;
2. **SetAmount 族互证**:`80 B8 [init] 00`(cmp byte [reg+init],0)签名
   → 序首 `mov r64,[rcx+M]` = Major→root,与锚 1 不一致即拒绝;
3. **字节锚定语义扫描**:全 .text 扫 `48/4C 8B ??(mod=00,rm=101)`
   (mov r64,[rip+d32],目标在写段)→ 0x200 小窗寄存器跟踪。跨代教训:
   全量线性反汇编在数据混排段失同步,字节锚定是唯一可靠路径。产物 =
   DriverGlobal 槽 + state→表 + 表内大偏移簇;
4. **表链消歧**:ID/Major 谁带 disp32 随代码生成翻转(610: ID 可见;
   616.92: Major 可见)——按**访问宽度**消歧(dword=ID、qword=Major),
   dword 候选逐个试,「ID 后方 0x100..0x400 有 count」为接受条件;
   count−ID = **0x1F8 三代恒定**(结构不变量,可作硬校验);
5. **RM 分派**:cmd 立即数在 .text(代码引用)与 .rdata(分派表)各命中
   一次,只认「+0x10 处为镜像内指针」的分派表命中。

产物 `NvlddmkmLayout`(serde)带锚点审计 trail;校准测试
`kmd_layout_probe_calibration.rs` 钉死三代地面真值逐字段断言。

| | 576.02 | 610.74 | 616.92 |
|---|---|---|---|
| state 槽 | 0x1239E50 | 0x13AAD58 | 0x13B2E18 |
| state→表 | 0x1F0 | 0x200 | 0x208 |
| count/ID/Major | 3F1F0/3EFF0/3EFF8 | 48440/48248/48240 | 48C48/48A50/48A48 |
| Major→root | 0x2258 | 0x2510 | 0x25B0 |
| root init/UPPER | 0x1638/0x164C | 0x3CE0/0x3CF4 | 0x3D10/0x3D24 |

## 功率墙原子流(`power.rs`,`set-power-command --kmd`)

写路径受控开放的唯一出口。**一条命令完成全周期**,漏洞驱动只在内存里
停留一个命令周期,按 PowerRoot 武装态自动分双臂:

```text
nvoc-cli set-power-command 160 --kmd --pmxdrvpath <pmxdrv.sys> --force
```

```text
服务注册(PMXDRV_KMD,残留自清;设备已在位则复用,崩溃残留认领)
→ 布局自动探测(layout_probe,对本机在役镜像;跨代零硬编码)
→ 走查(255 页低内存扫根 → 页表翻译)
→ GPU 表链走查 → 臂选择:
  root 臂(身份门过 = 移动/board 配置形态,4060L 实证)
    → D 状态感知定位(活体墙 tgp-control-current 精确匹配候选;歧义即拒)
    → UPPER 单 u32 写(身份门 init=1/key<0x40 + 读回,不符即时回滚)
    → 原生 tgp watt 写(仅控制值 < 目标时;窗随 UPPER)
  board 臂(身份门全拒 = 桌面形态:构造期无 board 配置对象,PowerRoot
    永不武装 init=0/key=0/UPPER=0,2070/610.47 实证 + idalib 定案;
    定位/扫查逻辑在 [`board.rs`](board.rs),纯值签名零布局硬编码)
    → 活体窗三元组(tgp_watt_range + status,GET 全安全面)
    → Board 控制表扫查(root 对象 8 页 + 内核指针一跳 + 候选页池邻域
      ±64 页;同页 {current,default,max} 三元组判据,值相撞时要求互异
      dword,全等三元组要求三处)
    → 逐候选探测(≤8):写窗 max(读回)→ percent 到达验证(多假设
      100%/按窗顶/按 default)→ 不到达即回滚+恢复 current。
      教训两连:① echo/lease 镜像行(0xFE 标记 cell)与活体行静态不可分;
      ② range GET(0x67F31384)读静态 policy info 行 —— 二轮差分(NVML -pl
      200)实证控制行 control 槽跟写走而 GET 不动,"GET 跟随" oracle 天生
      失灵,percent 读回 ≥ 目标是唯一可靠判据(每轮自愈零残留);
      无一跟随全回滚拒写(转差分:NVML 扰动 current 后重 trace)
    → percent 写 current(0xAD95F5ED 安全线;Turing 上 watt SET 0xAFFC2279
      毒,本臂绝不触碰 set_tgp_watt)
→ 租约写最后(回显面同步;checked 包络,越包络降级直写;两臂共用)
→ 复验(墙字段仍 == 目标)→ 服务注销(驱动拒卸载时诚实上报)
```

安全设计(逐条实证):

- `--force` 必需;**绝对上限 500 W**,任何旗标都不过;
- board 臂窗内目标直接拒(不需要内核写,percent/NVML 即可);
- root 臂 UPPER 易失、board 臂窗 max 预期同为易失(vBIOS 派生)——
  「每开机一跑」的单命令形态互相成全;- 崩溃自愈:服务名 PMXDRV_KMD 是本命令专属,上次进程死在清理前时,
  下次运行认领残留并在结尾清理;用户自管服务(如 PMXDRV_NEW)永不触碰;
- System32\drivers 回退:内核取不到原始卷镜像时(StartService win32 2)
  自动复制重试一次;`\??\` ImagePath 不吃 canonicalize 的 `\\?\` 前缀;
- 设备名冲突(`\\.\Pmxdrv` 已被在役实例占用 → win32 183):复用分支兜住,
  不与用户自管服务打架。

实测(4060L/610.74,2026-10-06):UPPER 140000→160000 抬墙后窗钳跟随
(tgp 160 接受),负载 Board Power **164.1 W**(超越 140 W 滑条顶),
1000 s FP32 GEMM 无计算错误;三代校准全绿。端到端三轮:干净态全周期 ✓、
幂等重跑 ✓、崩溃残留自愈 ✓。board 臂 2026-10-07 落地待两机实测
(桌面 2070/TU104 + 无 shunt mod 3060);单测覆盖三元组匹配/值相撞/
全等三元组/跨度拒/指针一跳定位/歧义拒绝/预算记账。

前置:管理员令牌;显卡低电压锁已解(不解除则负载吃不满新墙,详见
任务书 §4 field note)。

## Board 窗定位算法(`board.rs`,2026-10-07)

桌面形态破解臂的目标搜索(纯算法,跨平台可单测,写臂共用同一实现):

- **锚**:layout_probe 的 GPU 链给 root VA;root 对象页(root_va 起 8 页,
  覆盖三代字段族)+ 页内内核指针一跳目标(≤512 页)+ **候选页池邻域**
  (命中页 ±64 页,sweep 预算 768;总预算 ≤1288 ≤ BSOD 纪律 2000);
- **判据**:同页 dword 值签名 —— {current, default, max} 逐一等于活体 GET
  读数,全字段跨度 ≤0x20 字节;current==default(出厂未扰动)要求两处
  互异 dword,三元全等要求三处(噪声门);min 可选加分;
- **消歧 = 探测,不是拒绝**:echo/lease 镜像行与活体行静态不可分
  (2070 实测:0xFE 标记 lease cell 的三元组同样成立),且 range GET 读
  静态 info 行不反映 max 写 —— 写臂对候选逐个"写-percent 到达验-回滚+
  恢复"(每轮自愈零残留),胜出行保持抬升;候选 >8 拒(歧义面失控);
- `kmd_locate_trace_live.rs` 打印全部候选 + max 槽 ±0x20 hexdump 供人工
  判读;0 候选时跑差分(NVML 扰动 current 后重 trace,活体行的 current
  会跟动)。
