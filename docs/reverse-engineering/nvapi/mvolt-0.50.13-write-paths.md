# mVolt+ 0.50.13 写路径逆向（2026-11-14，50 系电压墙调查 Part 4a）

- 样本：`reverse/mvolt+/mVolt-0.50.13.exe`（SHA256 `b92735bf…`，release v0.50.13，2026-11-08；自报版本 0.50.13；内嵌字符串另见 2.0.15/2.0.2 痕迹的是 xOCD，与本样本无关）。
- 工具：idalib headless（session `mvolt0513`，imagebase 0x140000000，自动分析 3115 串）。
- 结论速览：**0.50.13 有三条写车道**——①melonVolt control SET（与我们 nvapi-rs 同几何同槽位语义）；②BIOS-override 存储上的「实验性 Voltage offsets」（621.01+ 的 README 承诺即此道）；③功率/F8 租约持久化（power_command 家族）。**「driver did not retain…」失败态对 mVolt+ 自己也是已知常态**（专有报错串），这正是②存在的理由。

## 1. 车道一：melonVolt control SET（sub_140206740 / sub_140207A30 / sub_140208870）

- GET 构建器 `sub_140206740`：stamp **0x20AC8 = (2<<16)|2760**（V2，无 V1 fallback——50 系专属工具形态）；`a4[v14+18]` 播种 type（dword 18 = 字节 +72），dense 步进 21 dword = **84 B**（与 nvapi-rs `ctrl_entry::STRIDE` 一致）；越界检查 `v14*4+76 <= 0xAC8`。
- SET 包装 `sub_140207A30`：入口校验 `*(u32*)buf == 133832 (0x20AC8)` 且 `buf[4] & 1`（mask 非零）→ 拒绝时 "Refusing to apply an invalid rail-control object"；接口不可用 → "The private voltage write interface is unavailable."。fnptr 存于对象 +4264/+4256（SET/GET），handle @+4288。
- **NVVDD/MSVDD offset apply `sub_140208870`**（rail 0/1，`a2 > 1` 即拒 "outside mVolt+ bounds"）：
  - 前置链：info(6220B, 0x2184C@0x1402043a2/0x140206619) → control GET → status(5664B, 0x21620@0x140206a7a) → 合成记录（132B/轨：+0 rail bit、+8 writable、+12..28 六值、+72 type、+80/+84 窗、+116.. OV 描述）→ `sub_1402075E0` 校验。
  - **patch 循环**：`buf[84*dense + 72 + 4*(slot_map[j]+1)] = value[j]`，映射表 **unk_1403ACCA0 = [3,0,1,2]**。对照调用参 a3=[VMIN, ALT/OP, REL, OV]：
    | mVolt+ 抽象槽 | melonVolt payload dword | 我方 P100/582.41 钉定语义 | 一致性 |
    |---|---|---|---|
    | VMIN (a3[0]) | **slot 3** | VMIN/min-hold（status[5] 跟随） | ✓ |
    | ALT/OP (a3[1]) | **slot 0** | µV operating offset | ✓ |
    | REL (a3[2]) | **slot 1** | VBIOS max wall offset | ✓ |
    | OV (a3[3]) | **slot 2** | VRM max wall offset | ✓ |
    一次 SET 写多 slot；`a9=0` 时只写 OV（起点 3）；reset 模式全写。
  - **失败态即我方症状**："The driver did not retain the requested rail offsets, or changed another voltage setting."——mVolt+ 也靠读回验证，驱动丢写时同样可见。
  - OV 顶：默认 **1150000 µV (1.15 V)**；`a5`(XOC range) 时 **1250000 (1.25 V)**；`rail ≥ 2` 直接拒（"OV adjustment requires a writable voltage-rail record"）。收尾窗检："The resulting voltage limits exceed the active mode ceiling or conflict with VMIN."。
  - **分派**：`if (*(a1+4456)) → sub_1402492F0`（实验道，见 §2）；否则走本道。SET 失败或读回不符 → `sub_1402094B0` 回滚。
- Boost 百分比 `sub_1402085C0`：能力位走 40B/0x10028 旧面；否则**缓存 control 块整体复制 + 单字节补丁（buf[8]）→ SET**。

## 2. 车道二：实验性 Voltage offsets（sub_1402492F0 → sub_14028C6F0 → sub_14028C390）

