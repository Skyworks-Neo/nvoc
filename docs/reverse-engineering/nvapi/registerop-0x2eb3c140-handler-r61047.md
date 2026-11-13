# NvAPI_GPU_RegisterOp (0x2EB3C140) handler anatomy — R610.47 Windows

2026-10-19，本机桌面 TU106 / RTX 2070 / 610.47。
样本：`System32\nvapi64.dll`（薄壳，572 KB）+ `DriverStore\FileRepository\nv_dispig.inf_amd64_*\nvapi64_impl.dll`（5.6 MB）。
IDA 证据：薄壳 `nvapi_QueryInterface` @0x18000A470 静态表未命中即经 `sub_18000A240` 转发 impl DLL。
impl 分发表（0x1804DE000 区）两条注册：`{0x1801AF1D0, 0x2EB3C140}`（主）与 `{0x180ACFF0, 0x2EB3C140}`（次，反编译失败，疑似另一客户端面）。

## 处理器契约（sub_1801AF1D0，逐条反编译）

1. **能力门**：全局能力字 bit 0x2000 置位（虚化/TCC 等状态）→ 直接 `-104 NVAPI_NOT_SUPPORTED`。
2. `handle == 0` → `-101 EXPECTED_PHYSICAL_GPU_HANDLE`。
3. **版本戳**：`version != 71688 (0x11808 = 6152 | 1<<16)` → `-9`。6152 = 8B 头 + 256×24B op —— 与 LACT/我们的布局逐位一致。
4. `count` 须在 `1..=256`。
5. **每 op flag 解码**（op flags @op+0，24B stride）：
   - 方向+尺寸组合必选一：`(f&5)==5` READ|32BIT→type0、`(f&9)==9` READ|64BIT→type2、`(f&6)==6` WRITE|32BIT→type1、`(f&0xA)==0xA` WRITE|64BIT→type3，其余 → `-5 INVALID_ARGUMENT`；
   - 类型位必选一：`f&0x10`→domain0（LACT 的 GLOBAL）、`f&0x20`→domain1，否则 `-5`。
6. op 字段拷贝进 8248B 内核包（含 offset/write_mask 高低半/value 高低半），随后
   **escape 0x070000F1（117440593）提交**；成功后 value 写回 op+16、per-op status 写 op+2
   （bit0|1|2|4 组合）。
7. 用户态所有校验都过之后，**执行成败完全取决于内核侧**。

## 实机两层验证（TU106/GDDR6，同一台机）

**非管理员进程**：任意偏移（含 0x00000000 散点）、任意合法 flag → 一律 call 级 `-1 NVAPI_ERROR`，
per-op status 恒 0 —— 内核注册读通道**整体权限门**，与偏移无关。用户态无从区分窗口存在性。

**管理员进程**（同机提权重跑）：

| 偏移 | 读值 | 判读 |
|---|---|---|
| 0x900200 clamshell | `0x049E1003`（bit22=1） | 2070 非 clamshell —— bit22 语义在 GDDR6 代不适用，佐证解码为 GDDR6X/7 专属 |
| 0x9024C0/D0（GDDR data/status p0） | `0xBADF5040` | **0xBADF0000 毒值族** —— 窗在、解码器在、数据被标"无有效传感器" |
| 0x9064D0（partition 1 status） | `0xBADF5040` | 同上 |
| 0xAD0AA0（Blackwell hotspot） | `0xBADF1100` | Turing 上为毒值 —— is_blackwell 门控正确 |
| 0x00000000 / 0x100000 / 0x800000 | `0x166000A1` / `0x0FFFFFFF` / `0x0` | 真实可读数据 —— 通道本身完全健康 |
| 0x02000000+ | `-1` | 高地址段仍有第二道窗口门 |

## 结论

