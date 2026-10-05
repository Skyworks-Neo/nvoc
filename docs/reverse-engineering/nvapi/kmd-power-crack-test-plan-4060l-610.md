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

### 3.3 定位交付

对象内核 VA、物理帧、base/amount/UPPER/selector 槽位偏移表(3.1 与 3.2
互相印证后定稿)。**没有这页档案,禁止进入 L3。**

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

## 7. 对照机:P100/582.41

同流程跑本机;静态轨对 `reverse/610_nvlddmkm.sys` 换成本机 582.41 镜像重推
偏移。两版布局 diff 单列一节——这是"RM 电源策略布局跨版本漂移规律"的
第一批实测数据,直接决定后续版本适配成本。