- 硬门（进入即查）：`!HIBYTE(v89)` → **"Voltage offsets require a verified BIOS override and Voltage Boost at 0 %."**——readback 状态字节（sub_1402076F0 产出）编码「BIOS override 已验证」；另有 "Voltage offsets are unverified or a previous voltage change needs recovery."（busy/恢复态门）与身份重验（"The original voltage-rail identity no longer matches."）。
- `sub_14028C6F0`：范围校验对 **BIOS 允许窗**（"The voltage offset is outside the range this BIOS allows."，窗值取自 BIOS 派生表 v15+40+4*idx 的 [min,max] 对）；写 40 B/条目的 override 存储（+27 min、+28 max、+37 applied 标志、+38 值）→ `sub_14028AEF0` 持久化 → `sub_14028C390` 应用。
- `sub_14028C390` = **读-比对-收敛循环**：读 blob（sub_14028B700）→ 与目标 memcmp → `sub_14028A6C0`（单步 apply 传输层，**未逆向**）→ 连续 **2 次**一致=成功；步间 50 ms（fnptr @+1296），≤12 轮。
- 语义：README "Dashboard voltage limits on 621.01+ specifically require these experimental Voltage offsets" 的实现体——**BIOS override 先行（把墙抬上去），实验道再把运行时 offset 面救活**。

## 3. 车道三：功率/F8 租约持久化（sub_140256420）

- Power-cap ownership/recovery 文件（`.bak` 回退读）：magic 串 `MVOLT_POWER_LEASE_V4/V3/V2`（20 B）与 `MVOLT_F8_LEASE_V1`（17 B）；租约字段校验（通道号 <0x20、互异、值≠-1、mode 1..2）；适配器身份绑定比对。对应我方 power_command（0x33AB0353/0x17695269，0xF8/0xFE）§18.8 认知。
- compat 报告仍输出 `"setters_resolved_but_never_called":true`（@0x1403744a0）+ `SET_NOT_CALLED` 表（8 处引用 @0x1403747xx）——**报告器仍是 GET-only**，写逻辑全在 GUI 主程序。

## 4. 对 5060 Ti / 621 失败的解释力（当前证据链）

1. 我方 `set-volt-rail-limit` 全 slot 写被驱动接受但读回不保留（写 0 平凡过，非零全丢）。
2. mVolt+ 同车道同几何，报错串证明「不保留」是其已知的失败形态。
3. mVolt+ 的 621.01+ 答案不在 melonVolt 道，而在 **BIOS override + 实验 offsets**。
4. ⇒ **H3（Blackwell/621 不消费 melonVolt control 写）获得强佐证**；H1（我方几何 bug）被反证削弱——mVolt+ 的几何/槽位/戳与我们完全一致；H4（5664B 大形态）未获支持（0.50.13 全线用 2760B V2）。
5. 待探针实机闭环：C 臂全缓冲 diff（写是否落进任何字段）+ D 臂镜像初始化。若 C 臂 diff 为空 ⇒ H3 机上实锤。

## 5. nvoc 落点与开放项

- **立即可做**（不依赖 BIOS override）：`set-volt-rail-limit` 多 slot 联合写（对齐 mVolt+ 的 VMIN+OP+REL(+OV) 一次 SET）+ OV 顶 1.15/1.25 V 语义 + "did not retain" 诚实报错（已落，见 nvapi-rs b1e6b82）。多 slot 联合写是否比单 slot 更易被消费 = 探针 D 臂可判。
- **墙解锁本体**（5060 Ti/621 的真正抬墙）：BIOS-override 车道——开放项：
  1. `sub_14028A6C0`（apply 传输层）：blob 如何被驱动消费（vBIOS 镜像替换？注册表？D3DKMT？）——下一段 RE 的主目标；
  2. override blob 格式（sub_14028B700 读 / sub_14028AEF0 写）与落盘位置；
  3. `sub_1402076F0` 的「verified BIOS override」判据（HIBYTE 字节）；
  4. `a1+4456`（实验道开关）的置位条件（驱动版本探测？）。
- 备选对照：xOCD 2.0.9 车道（Part 4b，待做——其 2.0.0 审计显示墙类抬升走内核 pmxdrv；nvoc 已嵌入同款驱动）。

## 6. 方法复现

idalib 打开 exe 后按戳常量字节搜定位实现区：`C8 0A 02 00`(0x200AC8 control) / `20 16 02 00`(0x21620 status) / `4C 18 02 00`(0x2184C info) → 命中 0x140204xxx–0x140207xxx → 沿 xref 找 apply/调用方。splitmix 常量在本版未以立即数出现（0x10AC8/0x10ACC V1 面整体缺席），旧 decode_ids.py 的表基址已漂移（compat 表 @0x140374xxx），未再花时间重定位——写路径经戳常量路线已全覆盖。

## 7. 车道二真身定案:PerfVFE 方程/变量引擎(2026-11-14 第二轮,六 ID 全破)

