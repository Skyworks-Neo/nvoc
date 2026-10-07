# 4060 Laptop + 610 驱动:内核态功率破解测试流程(任务书)

目标:在 RTX 4060 Laptop / 610.xx 驱动上,经 kmd 通道(pmxdrv 物理映射)定位
并改写 RM 电源策略的**执行层上限**,使功率墙超越合法滑条窗口(出厂 100 W)。
本机 P100/582.41 作对照机。

**版本红线(用户裁定)**:nvpwrctl 审计的 RM 对象偏移
(PowerRoot base@+0x3D14 / amount@+0x3D18 / UPPER@+0x3D24、Board selector2/3、
生成器 F7=min(C+A,U))是 **616.92 专属**,跨驱动版本不成立。610 与 582 的
每一步布局都必须以 **idalib 对本机构建逆向**为准;活体差分扫描只作为定位
手段,不作为布局证据。本文流程在这一点上是版本自洽的:任何"布局假设"
必须先被 610 的静态逆向或差分实验证实,才能进入写臂。

## 0. 已知结论(读我,避免重复踩坑)

1. **回显层 ≠ 执行层**(xOCD 审计 §18.8.2,P100 负载证伪):
   `set-power-command`(0x17695269 租约)只改 GET/NVML/nvidia-smi 的 Cap
   读数;负载钳位仍走滑条窗口。想抬执行墙只有两条路:**内核写 RM 策略
   UPPER**(本任务书)或 vBIOS 功率表。
2. **执行窗内的写**已经封装好:`nvoc-cli set-pwr-cur-limit <target> <value>`
   (0x8B3E7343/0xAFFC2279,窗钳)。测试中用它做执行层读回锚和对照臂。
3. 4060L 已知跨代锚点:板功率 policyId 0 默认 = **100000 mW**(=卡 TGP);
   `get-pwr-cur-info` / `get-power-command` / NVML 三面读数一致。
4. 每个写臂铁律:**identity → 扰动 → 读回 → 恢复**,恢复失败响亮报错;
   任何失配/异常(EC 狂转、黑屏、温度跳变)立即恢复 + 中止。
5. **(2026-10-06)静态轨已闭合,活体定位未闭合**:610.74 的 RM 命令
   (0x2080A61A/0x2080E61B)、PowerRoot 全字段漂移(-0x30)、函数族 RVA、
   UPpper 写入点枚举均已定案 → 见
   `kmd-power-policy-layout-r610-74.md`(capstone+idalib 双路互证)。
   差分判读不得硬性假设 base==UPPER(610 base 可能为 -1 哨兵)。
6. **(2026-10-06)BSOD 事故与安全 envelope**:BFS 25 万页池簇走查触发
   KMODE_EXCEPTION_NOT_HANDLED(无 minidump)。此后单次走查页数预算 ≤2000
   (NVOC_POWER_WALK_PAGES 显式放大须记录);策略对象不在锚页 2MB 簇、
   锚页 1-2 跳、.data 一跳、枢纽 FIFO 1500 页(全为阴性,详见布局档案 §6)。
7. **(2026-10-06)传输层代际**:PMXDRV(老服务/PAIPTAC 重建款)被驱动
   安全策略拦;**PMXDRV_NEW(Intel2019,B1A8EE…)可用**——本机实测多轮。
   kmd 通道一律连 PMXDRV_NEW。SET 须提权(非提权 -137,与既有台账一致)。

## 1. L0 环境准备

```powershell
# 管理员 PowerShell
nvoc-cli get-info                 # 记录 NVAPI interface version / 驱动串(应为 610.xx)
nvidia-smi -q -d POWER            # 基线: Power Limit / Default / Min / Max(记全)
nvidia-smi -q -d TEMPERATURE      # 基线温度
# HVCI 状态(须关;本机台式已确认关,笔记本须自查):
Get-CimInstance -Namespace root\Microsoft\Windows\DeviceGuard `
  -ClassName Win32_DeviceGuard | Select SecurityServicesRunning   # 期待 0 或无 2
# 卸载/退出 OEM 功率工具(Legion/OMEN/G-Sync 等),它们会抢写功率策略
```

驱动镜像取证(后续 idalib 用,**不要**直接用仓库里的 610_nvlddmkm.sys——
版本必须与本机一致):

```powershell
Get-ChildItem C:\Windows\System32\DriverStore\FileRepository\nv*.inf_* -Recurse -Filter nvlddmkm.sys |
  Select FullName, Length, LastWriteTime
