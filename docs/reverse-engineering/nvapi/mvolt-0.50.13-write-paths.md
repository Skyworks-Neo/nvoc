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
    | ALT/OP (a3[1]) | **slot 0** | µV operating offset | ✗ 勘误见 §12 |
    | REL (a3[2]) | **slot 1** | VBIOS max wall offset | ✗ 勘误见 §12 |
    | OV (a3[3]) | **slot 2** | VRM max wall offset | ✓ |
    > **§12/§13 勘误**：本表构造时假设 a3 序 = [VMIN, ALT/OP, REL, OV]；round-5/6
    > 槽名表 off_14037ADF0 + CLI `--offsets VMIN,REL,ALT[,OV]` 定案 staged 序 =
    > **[VMIN, REL, ALT/OP, OV]**，patch 循环 `value[j]→payload[map[j]]` 下
    > **payload0=REL / payload1=ALT/OP / payload2=OV / payload3=VMIN**（0/1 对调）。
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

## 10. 事故档案:语义扫描致内核死锁(2026-11-14,probe 3f1de28 前的运行)

- **事件**:19 var 连续 applied=1 微扰扫描(每 var SET→读墙→还原,250ms 间隔)
  运行中 GPU 内核态死锁——黑屏卡死,**事件查看器无 TDR(4101)记录** = 死锁在
  TDR 看门狗可见性之下(RM/GSP 策略线程互锁形态),只能重启恢复。
- **根因**:SET 保留已实证(驱动真实消费 VFE Var override),但 19 个变量的
  语义完全未知——其中 8/9/11/14-18 为 ±FLT_MAX 无界策略变量(type 2/3),对
  无界策略变量写 applied=1 + 微扰值,等于把未定义值灌进活的策略引擎。
- **定案**:①VFE Var SET 是**活体消费面**(不是回显存储)——写=改变驱动正在
  执行的策略;②语义未知变量**禁止盲扰**;③死锁无 TDR = 靠 nvidia-smi 黑屏
  症状人工判定,无法依赖系统自动恢复。
- **修订安全协议**(取代此前的扰动标定计划):
  1. 语义映射唯一路线 = **RE**(Equ 表结构分析、mVolt 流程逐步复刻),禁扰;
  2. 任何写实验:单变量、单会话、值 = 工厂等价值或经 RE 论证的合法值;
  3. 5060 Ti 额外红线:VRM 缺陷态(>1.02V)——电压类写必须先有墙值对齐证据;
  4. 写实验前声明恢复方案(重启),且不接受"还原了所以安全"——还原与扰动
     之间驱动的异步策略求值可能竞争。
- **事故前已固化成果**(不受影响):工厂目录 19 变量(var13 type9 450000..1243750
  µV = 电压类候选;var10/12 0..105 功率类;var7 1981.23..2335.54 频率类;
  ±FLT_MAX 8 个 = 偏移类)、SET 保留配方(完整工厂记录 base)、身份自举、
  协议公式全链。

## 11. 槽名表定案 + INFO 层名字 ID(2026-11-14 第五轮,纯 RE 路线)

- **staged 槽名序 = [VMIN, REL, ALT/OP, OV]**(off_14037ADF0 宽字符表,与 mVolt
  CLI "--offsets VMIN,REL,ALT[,OV]" 参数序一致;sub_1401EF1E0 验证链
  effective=min(OP, REL, OV, mode-limit) 逐槽报错同序佐证)。
- **VarGetInfo 大层(0x2F9568)掩码版 GET**:+4=掩码(0x7FFFF=19 位回显),
  目录 19 记录 @296 stride 276;entry5/6 携带 **RM 名字 ID 0x1613/0x1614**
  (5651/5652,u16,与 Equ name 0xFF0B/0x2413 同族)——**var→函数映射的权威键
  = RM name ID**,映射表在 RM 侧(nvlddmkm/envytools),为下一段 RE 目标;
  无 ASCII 名字(整 2.86MB 零字符串)。