- **解码器**:0.50.13 把接口 ID 加密为 18B splitmix64 密钥流 VM 程序
  (`sub_1401D2EE0`:seed = key|0x891F610E00000000,ks(p)=fm64(seed+GG·p)>>56,
  opcode 0xC1=MOV/0x02=XOR/0x43=ADD/0x84=ROL/0x4F=RET,imm32 逐字节同流)。
  破解脚本 `reverse/mvolt+/decode_override_ids.py`(自检:slot 161 必解出
  0x2C73AFDC,精确命中)。**坑:0x84+0x3D=0xC1 是 MOV,首版手算 0xBF 白跑一轮。**
- **填充点 `sub_14028B190`**(引擎构造):六槽 = QueryInterface(解码 ID):

| slot | ID | NVAPI 名(nvid.rs 已注册) | RM cmd | mVolt+ 用途 |
|---|---|---|---|---|
| +1248 | 0xB9DA41D6 | PerfVfe**VarGetInfo** | 0x2080A0B1 | 2,856,296B 变量表(BIOS limit 真身) |
| +1256 | 0x5D387298 | PerfVfe**VarGetControl** | 0x2080A0B3 | 40,904B 变量控制态 |
| +1264 | 0x79FA23A2 | PerfVfe**VarSetControl** | **0x2080E0B0** | **override 推送道(EXEC)** |
| +1272 | 0x8D49471C | PerfVfe**EquGetInfo** | 0x2080A0B5 | 623,684B 方程表 |
| +1280 | 0x4C75C9FE | PerfVfe**EquGetControl** | 0x2080A0B6 | 1,410,116B(+1024B 身份) |
| +1288 | 0x2C73AFDC | VoltVoltRailsGetInfo | — | 6,220B 轨描述符(已知) |

- **传输层 `sub_14028AEF0`**(通用 blob 传输):buffer+64B 头,0xCD 毒签填充;
  a4=0 走 GET 直调,a4=1 走 `sub_1401D32C0(cmd=28, fnptr, handle, buf+64, len)`
  参数化 SET;调用后毒签校验(完整性哨兵)。**`sub_14028A6C0` 是纯校验器**
  (132B 记录比对+步进窗插值+OV 顶 1.15/1.25V),非传输。