(Get-FileHash <选中的 nvlddmkm.sys> -Algorithm SHA256).Hash    # 记录
nvoc-cli get-info   # 里的 NVAPI interface version 与镜像配对
```

## 2. L1 传输层验收(只读)

```powershell
sc create PMXDRV type= kernel start= demand binPath= <pmxdrv.sys 绝对路径>
sc start PMXDRV
cargo test -p nvoc-core --test kmd_pmxdrv_probe_live -- --ignored --nocapture
```

预期:冒烟 map/read/unmap 全通;走查 255/255 低内存页可读、low-stub 唯一根、
活体 nvlddmkm 头与磁盘指纹三项全匹配。**任何一步失败即止**(输出含诊断)。
构建注意:4060L 实测 PAIPTAC 重建款被安全策略阻止装载,Intel 2019 款正常——
笔记本一律用 `reverse/xocd/pmxdrv.sys`(哈希钉死)。
测完 `sc stop PMXDRV; sc delete PMXDRV`(或保留服务供后续步骤,结束再清)。

## 3. L2 RM 策略对象定位(只读,核心难点)

双轨:静态逆向给布局假设,活体差分给实证,两者对上才算定位。

### 3.1 静态轨(idalib,对本机 610 nvlddmkm)

检索次序(从确定到猜测):

1. **从已知入口追**:`NvAPI_GPU_ClientTgpWattSetStatus`(0xAFFC2279)的内核
   对应物是功率策略 SET 的 RM 命令面——从它的 handler 深追到策略对象字段
   写入点,标定 amount/UPPER 槽位偏移(616.92 上这条路产出 base/amount/UPPER
   三元组;610 应同构,偏移大概率漂移)。
2. **同源函数特征匹配**:nvpwrctl 记录的生成器语义 `F7=min(C+A,U)` 与
   mutator 写 UPPER 的函数(616.92: 0x4E4610/0x4E4680/0x8D6960)——在 610 里
   按指令模式(min 选择 + 邻域常量)搜同源函数,再从其 xref 标定偏移。
3. 产出:**610 布局档案**(对象构造点、base/amount/UPPER/selector 槽位偏移、
   RvaTable),存 `docs/reverse-engineering/nvapi/`(新知识,提交)。

### 3.2 活体差分轨(kmd 走查,版本容忍)

原理:执行层 cap 改变时,RM 策略对象里的 amount 字段随之改变——**写值差分
指纹**能物理定位对象页,与驱动版本无关。

```text
a. 基线 dump:nvoc-cli set-pwr-cur-limit tgp 90   (窗内非默认值,90 W)
   kmd 走查 dump nvlddmkm .data(磁盘 PE 头给 RVA → read_virtual)+ 收集
   .data 里的内核指针,逐指针映射目标页,全量快照(记录:页 VA/帧、内容哈希)
b. 扰动:nvoc-cli set-pwr-cur-limit tgp 95         (差 5 W,只有 amount 变)
c. 重扫同一指针集,diff:出现 90000→95000 (mW) 变化的页 = 策略对象页候选
d. 结构确认:候选页里找与 amount 同页/邻页的 [窗 max](=UPPER 假设槽)与
   [默认 100000](=base 假设槽);三元组同页 = 高置信
