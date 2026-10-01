# P100 私有 VfTable offset 黑盒实验协议（操作者：用户）

配套：`p100-58241-privatevftable-audit.md`（静态结论）、`nvapi-rs/tests/p100_vfp_control_raw_dump.rs`（E0 仪器）。
目标：实证裁决 ①0..79 mode-0 VALUE 的单位（µV 2x / µV 无 2x / kHz）与方向矛盾（08-30"同频降压" vs 09-30"电压上升"）；②80..159 频率语义交叉复核；③mode-1 点索引位移模型的电压效应。

## 0. 前置条件（缺一不做）

- 卡已恢复：`nvidia-smi` 正常枚举，`get-private-vftable` 能读、全 offset 列为 0（楔死已清）。
  恢复阶梯：设备管理器禁用/启用 → 无效则重启（RM 运行时内存，重启即复位，无持久化风险）。
- 温度 < 75°C（`nvidia-smi --query-gpu=temperature.gpu --format=csv`）。
- 提权终端（写入需要管理员；读不需要）。**单命令一提权批**——绝不把 set+measure+reset 串在一个脚本/一个提权批里（reset 与被测状态同死无回滚）。
- ⚠️ **注册表检查**：确认从未设置 `RmVFPointCheckIgnore`（582.41 内核 VF 点校验旁路开关，审计报告 §5）。设过必须删除。

## 1. 禁止事项（明文）

1. **相邻点 raw 差分 ≥ 25000（= 一个 stock 阶梯步）即制造非单增 VF 表 → GPU 软挂死**（电压无明显变化、禁用/启用恢复；驱动对 raw 路径不做负向规范化）。推论：0..79 段负值写入必须保证与所有已写相邻点的差分在合法带内——**逐点补写相邻点极易无意制造差分**（#29 补写 #30 教训：等值 −25000/−25000 活，−25000/−50000 死）；深平台的边界边差分同样致命（原事故 16 点 × −200000 两端边界 = 16 阶梯步差分）。
2. 任何段大幅正写起步——一律从单点微值开始（|raw| ≤ 100000）。
3. -1 出现立即停手（通道已楔死，继续写只污染状态）。
4. 不碰 `RmVFPointCheckIgnore`。
5. 单点写目标避开 #0-4（405 MHz floor 平台，无分辨力）与 plateau 段 #36-79；用曲线膝盖区 #20-35。

## 2. 仪器命令（全程只读，任意多次）

```powershell
# E0 raw 仪器（写前/写后各跑一次；跑完把 JSON 改名留底）
cargo test -p nvapi --test p100_vfp_control_raw_dump -- --nocapture --ignored
mv ..\reverse\p100-58241-vfp-control-raw.json ..\reverse\raw-E1-before.json   # 示例

# 真实电压（逐轨 µV）+ get-status 核心电压
nvoc-cli get-volt-rail-info
nvoc-cli get-status

# 解码视图（读回 offset 列已修为有符号：-20.0 = raw -40000）
nvoc-cli get-private-vftable --bank 0

# 实测时钟（idle floor 对 MEM 段无分辨力，需锁频或负载下读）
nvidia-smi --query-gpu=clocks.gr,clocks.mem,temperature.gpu --format=csv
```

判读核心 = **写前后 raw JSON 逐字节 diff**：哪个 bank/index 的哪些 dword 动了，就是 VALUE 的落点与量纲铁证（与解码视图的 RM 外推镜像无关）。

## 3. 实验矩阵

### E-P · 双平面配对写（真实频率轴平移——主战线）
bank0 = 80 点 × 双平面（A=0..79 电压侧，B=80..159 频率侧，index+80 对应）。**等值配对写 = 相干点重定义 = 恒压真实超频**（已实证：25-35 + 105-115 各 typed 100000 → 750 mV 锁下 1215→1328 稳定）。
- 次序铁律：先 A 后 B；两平面**绝对同值**（B > A + 1 bin = raw 25000 差分 → 暴死，且配对违例楔死深于单段——禁用/启用可能无效，需重启，预留心理预期）。
- 配方（每条单独提权批，写前存档）：
  ```powershell
  sudo nvoc-cli set-private-vftable-range-offset 0 25 35 <V>
  # 观测（ clocks + 双轨电压 + get-private-vftable ）
  sudo nvoc-cli set-private-vftable-range-offset 0 105 115 <V>
  # 观测：服务频率是否 > 1328 = 天花板判决
  ```