- **收敛循环 `sub_14028C390`**:Var/Equ 表读-改-推,连续 2 次读回一致才算成,
  50ms/轮,≤12 轮。**"verified BIOS override" 门 = 表身份校验**
  (sub_1402898B0/BE0,1024B 身份块匹配,不符报 "installed voltage offsets do
  not match this GPU")。
- **结论**:mVolt+ 的 621.01+ "BIOS override" = **PerfVFE Var/Equ 表改写**——
  驱动侧 vBIOS 策略镜像(与 core/src/vbios/vfe.rs 的 vBIOS 侧 VFE 脚本同族,
  语义可对照);乘 boost-0% 门与毒签/身份/收敛三重防护。**5060 Ti 抬墙的
  用户态可行道 = Var SetControl(0x79FA23A2/RM 0x2080E0B0)——nvapi-rs 已注册
  ID、读三件套已封装,唯一缺口 = SET 写封装 + mVolt+ 协议(64B 头/毒签/收敛)。**

## 8. 5060 Ti 跨驱动 A/B 实机判决(2026-11-14,用户实测 610.88 vs 621.01)

2×2 矩阵收官:**杀招 = 621 驱动对 Blackwell 的针对性策略门**(非硬件特性):

| | 610.88 | 621.01 |
|---|---|---|
| 2070 (Turing) | ✅ | ✅ |
| **5060 Ti (Blackwell)** | **✅ slot 写生效** | ❌ 全丢写 |

610.88 实测链(5060 Ti):slot0/1 各 +300mV → Control values 保留 [300000,
300000,0,…] ✓;VBIOS 墙 1020→**1280**;Offset Ceiling 20→**300mV**
(status[1] 与 ceiling 双随动,slot1=REL 语义再+1);实测电压仍钳 ~1075mV =
**功率墙是上一级钳**;抬功率墙后电压锁可达 **1.145V**——墙→功率→电压逐级
解锁链完整。

⚠️ **安全发现(50 系特有)**:同抬 slot0/1 后功率百分比读数异常(60%→100%+);
抬功率墙 + 电压锁 1.145V 下**空载 ~230W 真实功耗(温度热力学实锤)**,解除
电压锁即回落——**电压墙修改会把 50 系驱动的功率管理模型打出校准失效,功率
执行层不再正确钳制**。推测即 NVIDIA 在 621 封道的动机,亦解释 mVolt+ 的
boost-0% 硬门与 Speedo 独立四态管理。

**精确边界(用户温度观测补刀)**:缺陷态触发条件不是电压锁本身,而是
**工作电压越过默认 VBIOS 墙(1.02V)**——锁定 ≤1.02V 待机功耗完全正常;
>1.02V(哪怕合法抬墙后的合法区间)→ 卡无法下探待机态(DVFS/P-state 以电压
域为键,墙外电压无合法 idle 映射),整卡高活动空转 ~230W。621 的 boost 100%
恰钳 1020mV=墙顶即驱动层刻意保证工作电压永不越墙。**nvoc 红线:50 系上
任何抬墙操作必须默认拒绝把工作电压推过 stock VBIOS 墙(--force 旁路 +
空载功率看门狗 + 自动回滚);VFE 车道实装同此约束。**

**GPU-Z 仪测快照(锁 1.145V 空载)**:Board 247.7W / GPU Chip 95.3W /
16-Pin 201.6W + Slot 46.1W → **152.4W 蒸发在 VRM/板级(效率仅 38%,正常
85-92%)** = VRM 相控病理态实锤(非软件读数问题);PerfCap Reason=VRel
(1.145V = 电压可靠性钳,非 VRM 墙 1.2V),空载时 DVFS 仍在要压(状态机坏,
VRel 兜底)。完整越墙阶梯:抬 VBIOS 墙→放功率墙→电压爬至 VRel 1.145V→
越 1.02V 即 VRM 病理态。**越墙 = 电气级缺陷,NVIDIA 621 封道动机实锤。**

**显存排除(用户 P5 实验)**:显存锁 P5 后功耗与比例完全不变 → 152W 损失
不在 GDDR7/MSVDD 侧,定案**核心 VRM 相控病理态**(芯片 95.3W + 相控损失
152W + 时钟停高位 = 「疯狂空跑」的完整机理)。**双域排除完成**:显存锁 P5
无效 + 核心降频 500MHz 无效 → 247.7W 与两个频率域活跃度完全解耦 → VRM 侧
固定损失(相控调制失效,功率级定态驱动,开关损耗与负载无关)。

对 nvoc 车道的影响:621 兼容路径 = PerfVFE Var SetControl(已封装
set-vfe-var);610.88 用户可直用 melonVolt 车道(set-volt-rail-limit)。

## 9. V2 大戳层协议破译（2026-11-14 第三轮，probe vfe_v2_tier_probe 2070/621.01 实机）

- **戳阶梯定案**：171976=0x29FC8=(v2<<16)|40904（VarGetControl 大层，此前手误作 0x2A008）；五 blob 戳全部落在 sys 层已记录的 magic 阶梯上（mVolt 用最大 tier：VarGetInfo 3118440/2856296B、EquGetInfo 885828/623684B、EquGetControl 1410116B）。
- **Var V2 请求布局**：stamp@0 + **32B 选择掩码@+4**（非身份！mVolt SET 组包 `byte(4+idx/8)|=1<<idx%8`，restore 函数同款单 bit）+ 记录区@36（**160B/记录，按 RM 位号索引**，非线性）；记录内 float[27]=min(+108) / float[28]=max(+112) / byte[148]=applied / float[38]=value(+152)——与第一轮 40B 条目 RE 完全互证。
- **slot→var 映射表 dword_1403C5D60=[3,0,1,2]**（4 项，后接 "Cannot enable the protected inte…" 串）：`var_slot = 4*rail + map[slot]` → NVVDD={3,0,1,2}、MSVDD={7,4,5,6}，8 变量 ✓（restore 的 v17<8 门）。
- **SET 保留前置 = 身份块**：零身份 GET/SET 全部骨架/接受即弃（256 位盲扫全不保留）；**小戳 EquGetControl(85016) 响应的 1024B 区块（@+0 起）回喂 V2 → EquGetControl 1410116B 丰富度 701 dword（vs 零身份 0）**——V2 大层是有身份门控的真实消费面，GET 通路已打通。
- **未决**：①Var V2 用 32B 身份切片仅 richness 9（弱信号）——Var 的身份形式/粒度待定；②SET 通路的身份携带方式（SET 请求 +4 是掩码，身份在哪个区？）；③small-tier 响应里身份块的精确边界（@+0 起 1024B 是粗定位）。
- **下一步**：①medium 层加身份自举（small-tier → identity → V2 GET）→ 身份认证版 get-vfe-tables；②2070 上对身份版 Equ V2 dump 做墙值对齐（1125/1093.75/1068.75/712.5mV）→ 语义图；③SET 带身份重试保留实验。