e. 写回原值:nvoc-cli set-pwr-cur-limit tgp <默认>
```

注意:cap 值可能有多份拷贝(遥测镜像),用"同页含 max+default 三元组"消歧;
`set-pwr-cur-limit` 会窗钳到 [min,max],选扰动值必须在窗内。

### 3.3 Root 形状签名(NvpwrControl 源码补充,2026-10-06)

差分扫描的消歧器(来自 NvpwrControl driver.c 源码,xOCD 审计
nvpwrcontrol-blackwell-tuner-audit.md §2.8):候选策略对象处应呈字段序列

```text
+0x3D10 init=1(u8) | elig(u8) | amountActive(u8) | pad
+0x3D14 cTGP(u32) | 0x3D18 amount(u32) | 0x3D1C policy_key(u32,<0x40)
+0x3D20 LOWER(u32) | 0x3D24 UPPER(u32)
```

出厂 4060L 期待 cTGP=amount=UPPER=100000、LOWER=min;差分(90→95W)时
amount 变、邻位不动。注意:以上偏移是 **616.92 布局**,610 上作为"五元组
相邻形状"的搜索模式使用(找到等价五元组再定 610 的实际偏移),不得照抄数值。

### 3.4 定位交付

对象内核 VA、物理帧、base/amount/UPPER/selector 槽位偏移表(3.1 与 3.2
互相印证后定稿)。**没有这页档案,禁止进入 L3。**

### 3.5 L3 写协议纪律(NvpwrControl 两阶段范本)

内核写保持 RM 状态连贯的完整范本(audit 文档 §2.8 ②):Phase A 檐下整备
(MAX/UPPER 钉回出厂 → 经**原生 setter** 改 cTGP/amount/elig 触发生成器重跑
→ F7/CURRENT 验证连贯)→ Phase B 抬顶(MAX+UPPER)→ 收敛验证;回滚镜像逆序。
610 适配清单(全部需 610 idalib):§2.4 的 6 个 RVA+签名、GPU 表链、
Major+0x25B0→Root、Board+0x2D0 setter。粗验证起步可只直写 UPPER+amount
观察生成器是否回写,连贯写再补原生调用。

## 4. L3 执行层写探针(写,高危)

前置:L2 档案 + 全部读回锚在线。

```text
1. 读 UPPER 原值(经 kmd 映射读该槽),存档
2. UPPER += 10000 (抬 10 W,保守量级;不要一步到 2 倍)
3. nvoc-cli set-pwr-cur-limit tgp 110   (>出厂窗 max 100 W,若窗钳拒绝=窗没变,
   恰好证明 UPPER 与窗的联动关系,记录)
4. 负载实测:stressor/furmark 2 分钟,nvidia-smi --query-gpu=power.draw --format=csv -l 1
   采样。判据:功率平台值顶到 ~110 W = 执行层抬顶成功;
   仍钳 100 W = UPPER 在 610 上不是执行墙 → 回 L2 重推布局
5. 温度/风扇全程监控;异常即恢复中止
   field note(nvpwr_another__Release mVolt+ 作者实测):若解锁后负载吃不满
   新墙,**抬 MSVDD 最低电压**(mVolt+ 实测有效;NVVDD min 无帧数收益)——
   显存侧 Vmin 是低负载吃满功率的常见卡点,对应我们的 set-domain-voltage 面
6. 恢复:UPPER 写回原值 → set-pwr-cur-limit 回默认 → 重启复验持久性
   (预期:UPPER 写易失,重启回落;若持久,记录为重要发现并更新文档)
```

**终态同步(用户假设,写臂成功后执行)**:`set-power-command` 租约写与内核
UPPER 写是互补的一对——UPPER 管执行墙,租约管回显面。L3 抬顶成功后,把
租约写到与 UPPER 一致的值(`nvoc-cli set-power-command <新墙 W> --force`),
使 GET/NVML/nvidia-smi 的 Cap 与真实执行墙一致,否则遥测面会显示旧值造成
误判;恢复时反向同步。即完整破解 = 回显(租约) + 执行(UPPER) 双写。

## 5. 对照臂 L3e(回显层,已封装,低风险)

```powershell
nvoc-cli set-power-command 115 --force   # >窗 max,租约写需 --force 过包络
# 确认: get-power-command / NVML / nvidia-smi Cap 读 115 W;
# 负载仍钳(P100 已实证;4060L 复验记录即可)
nvoc-cli set-power-command 100            # 恢复
```

## 6. 记录与交付

- 全程日志/JSON 归档 `reverse/kmd-power-4060l/`(untracked);
- **610 布局档案提交 docs**(与 582 对照后的 diff 表 = 跨版本规律第一手数据);
- 失败与否定结果同样记录(负面知识防重蹈);
- 更新本任务书的"已知结论"节(如 UPPER 易失性、窗联动行为)。

## 8. 产品化落地(2026-10-06,全链成功后)

**`nvoc-cli set-power-command <W> --kmd --pmxdrvpath <pmxdrv.sys> --force`** =
一条命令完成全周期(漏洞驱动只在内存里停留一个命令周期):

```text
服务注册(PMXDRV_KMD,残留自清;设备已在位则复用,崩溃残留认领)
→ 布局自动探测(layout_probe,对本机在役镜像,fail-closed;跨代零硬编码)
→ 走查 → D 状态感知定位(活体墙 tgp-control-current 精确匹配候选;D2=55W 类
  非 D1 场景下 UPPER≠滑条顶,靠活体墙消歧,歧义即拒)