- 升级序列：100000（已验证 +100 MHz）→ 125000 → 150000，每次两侧同值同增；服务频率突破 1328 = 08-30 天花板问题攻破；若钳 1328，试段宽扩展（20-40/100-120）或平台段。
- 恢复：两侧各写 0 归零（先 B 后 A），-1 立即停，失败则重启。

每个实验 = 基线(raw+volt-rail+clocks) → 单条写命令(单独提权批) → 观测(raw+volt-rail+clocks+get-private-vftable) → 恢复(写 0) → 复核(raw diff 归零)。

### E1 · GPC 单点微正（核心裁决）
```powershell
sudo nvoc-cli set-private-vftable-point-offset 0 25 50000
```
raw 落表 = +100000（CLI 已做 Pascal ×2）。**观测时 rail0、rail1、get-status 的 VFP voltage 三方分别记录**（已知分层：VoltRails=物理真值、VFP voltage=表派生）。两轨预期同步移动（E0 实测 idle 双轨仅差 6.25mV）。判读表：

| 观测 | GPC VALUE = µV(2x) | µV(无2x) | kHz(2x) |
|---|---|---|---|
| rail0/rail1 µV 变化 | **各 +50 mV** | 各 +100 mV | 不变 |
| 读回 effect 列 | +50.0 | +100.0 | +50.0（镜像，不作数） |
| get-status VFP voltage | 不动（表派生） | 不动 | 不动 |
| 实测 GPC 时钟 | 不变 | 不变 | 曲线上移特征（同频降压） |

若 rail 电压**下降** → 08-30 判读正确（曲线频率上移），本轮"电压上升"观察需重审观测条件。

### E2 · GPC 单点微负（斜率帽行为 + 修后显示）
```powershell
sudo nvoc-cli set-private-vftable-point-offset 0 25 -20000
```
raw = -40000。期望：retained -40000（不钳）；读回 effect 列显示 **-20.0**（修复后符号正确）。电压按 E1 裁决的单位移动。**绝不扩大到 range 写。**

### E3 · MEM 单点微正
```powershell
sudo nvoc-cli set-private-vftable-point-offset 0 110 20000
```
raw = +40000。既有结论：MEM 因子 2000（233600↔+116.8MHz）→ 期望 mem 时钟 +20 MHz（负载/锁频下可测，idle floor 无分辨力）、电压不变、raw diff 命中 #110。

### E4 · MEM 单点微负
```powershell
sudo nvoc-cli set-private-vftable-point-offset 0 110 -20000
```
期望 mem -20 MHz、电压不变。MEM 段负单点是安全已知项（历史 A/B 走过）。

### E5 · GPC mode-1 对照（点索引位移 → ΔV=C）
```powershell
sudo nvoc-cli set-private-vftable-point-offset 0 25 --raw 100
```
mode-1 raw = VALUE 低 i16，g(def) 1:1。期望：电压按 C(#25 的 g(def) 斜率) 移动、读回 freq_default 镜像移动。恢复必须用 `--raw 0`。

### E6（可选，E1 结论之后）· GPC mode-0 上限探测
单点 #25 从 +100000 起步按 +50000 步进（每次一提权批、写前存档），找电压偏移的上限/clamp 点（mode-0 已知全段接受 0..330400 的频率解释下限，电压解释下的上限未知）。

## 4. 恢复阶梯（按序尝试）

⚠️ **双平面卡上 reset 已内建有序复位**（CLI/GUI/TUI 全部：plane B 80..159 先归零 → plane A 0..79 随后；按 index 顺序清零 = A[n] 归零时 B[n+80] 仍持正值 = B > A 违例 = 炸卡）。CLI 默认走配对写/有序复位，`--unsafe` 是单平面逃生口（自担时序责任）。

```powershell
# 1) 单点归零（每条单独提权批；双平面卡默认配对，两平面一起归零）
sudo nvoc-cli set-private-vftable-point-offset 0 25 0
# 2) 整段归零（双平面卡默认配对有序）
sudo nvoc-cli set-private-vftable-range-offset 0 20 35 0
# 3) 全表有序复位
sudo nvoc-cli reset-private-vftable-offset 0
# 4) 设备管理器禁用/启用（单段违例通常够；配对违例可能无效）
# 5) 重启（终局，必定干净；配对违例的 RM 错误风暴形态常需此级）
```

## 5. 判读回填

实验完成后把每步 diff 结论回填审计报告 §6（单位矩阵、方向矛盾裁决、2x 轴适用性），并决定 CLI 的 `Unit: kHz` 标签与 Pascal-2x 逻辑是否需要按段拆分（另起 commit，先征求确认）。