- **GDDR6 颗粒没有可经此接口读取的片上温度传感器**：GDDR6X（Micron PAM4）才引入 per-chip
  传感阵列，GDDR6 窗恒返 `0xBADF****` 毒值。LACT 的 `GDDR6x|GDDR7` 门控与 `0xBADF0000`
  过滤语义均被本机实证；我们的忠实移植无需改动。
- **RegisterOp 需要管理员进程**（Windows 同 Linux root）；非管理员表现为 call 级
  `-1 NVAPI_ERROR`（不是 -104/-5/-9，所有用户态校验先通过）。
- GPU-Z/HWiNFO 仅在 GDDR6X/7 卡显示 VRAM 温度，与本判决一致（旁证）。

## 窗口测绘与全量扫描可行性（提权，TU106/2070，2026-10-19）

批量语义：**all-or-nothing**——包内任一 op 越门整包 `-1`（per-op status 全 0），合法 256-op
包整体 rc=0。全量扫描必须按已知合法窗口切片，失败批回退单发。

实测吞吐：单发 16.3 µs/call；256-op 批 133 µs（0.52 µs/op）。16 MiB 窗全量 4,194,304 dword
debug 构建实测 **29.4 s**（release 更快），`fail = 0` ——**全量扫描完全可行且已跑通**。

有效窗口：**0x0..0x100_0000（16 MiB = 2^24）**，与 Turing BAR0 孔径一致；0x0100_0000 起即使
提权也一律 `-1`（第二道门）。Ampere+（BAR0 32 MiB）上门限大概率移到 0x200_0000，待目标机验证。
flag 0x20 第二域（TYPE_20）在 2070 上提权仍 `-1`；READ|64BIT 合法（值 = 相邻两 dword 拼接）。

16 MiB 全量分类：data 372,198（8.9%）/ zero 430,710（10.3%）/ poison 3,391,396（80.9%），
共 35,919 个 data 连续段。结构样本：0x010000、0xA0000-0xB0000、0x300000-0x3FFFFF（约 1MB
连续 85-100% data 的引擎块）、0x7B0000-0x800000（data/zero 交错）为富矿；0xF00000-0xFFFFFF
纯 poison。

安全性证据：80.9% 毒值说明内核用解码表把关——未映射地址返回**合成**毒值而非真读总线（扫完
桌面/驱动无异常）。注意存在**第二毒值族 `0xBAD0****`**（如 0x7000-0x8000 区的 0xBAD00100），
与 LACT 过滤的 `0xBADF****` 并存；通用读通道的调用方应按
`(v & 0xFFF0_0000) == 0xBAD0_0000` 统一识别。残余风险仅剩 read-to-clear 类寄存器
（中断/状态寄存器），勿在 GPU 负载下扫。

Probe：`nvapi-rs/tests/register_op_sweep_live.rs`（flag 变体 + 偏移散点，提权前后各跑一次）、
`register_op_map_live.rs`（批量语义 + 吞吐 + 边界）、`register_op_fullsweep_live.rs`
（16 MiB 全量分类 + 区域画像 + data run 列表）、`register_op_context_live.rs`
（温度窗邻域 + PMC/TIMER 语义验证）。

## 温度窗在地图上的位置与公开谱系（2026-10-19 考证）

本机（TU106/2070）提权实测定位：

| 窗口 | 位置 | 本机读值 | 语义 |
|---|---|---|---|
| LACT GDDR6x/7 模块窗 | `0x9024C0/D0` (+0x4000×partition) | poison | NV_PFB_FBPA_DQR_STATUS_DQ，DRAM MR 温度码 bits 23:16，2°C 步进；GDDR6 无 MR readout → 毒值 |
| clamshell | `0x900200` bit22 | 0x49E1003（bit22=1） | GDDR6 代语义不适用（2070 非 clamshell） |
| GB202 GDDR7 模块窗 | `0x9A24C0/D0` | poison | 同族 FBPA 块在 GB202 上的重定位（gddr6.c 5090 表） |
| Blackwell 聚合 hotspot | `0xAD0AA0` | poison(0xBADF1100) | Blackwell 专属，is_blackwell 门控正确 |
| Ampere/Ada 聚合显存结温 | `0xE2A8`（GA104 GDDR6 `0xEE50`） | 0x0 / 数据 | 热区聚合值 bits 11:0 ÷32 °C——**Ampere GDDR6 板也有**（A2000/L4/L40S 实证于 gddr6.c 表）；Turing 无此窗 |
| 0x902400 邻域 | dword 级地图 | 0x902400-47 零楔 → 0x902448 起纯毒 | FBPA 解码窗边缘形态 |