→ UPPER 单 u32 写(身份门+读回,不符即时回滚)
→ 原生 tgp 写(仅控制值 < 目标时;防窗钳压低已有控制)
→ 租约写最后(回显面同步;checked 包络,越包络降级直写)
→ 复验 → 服务注销(驱动拒卸载时诚实上报:标记删除+重启消失)
```

安全门:--force 必需;绝对上限 500 W(无旗标可过);UPPER 易失=每开机一跑。
System32\drivers 回退:内核取不到原始卷镜像时自动复制重试一次。
实现:`core/src/kmd/power.rs`(core 写路径受控开放,章程已同步)。

## 9. Board 窗臂(2026-10-07,桌面形态;双臂自动化)

桌面(无 board 配置对象)PowerRoot 永不武装(init=0/key=0/UPPER=0,2070
TU104/610.47 活体 trace + idalib 定案:构造回调唯一写点 sub_1404ED4E0 在
board 选择器解析失败时静默 return)→ root UPPER 桌面零观察者,身份门全拒
改为**自动回退 board 臂**(`core/src/kmd/power.rs` + `board.rs`):

```text
活体窗三元组(tgp_watt_range 0x67F31384 + status 0x8B3E7343,GET 全安全)
→ Board 控制表扫查:root 对象 8 页 + 内核指针一跳 + 候选页池邻域 ±64 页
  (总预算 ≤1288 页),同页 {current,default,max} 值签名(跨度 ≤0x20;
  值相撞要求互异 dword,全等三元组要求三处)
→ 逐候选探测(≤8):写窗 max(读回)→ percent 到达验证(多假设
  100%/按窗顶/按 default)→ 不到达即回滚+恢复 current。
  实测教训两连(2070):① 首轮写中的是 echo/lease cell(0xFE 标记行),
  三元组同样成立 —— 镜像与活体行静态不可分;② 二轮差分(NVML -pl 200)
  实证控制行 control 槽(+0x600)跟写走而 range GET 不动 —— range GET
  读静态 policy info 行,"GET 跟随" oracle 失灵,percent 读回 ≥ 目标是
  唯一可靠判据(每轮自愈零残留)
→ percent 写 current(0xAD95F5ED 安全线;Turing 毒 SET 0xAFFC2279 绝不触碰)
→ 租约写 → 复验
```

**0 跟随的差分判读**(探测全回滚后):`nvidia-smi -pl <窗内非默认值>`(如
200,避开 105/175/219 三个已知常量)扰动 current → 重跑 locate-trace →
哪一行的 current 跟动哪行才是活体行;若扫查域内仍无,活体行在指针两跳
之外,需扩 sweep 或从 percent-GET 表反查。

与 root 臂的互补关系(跨代规律第一批实测):
- 4060L(移动/Ada):board 配置在 → root 武装 → 窗=UPPER;watt SET 可用,
  percent/NVML 不可用 → 破解 = UPPER 写(§8 已收官);
- 2070/3060(桌面/pre-Ampere):board 配置缺席 → root 永不武装 → 窗在
  Board;percent/NVML 可用而 watt SET 毒 → 破解 = 抬 Board 窗 + 安全写补
  current。

实验顺序(两台目标机:桌面 2070/TU104 610.47、3060 无 shunt mod):

```powershell
# 管理员;先只读诊断(身份门逐格 + board 链 dump + 三元组扫查 + 候选邻域)
sc create PMXDRV type= kernel start= demand binPath= <pmxdrv.sys>; sc start PMXDRV
cargo test -p nvoc-core --test kmd_locate_trace_live -- --ignored --nocapture
# 唯一候选 ✓ 后再开写臂(自动选臂,输出带 "arm": "board"):
nvoc-cli set-power-command <目标 W> --kmd --pmxdrvpath <pmxdrv.sys> --force
# 负载实测:nvidia-smi --query-gpu=power.draw --format=csv -l 1
# 恢复:重启(窗 max 预期易失);或回写原值 + set-public-tgp-percent 100
sc stop PMXDRV; sc delete PMXDRV
```

判读要点:3060 若 percent 写后 current 读回 < 目标(换算/取整),手动
`set-public-tgp-percent` 补;窗跟随验证失败 = 候选是遥测镜像,按 trace
输出人工判读后重跑。

## 7. 对照机:P100/582.41

同流程跑本机;静态轨对 `reverse/610_nvlddmkm.sys` 换成本机 582.41 镜像重推
偏移。两版布局 diff 单列一节——这是"RM 电源策略布局跨版本漂移规律"的
第一批实测数据,直接决定后续版本适配成本。
