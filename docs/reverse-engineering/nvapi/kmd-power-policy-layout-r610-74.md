# RM 电源策略对象布局 — nvlddmkm 610.74(RTX 4060 Laptop,AD107)

**版本红线适用**:本文全部 RVA/偏移仅对本机构建
(`DriverStore\FileRepository\nvami.inf_amd64_1a0e2bbd9919af1d\nvlddmkm.sys`,
SHA256 `61CB92ADF51E915510926DFF6533CBE92591861316142DE30DF5B4A5B1EBA76`,
TimeDateStamp `0x6A46A468`,SizeOfImage `0x7370000`)成立,跨版本不成立。
与 616.92(nvpwrcontrol-blackwell-tuner-audit.md)的对照列为第三列,用于
观察漂移规律,不得移植数值。

逆向方法:capstone 字节签名(从 616.92 审计的签名泛化)+ idalib 9.0 建库
交叉验证(调用图/字段写入点枚举),两路结论一致。逆向日期 2026-10-06。
活体走查工具:`core/tests/kmd_power_policy_diff_live.rs`(kmd 通道,只读)。

## 1. RM 命令面

| 项 | 610.74 | 616.92 |
|---|---|---|
| NVAPI 函数 | 0x8B3E7343 GET / 0xAFFC2279 SET(PowerPolicyGet/SetControl) | 同 |
| **内核 RM 命令** | **0x2080A61A(GET)/ 0x2080E61B(SET)** | —(未记录) |
| 分派表项 | .rdata `{cmd u32, params u32=0x3610, link ptr, handler ptr, flags}`;GET entry @RVA 0xE73740,SET @0xE747A0 | — |
| handler | GET 0x4E9790 / SET 0x4E9A90 | — |

用户态 nvapi64_impl.dll(R610.74)GET handler @0x180259530 写 `0x2080A61A`
入工作缓冲 +0x34,接受版本戳 {0x10298, 0x106DC, 0x10A4C, 0x11F10, 0x12720,
**0x5B0B0**(新)};SET handler @0x18025DDE0 分支写 `0x2080E61B`。

## 2. 对象链(SET handler 0x4E9A90 实证)

```
Major  = [[pRmCtrlParams + 0x158] + 0xE0]
PowerRoot = [Major + 0x2510]                  ← 616.92: +0x25B0
Board  = lookup([root + 0x1C90], key)         ← 616.92: registry +0x1CC0
         lookup fn 指针 @ root + 0x1CC8        ← 616.92: +0x1CF8
         (静态:通用查找函数 RVA 0xCF1EA0,三个调用点共享 .rdata 槽 0xE07AB8)
BoardSet fn 指针 @ Board + 0x2D0(生成器用)/ +0x2D8(SET handler 用)
```

有效性门(与 616.92 同构):`byte[root+0x3CE0] == 1`(init)、
`byte[root+0x3CEC] < 0x40`(policy key)。

## 3. PowerRoot 字段(漂移规律:**整块 -0x30**)

| 字段 | 610.74 | 616.92 | 语义 |
|---|---|---|---|
| init | +0x3CE0 | +0x3D10 | ==1 才有效 |
| eligibility | +0x3CE1 | +0x3D11 | DB 资格;SetElig 调用即重跑生成器 |
| amountActive | +0x3CE2 | +0x3D12 | 动态量激活 |
| **base / cTGP(C)** | **+0x3CE4** | +0x3D14 | 生成器 C 项;-1 = 未设哨兵(610 可能为哨兵态) |
| **amount(A)** | **+0x3CE8** | +0x3D18 | PPAB/DB 预算 |
| policy key | +0x3CEC(byte) | +0x3D1C | Board 注册表键 |
| **LOWER(B)** | **+0x3CF0** | +0x3D20 | 生成器 B 项 |
| **UPPER(U)** | **+0x3CF4** | +0x3D24 | 饱和顶,抬顶核心字段 |
| aux1/aux2 | +0x3CF8/+0x3CFC | +0x3D28/2C | 未破译(生成器在哨兵分支读) |
| selector 索引表 | +0x268C(byte/项,0xFF=未映射) | — | selector id → 槽位 |
| root+0x2310 | 记录应用器 0x4ED7A0 读取 | — | 伴随对象 |

Major+0x2508 = PowerRoot 的兄弟对象(深层 setter 0x4E75D0 校验用);
root+0xEE10 形态的引用见 PAGE_DD 0x15C1B61 字段拷贝循环(源对象 +0xEE10
→ Major+0x2510,即另一对象类型同持 root)。

## 4. 函数族(610.74 RVA)

| 函数 | 610.74 | 616.92 | 备注 |
|---|---|---|---|
| F7 生成器 | **0x4ED5C0** | 0x4E3EF0 | 数学同构:min(C, U−A)+A ≡ min(C+A, U) |
| 记录写点 | 0x4ED758(`mov word [rsp+0x48], 0x3F7`) | 0x4E4088 | 栈偏移 0x48 相同 |
| 记录应用器 | 0x4ED7A0 | — | 写 {0x3FE, 0x3F7, 0x6FE} 三记录入 Board |
| SetAmount 族 | 0x4EDCE0 | 0x4E4610 | 写 amount/amountActive,读 UPPER |
| SetEligibility | 0x4EDD50 | 0x4E4680 | 写 elig(0x88)并调生成器 |
| base/UPPER 重写器 | 0x4EEB60 | — | 含 base 的 `mov [mem],reg` 写点 0x4EEE0C |
| 深层 setter | 0x4E75D0 | 0x8D6960 | source==0xFD 门,窗钳校验(source0/2) |
| selector 写入器 | 0x4E70A0 / 0x4E7300 | 0x8D6960 家族 | mode {0-4}/{7,8} 分派 |
| 内层应用 | 0x5B5560 → 0x5CA880 | — | — |