- 工厂目录 19 变量(2070/621.01):var0 0..127 / var1 54..67 / var2 0..31 /
  var3 ±FLT_MAX / var6 60..60 / var7 1981.23..2335.54(频率域 float)/
  var10/12 0..105(功率)/**var13(type9) 450000..1243750 µV(电压域,唯一
  绝对电压候选)** / 8 个 ±FLT_MAX 偏移类。与 staged 8 变量的对位
  (engine+28 记录号)仍需 RM name 表或 mVolt staged 记录结构 RE。

## 12. 语义映射修正 + 每代际 var 索引问题(2026-11-14 第六轮)

- **payload 槽语义修正(权威源 = mVolt 槽名表 + UI 串)**:melonVolt payload
  **0=REL**(reliability)/ **1=ALT/OP**(operating, Vop)/ 2=OV(VRM)/
  3=VMIN——与 P100 钉定的 [OP, REL, OV, VMIN] **0/1 对调**。交叉验证:5060
  Ti/610.88 slot0+slot1 各 +300mV → REL 1320 → 被钳设备顶 1280mV ✓。CLI
  `set-volt-rail-limit --slot` 语义表需按此修正(slot0=REL、slot1=ALT/OP)。
- **值语义**:offsets 以 mV 存储(字符串 "stored as offsets... limits can
  shift with GPU conditions");applied 记录 min=fmin(v,0)/max=fmax(v,0) 与
  此一致(偏移可正可负)。
- **UI 语义串实录**(mVolt 全串收割):VMIN="rail's minimum voltage limit,
  GPU can still use more voltage under load";REL="reliability voltage
  limit, one of the rail's maximums";ALT/OP="maximum operating voltage
  limit (also called Vop), driver decides when this limit applies";OV=
  overvoltage;Speedo 有 original/effective 双读数。
- **621 封道 mVolt 侧文案**:"NVVDD/MSVDD offsets were skipped; this driver
  ignores them" / "voltage offsets are not verified yet"。
- **每代际 var 索引问题(新开放项)**:8 电压变量在 V2 var 空间(255 槽)中的
  索引是每代际运行时配置(mVolt staged records 经 BIOS override 验证流推导),
  2070 的 var0..7 ≠ 5060 Ti 的电压变量位。**解法 = 5060 Ti 上跑探针的目录
  扫描(GET-only,零写,安全)**——单 var GET 已全栈可用,工厂 min/max 值域
  + mVolt UI 语义串 + 值域相关分析即可无写定案映射。

## 13. 5060 Ti 目录扫描 + var→role 映射定案(2026-11-15 第七/八轮,全 GET-only)

探针 `vfe_v2_tier_probe` 第七轮(45648f6)/第八轮(7374909):5060 Ti 与 2070/621.01
双机 GET-only 对照。**零写入**;5060 Ti 采样时活体墙 = melonVolt payload 面
+200mV 已消费态(VBIOS 墙 1220/ceiling 200mV/min-hold 625),VFE override 面
全部 applied=0。

### 13.1 几何定案(双机互证)

- **160B 工厂记录层 @104+160·idx(round-4 SET 配方几何)跨代际成立**:单
  var+身份 GET 中选中 var 的 type@+0/min@+4/max@+8 回填 round-4 位置
  ——2070 var0@104(type7/max127)、var1@264(7/54/67)、var2@424(7/31) 三点
  实锤,5060 Ti var0@104(type13/max127) 同构。**50 系工厂 type=13(=SET
  请求型),2070=7 → 50 系 GET 回的记录天然 SET-ready**。
- **76B 活值槽层 @136+76·k 内嵌于记录区**(与 160B 网格字节共存、字段不冲
  突):槽字段 [type@0, min@4, max@8, value@44];0xFFFFFFFF=空槽。var k 的
  槽=k2k/k2k+1(var0→k0,k1;var1→k2,k3;…var5→k10,k11)。**2070 全部空槽
  (NaN);5060 Ti k0-k6 有实值**:810000/900000/0/900000..1100000(type6)/
  20000/700000/1200000(+k7 ±FLT、k9 51000、k10 -FLT..0)——值集=工厂快照
  语义(900000=VRM default、700000=descriptor B、20000=stock ceiling 20mV、
  1200000=OV 墙),**墙现值(1220/1020/625/635)不在 VFE 层**。
- 身份块 @100/176/252/328/404 各授权不同槽基(@188+76k 等,2070 上全空);
  ident@0(=small[0..1024]) 为唯一 rich 形态(5060 Ti 58/821,2070 28/701)。
- info 层掩码语义不同:零掩码 GET 回显 populated 位掩码(2070=0x7FFFF=19
  位,5060 Ti=76 位);掩码位不影响条目集。RM name ID:2070 entry5+0xF0=
  0x1613/entry6+0x0=0x1614(reproduced),5060 Ti entry5+0xF0=0xD0A(3338)。
- bare V2 GET @36 单 dword:2070=200/0xC8,5060 Ti=20/0x14(含义未决)。

### 13.2 var→role 映射定案(mVolt 模型 + 界值代际互证)

mVolt slot→var 模型 `var_slot = 4·rail + map[3,0,1,2]`(staged 序
[VMIN,REL,ALT/OP,OV])+ 界值跨代际比对:

| var | role | 2070 界(type7) | 5060 Ti 界(type13) | 判据 |
|---|---|---|---|---|
| **var0** | **NVVDD REL offset(VBIOS 墙 ↑)** | 0..127 | 0..127 | 界代际稳定;2070 SET var0=6250µV 影响活体电压(dec4617) |
| var1 | NVVDD ALT/OP offset | 54..67 | 0..511 | 界漂移,角色一致 |
| var2 | NVVDD OV offset | 0..31 | 0..31 | 界代际稳定 |
| **var3** | **NVVDD VMIN offset(±,降压道)** | ±FLT | ±FLT | ±无界=offset 类 |
| var4 | MSVDD REL offset | 0..0 | 0..0 | 零余量=无头寸 |
| var5 | MSVDD ALT/OP offset | 0..0 | 0..0 | 同上 |
| var6 | MSVDD OV offset | 60..60 | 0..5 | 有界小顶 |
| var7 | MSVDD VMIN offset | 1981..2335(freq,2070 异常位) | ±FLT | 50 系归位 |
| var13/var45 | OV 包络(绝对 µV) | 450000..1243750 | 450000..1250000 | 1250000=1.25V=XOC OV 顶 |

- var13/var45(type9 µV)**不在 8 变量组**=OV 包络变量(绝对电压域)。
- type=8 百分比类:2070 var10/12(0..105,extra +0x78/+0x7C=0x500);
  5060 Ti var15/16/22/44/54/73(−5..105,extra=0x100)——50 系多 4 个。
- 5060 Ti populated=76/255,频率=var14(2049.34..2292.46)。

### 13.3 621 写实验设计(待执行,单变量单会话)

- **首个写 = var0(NVVDD REL offset)applied=1 value=+20000µV(+20mV)**:
  RE 论证链 = mVolt 模型 var0=REL + 界代际稳定 + 2070 SET var0 活体消费
  实证 + +20mV 与 stock ceiling 语义对齐。验证 = `get-volt-rail-info`
  VBIOS 墙 1020→1040 + status[1] 跟动 + var0 记录回读 applied/value。
  还原 = applied=0 或工厂记录 SET 回。
- **降压道 = var3(VMIN offset)负值**——不触 VRM 缺陷态(工作电压只降不升),
  可作 621 消费面的零风险先行验证。
- 红线不变:50 系不把工作电压推过 stock 墙 1.02V(--force + 空载功率看门狗
  + 自动回滚);还原≠安全(异步求值竞争,§10)。
- **驱动版本定案(V8-0,2026-11-15):5060 Ti 采样机 = 610.88(r610_85)**
  ——两轮 5060 Ti VFE dump 全为 610.88 态;621 的 GET 基线未采。**写实验
  必须在 621 上做**(610.88 melonVolt 直写已够,VFE 道只对 621 有意义)。
- **V8 判读(5060 Ti/610.88)**:①槽层=工厂快照表,ident 块=槽授权切片,
  响应表按身份旋转起始槽(ident@0 基 136/@100 基 36/@176 起始 r1/@252,328
  起始 r3/@404 起始 r4);Equ 层多一条 Var 层没有的 r8=510000;②**V8-3 单
  var 记录内嵌槽实拍:var k 拥有槽 {2k,2k+1} 跨双机成立**(var0→k0/k1、
  var1→k2/k3、var2→k4/k5、var3→k6/k7、var4→k8/k9、var5→k10/k11);k10 携带
  101.0、k11 携带 1.0(百分比/标志类新字段)。
- **写臂 v4(8395929)**:门分离——var0 SET 臂 = `NVOC_ALLOW_VFE_VAR0_WRITE`
  (+`NVOC_VFE_VAR0_VALUE_UV` 默认 20000),DANGER sweep 留在
  `NVOC_ALLOW_VFE_WRITE` 永不共门;流程=工厂记录 base→patch
  min/max/applied/value→SET→读回+墙值三查(before/mid/after)→工厂 SET 回
  →还原复核。

## 14. offsets 车道全链 RE + 621.24 写实验(2026-11-15 第九轮,commits 5ed7a67)

### 14.1 621.24 写实验判决(实机,GET-only + var0 单写)

- **621.24(r621_20)VFE Var SET = RETAINED**(applied=1/value=20000µV 读回一致
  ——621 策略门不挡 VFE override 面,只挡 melonVolt payload 面);**但 var0
  override 未被消费**(墙 [1000/1020/1200/635] 全 stock 纹丝不动,且未越权)。
- 621 GET 层差异:**全掩码 Var V2 GET 返回 −1 但槽表内容仍写入缓冲**;Equ
  rich GET 821→56 塌缩(深层方程区被拒);工厂目录/单 bit GET/槽表内容与
  610.88 完全一致,621 槽表更完整(r6 type 回填/r8=510000 出现于 Var 层)。
- 切 621 后 melonVolt +200mV 消失(walls 回 stock)——610.88 态不跨驱动。

### 14.2 mVolt apply 流全链 RE(93c20dca session,C6F0/B700/9BE0/C390/A6C0)

- **收敛轮询 C390**:B700(读)→ memcmp 目标 → A6C0(校验)→ 50ms×≤12 轮
  → 连续 2 次一致才算成。**A6C0 = 逐 dword 校验器**(132B 步进 staged 记录,
  4 dword payload @+68..84,只允许全局 dword 偏移 4·ID+idx 一处变动,步进
  量化界,OV 顶 1150000/1250000)。
- **B700(读)**:VarGetControl(171976,**请求带 32B 身份@4,来自 mVolt 对象
  +184**) + 第二对象 137292(6220B) 双 GET → **98B0 身份校验**("The installed
  voltage offsets do not match this GPU.");深模式加 VarInfo/EquInfo/
  EquControl(1024B 身份@4)→ 9BE0 全表校验;boost 状态检查("require Voltage
  Boost at 0 %",比较 6220B 对象 +4)。
- **C6F0(写)**:逐 slot 串行(每轮 B700 GET → 单 var patch → SET → C390
  收敛)。**patch 几何 = float* 索引 40·v+27/28/37/38 = 字节 160·v+108(min)/
  112(max)/148(applied)/152(value)**——与 round-4 配方逐字节一致!掩码同样
  @4+v/8;min/max=本步增量(±clamp),value=累计 offset。**var 索引来自运行
  时表(对象 +28 步进 20×8 字节,enable@+40 同步距)**,由安装/验证流锚定。
- **9BE0(全表校验)暴露的 VarGetInfo 真结构**:**255 条 × 11200B @296**
  (296+255×11200=2856296 整);电压 offset var 签名 = **type==13 &&
  byte[+1620]==7 && byte[+1621]==8**,+260 起 336B = per-var 特征区(同 rail
  var 互相比对),info+4 与 control+4 32B 互证;EquControl @76·equ+1092 的
  +4/+6 u16 = var 关联对,@172·equ+1092 有 (min,max,0,1,0) float 组。
  早期"19 条目 @296 步进 276"解析 = 在第 0 条记录内部打转(误读)。

### 14.3 待办(下一段)

1. **5060 Ti 跑 V9-1 扫描**(GET-only)→ 按签名+关联对定 8 个 offset var
   的真实索引与顺序(2070 端 rec13=type9 与 var13 1:1 已证 info↔control
   索引同构)。
2. 用正确 var 索引重做写实验;若仍存而不消费 → 复刻 mVolt 全链(身份版 GET
   → verbatim SET → 收敛轮询),重点补 **32B 身份@4**。
3. var 索引表锚定流(install 时谁填对象+28)待挖(28AE30/28A900/28AA10)。

## 15. 热面 SET 全链 RE 定案（2026-11-15 第十八轮，T6c -9 判决触发）

### 15.1 T6c -9 根因：124B 结构体不是驱动载荷

- T6c（124B ver1 载荷 + SetControl）5060 Ti 返回 **-9（NO_IMPLEMENTATION）**
  → 回 mVolt IDA（93c20dca session）复核，定案：**sub_140219C90 的 v42
  124B 结构体 = mVolt 内部"期望状态"验证模板**（六条 20B 记录，header
  byte0=1/byte1=(state==1)，仅用于 apply 后两次 SET-marker GET 回读比对
  "did not retain"），**从不发给驱动**。sub_1401F4D20 也不是 push——是
  UI 状态标签函数（"GPU interface <name>"/": recovered"，写 obj+2288+32·idx）。

### 15.2 真实 SET 协议（sub_140219B60 / 14021A360 / 1401D32C0:22 全链）

- **GET/SET 共用同一 10444B 自描述缓冲、同一两参调用形
  `fn(handle, buf)`**（mVolt 调用处证实 2 参；probe 的第 3 参被忽略）。
  缓冲自描述：ver@0=0x328CC(207052)、mask@+8（GET 请求：obj+28 缓存，
  0xFFFF 失败重试 **63**=0x3F）、**六映射通道 dword_1403746B0=[1,3,4,5,2,0]
  的 entry+8 请求期写入 marker**：GET=0x13579BDF / SET=0x2468ACE0，
  驱动对未设置槽**回显哨兵**（=0x2468ACE0 即"无 staged 值"）。
- entry 布局（76+108·ch）：class@+0（合法 1..5）/ **applied@+4（byte∈{0,1}）**
  / value@+8（Q8；GET 视图=激活 sim 值，SET 视图=staged 或哨兵）。
- **mVolt apply（sub_14021A360）**：SET-marker GET prime → 逐输入 patch
  （12B 请求 {en_a,en_b,…,applied_src@4,value@8}，en_a&&en_b 才纳入）→
  **mask@+8 = 触达通道位图（⊆0x3F，case 22 客户端校验
  `(mask & 0xFFFFFFC0)==0`）** → SetControl（写面函数指针）→ 两次
  SET-marker GET 回读比对（"did not retain"/"unrelated control changed"
  ——entry+44 byte 是驱动侧变更计数）。
- **写面/读面是两个函数指针**：mVolt 热面对象 obj+0=handle、obj+8=读 fn、
  obj+16=写 fn（obj+16 为空即 "Thermal input control is unavailable"）。
  与 nvapi_QueryInterface(0xA933CE98/0x8DF19FA2) 两参 FFI 同构。
- **sub_1401D29A0 布局校验器**（GET 响应与 SET 缓冲共用）：len==10444、
  ver==0x328CC、dword@12/16/20/24/28/32/36 全零、mask@+8 高 16 位为零。
  （probe 旧 T6 的 mask=0x1cfff 过校验 → SET=0 但语义字段不对，default
  态不消费。）
- 布局校验也解释了 mVolt 版本探测：**id 0x8DF19FA2 作为 GET 用返回 -121**
  （T3c），写面不受理 GET 视图请求。

### 15.3 T6d（probe v7，SHA256 77aa3394…32ada2）

- 配方：SET-marker GET prime（mask=63 优先回退 0x1cfff）→ 哨兵回显计数
  → 触达通道 applied=1+Q8、未触达 sentinel→0 → mask=1<<ch → SetControl
  → 三面验证（Status 跟随 / SET 视图 retain / GET 视图激活）→
  还原（applied=0+value=0，mask 同位图）。
- **5060 Ti 实测判决（2026-11-15，thermal-t6d.txt）：CONSUMED 实锤**。
  default 态（同场 T6 旧配方 no-follow、T7 时 Status 28.x 真温度漂移）
  `SET=0 [mid] status ch1 = Some(30.0) <— CONSUMED`，retain 精确
  （applied=1/value=7680 读回一致），restore（applied=0+value=0）回
  28.11°C 真传感器——**nvoc 配方无 mVolt 自足，"独立武装位/总开关"
  假说否定：applied@entry+4 + mask 触达位图就是武装本体**。附带收获：
  mask=63 st=0（mVolt 重试链终态确认）；**sentinel 回显 0/6**（621.24
  不回显 0x2468ACE0，GET prime 返回全零——mVolt 的哨兵判"未设置"逻辑
  在此驱动上是空转，无碍配方）。
- **2070/610.47 复判**：CONTROL GET mask=0x3F 也命中（populated=0x3F，
  之前"2070 无 CONTROL"= mask 0x1cfff 含未充填位假象）；SET=-137
  INVALID_USER_PRIVILEGE（权限位拒绝非面缺失，提权后是否放行=开放题，
  涉及"此车道是否 50 系独占"的定性）。
- **CLI 收官（nvoc core+cli 同轮落地，第二十一轮合并定名）**：core 三操作
  QueryNvapiThermalInputs/SetNvapiThermalInput/ResetNvapiThermalInputs
  （hi::Gpu 三 pass-through 补齐）；medium 配方迁移（0x3F prime +
  applied/value patch + mask=触达位图，删 0x2F/0x65/0x80 旧经验字节）；
  **GET 面 = get-thermal-channels 单命令**（Info/Status 与 CONTROL 同族且
  角色标签本来就冗余拉 channels 快照——合并后 inputs 作为附加段，一次
  调用全览，CONTROL 拒绝时显 N/A 行）；写面两命令
  **set-thermal-channel-override**（--channel 0..5 默认 1、celsius 拒 0
  （Q8 歧义 applied+0）、范围 -40..150、set 后 retain 复读）/
  **reset-thermal-channel-override**（默认清 0..5、--channel 单清）。
  nvoc-cli 82 测试全绿，clippy/fmt 净，2070 冒烟通过。

## 16. Thermal limits 面板 RE 收口（2026-11-15 第二十二轮续，probe d263d11/a9d1c00）

### 16.1 混淆表解码：无新接口

- 审计器 sub_1400120F0 的探测对（blob VA, obf key）经 decode_override_ids.py
  的 splitmix VM（seed=key|0x891F610E00000000，opcodes 0xC1/0x02/0x43/0x84/0x4F，
  18B cap）直解：
  - **ThermalPolicyInfo = 0x2F69F8E5**（=nvoc PRIVATE GetInfo）
  - **ThermalPolicyControl = 0xC4554575**（=TARGET GET-prime）
- mVolt Thermal limits 面板 = nvoc 已封装的 ClientThermalPolicies 族，无新 ID。
  解码对：PolicyInfo(0x1403A5D4C,0xE12DA85F)、PolicyControl(0x1403A5D5E,0x4B9F8F69)。

### 16.2 5060 Ti 三缓冲全查 + T12 写实验

- documented INFO(0x0D258BB5) **0/350 全零**；documented STATUS(0xE9C425A1)
  count=0（全 Include Off；2070=1 → count=已装填策略数）；PRIVATE INFO 全扫
  布局 50 系同构（231 步进：slot1@dw461 [4,0,105,105,105,1]、slot4@dw1154
  [4,2,105,105,105,1]）但区间全退化 [cur,cur,cur]。
- T10 尺寸扫描：TARGET 族只收 984B（v2/v3 同尺寸），更大全拒；扫描必须单
  bit 掩码。T12 RMW 写（GET-prime→patch Q8@dword(15·slot+7)→SET 0xE097144F）：
  hotspot 106/110/111、GPU 105、Memory 106、**hotspot 104（范围内）**、
  sudo 复跑——**全部 SET=-1**。
- **对照实验闭合**：2070 slot2（真区间 [65,81,88]）非提权=**-137
  （INVALID_USER_PRIVILEGE）**、UAC 提权写 80C=**SET=0 retain=YES 精确读回
  +还原干净**。错误码分型：-137=权限门（可达）、-1=驱动拒写。

### 16.3 判决

- **5060 Ti 策略槽 0/1/4 = 驱动只读**：退化区间 [cur,cur,cur] 与 SET=-1
  （含提权）一致——min==max 即只读标记。
- **mVolt Thermal limits 面板在 50 系桌面 = 软域叠加**：Allowed
  （GPU -40..105 / Hotspot -40..110 / Memory -40..105）客户端 fiction
  （三缓冲均无源，档位=整数常量）；Include（无独立总闸，header=三旗 OR，
  profile 键 thermal/hotspot/memory_limit_enabled）落 STATUS count 的写
  同样未生效。
- **50 系桌面可写热面仍只有 sim override 车道**（T6d，ThermChannel
  CONTROL）；热策略写在驱动策略层锁定。可选收尾：mVolt Apply 后
  nvidia-smi GPU Target Temperature 应不变（面板装饰性用户验证）。