**公开谱系（发现者与旁证链）**：
1. **2020-11 → 2021-01（Windows）**：HWiNFO（Martin Malík）逆向驱动遥测，论坛开题
   （hwinfo.com "RTX 3080/3090 Memory/VRM temperature sensor"），v6.42 上线
   "Memory Junction Temperature"（初代限 GDDR6X）；Martin 确认读数来自 DRAM 硅内结温。
   GPU-Z 同期跟上。
2. **2023-24（Linux）**：olealgoritme/gddr6——逆向 **Linux 内核驱动** 得 MMIO 直读路径
   （/dev/mem + iomem=relaxed），per-card offset 表：Ampere/Ada 聚合结温 `0xE2A8`/`0xEE50`
   （bits 11:0 ÷32 °C，GDDR6 板在列），后补 Blackwell GDDR7 per-module `0x9A24C0`
   （注明 "the register the FBFALCON..."——从 FBFALCON 微码引用侧定位）。
3. **2026-07**：Paulo Gomes + 巴西 GPU 维修团队（此前公开 50 系 hotspot 传感器）以 MMIO
   逐通道映射 30/40 系每颗显存温度（hwbusters/VideoCardz 报道），桌面 GDDR6 当时仍未支持。
4. **2026-08**：LACT 0.10（videocardz 报道）——把逐芯片读移植到 **NVAPI RegisterOp
   (0x2EB3C140)** 用户态通道（libnvidia-api.so.1），base 取 `0x9024C0`（GDDR6x+GDDR7）。
   我们的移植即源于此。
5. **寄存器命名** `NV_PFB_FBPA_DQR_STATUS_DQ`（FBP partition / DRAM-Query-Readout
   Status-DQ）来自 NVIDIA priv 头文件命名族（NV_PFB_FBPA_*，SDK nvmisc.h 有
   "See FB_FBPA*" 引用惯例）；硬件语义 = JEDEC GDDR6X/GDDR7 DRAM 的 **Mode-Register
   温度读出**（2°C 步进，-40..120°C 范围由 `2·min(pc,0x50)−40` 的 0x50 帽体现）——
   传感器在 DRAM 硅内，由显存控制器经 MR readout 事务读取。GDDR6 颗粒无此 MR 特性
   → 窗在、解码器在、数据标毒。HWiNFO/GPU-Z 仅 GDDR6X/7 卡显示显存温度 = 同一结论的旁证。

**语义验证（本机 live，教科书级锚点）**：`0x0` = NV_PMC_BOOT_0 = `0x166000A1`
（芯片族 0x166 = TU106 + 版本字段，envytools/nouveau 二十余年公开文档）；
`0x9400` = PTIMER 纳秒计数器（120 ms 间隔 delta = 120,574,432 ≈ 1 ns 精度严丝合缝）。
这两点证明寄存器通道读的是真实 priv 空间。

**我们移植的校准项**：GB202 上同一 FBPA 块在 `0x9A24C0`——已在
`Gpu::vram_chip_temps` 增加 GDDR7 双 base 回退（先 LACT base，无有效模块再试 GB202 base）。
后续待办：Ampere/Ada 机器上接 `0xE2A8` 聚合显存结温（GDDR6 板也可用，超出 LACT 面）。