**UPPER 写入点枚举(idalib 全镜像 disp32 扫描)**:常规 `mov [r+disp32]`
编码下 **0x3CF4 无任何写点**(5 处全为读);写路径仅存在于 SIB/memcpy 形态
(构造/装载路径 0x14EA450/0x14EBF30 家族)——与 616.92「顶侧无命令面出口」
结论同构。base 的常规写点仅 0x4EEE0C(0x4EEB60 内)。

## 5. 出厂态与差分指纹(4060L 实测锚)

get-pwr-cur-info(policyId 0)= 100000 mW;nvidia-smi 窗 [5, 140] W;
NVVDD OCP 135000 mA / MSVDD 5001000(哨兵)。
出厂期待:UPPER=100000、amount=0、elig=0、amountActive=0;
base 可能为 100000 或 -1 哨兵(**未经活体证实,差分判读时不得硬性假设 base==UPPER**)。

root 同页指纹(活体定位用,偏移按 610):
`dword[o]==100000(UPPER) && byte[o-0x14]==1(init) && byte[o-8]<0x40(key)`,
root_va = page_va + o - 0xCF4。

## 6. 活体差分状态(2026-10-06,L2.2 未闭合)

**已实证**:
- SET 提权可用:90/95/100 W 全部接受 + 读回一致(-137=非提权,与既有台账一致);
- 走查协议工作正常(镜像节指针收集 → 页表翻译 → 物理页读)。

**阴性结果(防重蹈)**:
1. 策略对象**不在**锚页(功率通道控制表,含 {100000,150000,50000,500000,
   10000,1000000} 值行,90 W 时不变 → 是默认/镜像表非活体状态)的 2MB 池簇;
2. 锚页 1-2 跳指针邻域无 root 指纹;
3. .data 一跳 + LIFO 2000 页、枢纽 FIFO 1500 页均未命中;
4. GPU-ID(vendor:device 打包 0x28E010DE)标记找到的是资源描述符注册表
   (NVRM/DxgKi4$ 标签记录),非 RM 对象表;610 的 GPU 表无 616.92 式
   {Major 指针,GPU ID} 相邻对。

**未闭合**:PowerRoot 活体 VA/物理帧。静态布局已齐,差分判读只要扫到
含五元组的页即可定案,缺的是"从哪扫"的最后一跳。

**补充阴性台账(同日晚间,~15 轮有界活体运行)**:
1. 全局槽已找到:语义扫描(rip 全局加载 → ≤2 条内 deref,~3.29M heads)唯一热点
   **state 槽 RVA 0x13AAD58**(偏移族 {0x168,0x200..0x280});活体读出
   state = 0xFFFFD205858D0D30(每次 boot 漂移,0x13AAD58 处恒有值)。
2. **610 的 state→GPU 表偏移 ≠ 0x208**(与 616.92 同值是巧合):
   [state+0x208] 指向的对象是事件聚合结构(字段形态 {count,value} @
   0x50/0x1C8/0x340/0x4B8),其指针域无 {Major,GPU-ID} 邻接对、无 100000。
3. state 前 4 页(16KB)全部指针 P:探 [P+0xEE10](per-GPU 大上下文持 root
   形态)与 [P+0x2510](Major 形态)的 root 门(init=1 && key<0x40 &&
   UPPER∈{100000,135000,140000,150000})全阴性;P+0x48000 可读的
   "大分配"候选 = 0(GPU 表不在 state 前 4 页指针的直接目标里)。
4. 下一步(按性价比排序):a) idalib 深追 RM dispatch 的 ctx 构造链
   ([[ctx+0x158]+0xE0] 的 0x158 对象从哪个全局锚生);b) 拿 616.92 x64
   二进制比对 state 结构布局(桌面上 616.00 是 ARM64,不可用);
   c) root 门放宽前的其它结构假设。差分协议本身已就绪,只差入口指针。

## 7. 安全 envelope(BSOD 教训,硬性)

2026-10-06 02:07 BFS 25 万页池簇走查触发 **KMODE_EXCEPTION_NOT_HANDLED** 蓝屏
(无 minidump 留存;当时服务 PMXDRV_NEW/Intel2019)。此后纪律:
- 单次走查页数预算 **≤2000**(已 5 次安全复现的量级),放大必须显式改
  `NVOC_POWER_WALK_PAGES` 并记录;
- RAM 白名单(`HARDWARE\RESOURCEMAP\...\Physical Memory`)已实现但该表在
  本机只有 0.91 GiB 碎片视图(legacy 截断),**不能**当完整 RAM 地图 —
  白名单只作 defense-in-depth;
- 头号嫌疑 = 批量映射 MMIO/保留帧触发 pmxdrv 无 probe 解引用缺陷;宁可少走。
