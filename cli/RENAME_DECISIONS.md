# nvoc-cli 命令命名归一化决策记录

分支:`cli-arg-organized-more-reversing`
方向:**扁平 + 归一化**(对外保持扁平 60+47 命令,只整名字;内部维持手写 enum 不动)

## 归一化规则 R1–R7

- **R1 单位后缀**:仅在消歧义时保留(如 watt vs percent 区分两种功率面)。无歧义时去冗余后缀(`-mhz`/`-uv`/`-c`/`-mv`),因参数名(`OFFSET_MHZ`)+格式(`125MHz`)已含单位。
- **R2 get/set/reset 对称**:词干一致;reset 不带值可省单位后缀,但词干与 get/set 同名。
- **R3 同义词统一**:`app` vs `applications` 选一;`temp` vs `limit` 按语义选。
- **R4 单复数名副其实**:批量用复数,单个用单数。
- **R5 动词规范**:只读=`get`;有写副作用的探测=`scan` 不用 `probe`;纯谓词补名词。
- **R6 孤立 setter**:无 get/set 对的,要么补对、要么名上暗示。
- **R7 后端语义暴露**:同一 CLI 名跨后端做不同语义操作,需在名/help 暴露或拆分。

## 已确立术语体系

| 术语 | 含义 | 底层线 / nvapi ID |
|---|---|---|
| `public-vftable` | 公共 VF 表(VfPoints 线,曲线点频率 offset) | VfPoints `0x21537AD4`/`0x0733E009` 等 |
| `gpc-volt-lock` | GPC 域电压锁(PerfClientLimits 线) | `0x39442CFB`/`0xE440B867` |
| `pstate-global-freq-offset` | P-State 级全局频率偏移(Pstates20 线) | `0x0F4DAE6B`/`0x6FF81213` |
| `freq-range` | 读 min/max 频率范围(NVML) | NVML |
| `freq-lock` | 锁定频率(各类 lock 体系) | — |
| `via-mem-range` | 点明 NVAPI 路径的内存频率窗口机制 | — |
| `legacy-*` | Kepler 老驱动接口前缀 | SetClocks `0x6F151055` 等 |
| `application` | 全称(非 app 缩写) | NVML |
| `tgp` | 统一功率族(NVAPI ClientPowerPolicies / NVML TGP 墙) | `0xAD95F5ED`/`0x34206D86` |
| `public-tgp-percent` | NVAPI ClientPowerPolicies 百分比 | `0xAD95F5ED` |
| `offset` | 取代 `delta`(曲线点/P-State 频率偏移) | — |

## 已完成族决策

### vfp-curve 族(9→6,3 移除)

| 原名 | 决策 | 底层线 |
|---|---|---|
| `get-vfp` | → `get-public-vftable` | VfPoints `0x21537AD4`等 |
| `get-vfp-point-voltage-mv` | **移除**(历史遗留,无消费) | — |
| `probe-voltage-limits` | **移除**(功能被 get-volt-rails P0 电压上下限覆盖) | — |
| `check-voltage-frequency` | **移除**(中层接口历史外露) | — |
| `set-vfp-voltage-lock` | → `set-gpc-volt-lock` | PerfClientLimits `0x39442CFB` |
| `set-vfp-point-delta-mhz` | → `set-public-vftable-point-offset` | VfSetControl `0x0733E009` |
| `set-vfp-range-delta-mhz` | → `set-public-vftable-range-offset` | VfSetControl `0x0733E009` |
| `reset-vfp-deltas` | → `reset-public-vftable-offset` | SetPstates20+VfSetControl |
| `reset-vfp-lock` | → `reset-public-vftable-gpc-lock` | PerfClientLimits `0xE440B867`+`0x39442CFB` |

注:hi 层 delta÷2 的 GPU×2 语义坑(名带 mhz 会误导),故去 -mhz。

### clock-offsets 族(7→3,4 移除)

| 原名 | 决策 | 底层线 |
|---|---|---|
| `get-clock-offset-mhz` | → `get-pstate-global-freq-offset` | GetPstates20 `0x6FF81213` |
| `set-core-offset-mhz` | **移除**(被通用版替代) | — |
| `set-memory-offset-mhz` | **移除**(被通用版替代) | — |
| `set-clock-offset-mhz` | → `set-pstate-global-freq-offset` | SetPstates20 `0x0F4DAE6B` |
| `reset-core-offset-mhz` | **移除**(与 set 对称) | — |
| `reset-memory-offset-mhz` | **移除**(与 set 对称) | — |
| `reset-pstate-clock-offsets` | → `reset-pstate-global-freq-offset`(**+需支持 --domain 选择**) | GetPstates20+SetPstates20 |

注:`global` 对应 `public-vftable` 的 `point`(点级 vs 全局)。core/memory 快捷命令全砍,统一走 `--domain`。reset 改单数 + `--domain` 过滤(非批量清所有,需改 medium 层枚举逻辑加 domain 过滤)。

### pstate-appclock-legacy 族(6 命令)

| 原名 | 决策 | 底层线 |
|---|---|---|
| `get-pstates` | → `get-pstate-freq-range` | NVML min_max_clock_of_pstate |
| `set-pstate-lock` | → `set-pstate-lock-via-mem-range` | PerfClientLimitsSetStatus `0x39442CFB` |
| `set-applications-clocks-mhz` | → `set-legacy-application-freq-lock` | NVML set_applications_clocks |
| `get-supported-app-clocks` | → `get-supported-legacy-application-freq` | NVML supported_*_clocks |
| `reset-applications-clocks` | → `reset-legacy-application-freq-lock` | NVML reset_applications_clocks |
| `set-legacy-clocks-mhz` | → `set-legacy-freq`(**分 domain core/mem,Kepler 未测**) | SetClocks `0x6F151055` |

注:#6 改为单值 + `--domain`(与 set-pstate-global-freq-offset 的 --domain 一致),Kepler 老驱动接口尚未测试。

### power 族(4 命令,合并方案)

| 原名 | 决策 | 底层线 |
|---|---|---|
| `get-power-watt` + `get-tgp-watt-range` | **合并为 `get-tgp-watt`**(auto: NVAPI 优先读 ClientPowerPoliciesGetInfo `0x34206D86` 取 TGP 范围,NVML 兜底读 power_management_limit) | NVAPI+NVML |
| `set-power-watt` | → 合并入 `set-tgp-watt`(auto,NVAPI 也有写入原语 `0xAFFC2279`/`0xBFF09E59`,走 auto) | NVAPI+NVML |
| `set-power-percent` | → `set-public-tgp-percent`(仅 NVAPI) | ClientPowerPoliciesSetStatus `0xAD95F5ED` |
| `reset-power-percent` | → `reset-public-tgp-percent`(仅 NVAPI) | ClientPowerPoliciesGetInfo+SetStatus |

注:NVAPI ClientPowerPolicies 有瓦特写入原语(实测 `set-tgp-watt 60` via nvapi 成功),非只有百分比。

### thermal 族(12 命令,恢复后范围)

| 原名 | 决策 | 底层线 |
|---|---|---|
| `get-temp-thresholds` | → `get-temp-limit`(优先 NVAPI,双端;原 get-temperature-thresholds 已不存在,被此合并) | NVAPI `0xC4554575`/`0xE097144F` + NVML |
| `get-temperature-thresholds` | **已不存在**(被 get-temp-thresholds 取代合并) | — |
| `get-thermal-settings` | → `get-legacy-temp-sensor` | NVAPI GetThermalSettings `0xE3640A56` |
| `set-thermal-limit-c` | → `set-temp-limit`(去 -c) | NVAPI ThermalPoliciesSetStatus `0x34C0B13D` / NVML |
| `reset-thermal-limit-c` | → `reset-temp-limit`(跟随 set 词干) | NVAPI `0x0D258BB5`+`0x34C0B13D` |
| `set-acoustic-temp-c` | **合并到 `set-temp-limit` 的 NVML 分支**,**加 `--domain` 选择**(AcousticCurr/GpuMax 等 enum 作为 domain) | NVML set_temperature_threshold |
| `get-tdp-temp-limits` | **拆分为 `get-public-power-limit` + `get-public-temp-limit`** | NVAPI `0x34206D86`+`0x0D258BB5` |
| `get-throttle-reasons` | 保留 | NVML |
| `get-thermal-sim` | 保留 | NVAPI GetThermalSimulationMode |
| `set-thermal-sim` | 保留 | NVAPI SetThermalSimulationMode |
| `disable-thermal-sim` | 保留 | NVAPI DisableThermalSimulation |
| `get-rated-tdp` | 保留 | NVAPI `0xED2BEA09`等 |

注:`temp-limit` 取代 `thermal-limit`。`public-power-limit`/`public-temp-limit` 拆分自 `tdp-temp-limits`,用 `public-` 前缀对齐 `public-tgp-percent`/`public-vftable` 体系。

### fan 族(9 命令,合并后收敛)

| 原名 | 决策 | 底层线 |
|---|---|---|
| `get-fan-info` | 保留 | NVML+NVAPI FanCoolerGetInfo |
| `set-fan-percent` + `set-fan-rpm` | **合并为 `set-fan-speed`**(`--percent`/`--rpm` 模式分派):NVAPI 分支默认 `--percent` 走原 set-fan-percent(ClientFanCoolersSetControl `0xA58971A5`),`--rpm` 走原 set-fan-rpm;NVML 分支默认 `--percent` 接入 NVML set_fan_speed | NVAPI `0xA58971A5`/NVML |
| `reset-fan` + `reset-fan-rpm` | **合并为 `reset-fan-speed`**(遵循一致的 NVAPI/NVML 分派逻辑):NVAPI 重置 cooler levels, NVML set_default_fan_speed | NVAPI `0x8F6ED0FB`/NVML |
| `get-fan-curve` | 保留 | NVAPI ClientFanPolicies `0x200DC` |
| `set-fan-curve` | 保留 | NVAPI ClientFanPolicies `0x200DC` |
| `reset-fan-curve` | 保留 | NVAPI ClientFanPolicies `0x200DC` |
| `set-fan-stop` | 保留 | NVAPI |

注:`set-fan-speed` 用 `--percent`/`--rpm` 模式统一原 percent/rpm 两命令;`reset-fan-speed` 同理分派。fan-curve 独立保留(曲线 vs 单点不同概念)。

### voltage 族(8→6,2 合并)

| 原名 | 决策 | 底层线 |
|---|---|---|
| `get-pstate-base-voltage-uv` | → `get-legacy-gpc-rail-overvolt-limit` | GetPstates20 `0x6FF81213` |
| `set-pstate-base-voltage-uv` | → `set-legacy-gpc-rail-overvolt-limit` | SetPstates20 `0x0F4DAE6B` |
| `reset-pstate-base-voltages` | → `reset-legacy-gpc-rail-overvolt-limit` | SetPstates20 `0x0F4DAE6B` |
| `get-voltage-boost-percent` | → `get-public-gpc-rail-volt-boost` | ClientVoltRailsGetControl `0x9DF23CA1` |
| `set-voltage-boost-percent` | → `set-public-gpc-rail-volt-boost` | ClientVoltRailsSetControl `0xB9306D9B` |
| `reset-voltage-boost-percent` | → `reset-public-gpc-rail-volt-boost` | ClientVoltRailsSetControl `0xB9306D9B`(=0) |
| `get-legacy-overvolt-ranges` + `get-legacy-p0-core-max-voltage-delta` | **合并为 `get-legacy-gpc-rail-volt-range`**(`--pstate` 指定 P0,原 #8 是 #7 的 P0 max 子集) | GetPstates20 `0x6FF81213` |

注:`gpc-rail` 统一 voltage 族术语。`legacy-gpc-rail-overvolt-limit`(Pstates20 线,P-State 基础电压 delta)、`public-gpc-rail-volt-boost`(ClientVoltRails 线,电压提升百分比)、`legacy-gpc-rail-volt-range`(过压范围,P0 用 --pstate)。

### locked-clocks 族(2 命令)

| 原名 | 决策 | 底层线 |
|---|---|---|
| `set-locked-clocks-mhz` | → `set-freq-lock` | NVAPI PerfClientLimitsSetStatus `0x39442CFB` / NVML set_gpu/mem_locked_clocks |
| `reset-locked-clocks` | → `reset-freq-lock` | NVAPI `0x39442CFB`(None) / NVML reset_gpu/mem_locked_clocks |

注:`freq-lock` 词干与 `gpc-volt-lock` 对齐(同 PerfClientLimits 线的 lock)。NVAPI=VFP 频率锁(upper+lower),NVML=硬时钟 floor/ceiling,语义不一但统一入口。

### autoboost-apirestriction 族(5→4)

| 原名 | 决策 | 底层线 |
|---|---|---|
| `get-auto-boost` | → `get-autoboost-status` | NVML auto_boosted_clocks_enabled |
| `set-auto-boost` | → `set-autoboost-status` | NVML set_auto_boosted_clocks |
| `set-auto-boost-default` | → `reset-autoboost-status`(reset 对应 set default) | NVML set_auto_boosted_clocks_default |
| `get-api-restriction` | → `get-autoboost-support` | NVML is_api_restricted |
| `set-api-restriction` | → `set-autoboost-support` | NVML set_api_restricted |

注:合并为 autoboost-status 三件套(get/set/reset)+ autoboost-support 二件套(get/set)。词汇统一,reset 对应 set default。原 api-restriction 概念重定义为 autoboost-support。

### edid 族(3 命令)

| 原名 | 决策 | 底层线 |
|---|---|---|
| `get-edid` | 保留 | NVAPI GetEDID `0x37D32E69` |
| `set-edid` | 保留 | NVAPI SetEDID `0xE83D6456` |
| `clear-edid` | 保留 | NVAPI SetEDID `0xE83D6456`(空 EDID) |

## 待处理:恢复的 thermal/power 成果族

以下恢复命令需逐一审查命名是否与新术语体系(public-/gpc-/legacy-/freq-lock/temp-limit/autoboost- 等)一致:

- **volt-rails**:`get-volt-rails`/`set-volt-rail-offset`/`set-volt-rail-target`(volt-rails 线,命名清晰,待审是否改 gpc-rail 对齐)
- **dnotifier**:`get/set-dnotifier`(D-Notifier 功率状态通知)
- **power-mizer**:`get/set-power-mizer`(PowerMizer 策略)
- **power-mode**:`get/set-power-mode`(PowerModes 三件套)
- **rated-tdp**:`get-rated-tdp`(Rated-TDP 控制三件套)
- **force-pstate**:`set/reset-force-pstate`(SetForcePstate)
- **perf-level**:`set-perf-level`(SetPerfLevel 0x75DD3E6A 免管理员 pstate 锁)
- **perf-freq-caps**:`set/reset-perf-freq-caps`(PerfLimits 0x32CA4983 族)
- **clk-domain**:`get-clk-domains`/`set-clk-domain-offset`/`get-clk-domain-freq`/`get-clk-vf-points`(ClockClient 域)
- **core-voltage-control**:`get/set-core-voltage-control`(CoreVoltageControl 0xA91F88EB)
- **overvolt**:`set-overvolt-uv`(legacy overvolt)
- **vfp-private**:`set-vfp-point/range-private`/`reset-vfp-private`(私有 V/F-POINTS)
- **pmgr-arbiter**:`get/set-pmgr-arbiter`(PMGR 电压仲裁)
- **oc-scanner**:`oem-oc-scanner`(NVIDIA OC Scanner 四件套)
- **restart-display-driver**:`restart-display-driver`
- **pstate-native**:`get/set/reset-pstate-native`(queryPStateInfo 0x7B30AE0D)

### 恢复子族决策

#### volt-rails 子族(#1-4)

| 原名 | 决策 | 底层线 |
|---|---|---|
| `get-volt-rails` | → `get-volt-rail-info` | VoltRails `0x465F9BCF`等 |
| `set-volt-rail-offset` + `set-volt-rail-target` | **合并为 `set-volt-rail-limit`**(`--offset`/`--target` 选项分派) | `0x87C55C8A` |
| `get/set-core-voltage-control` | **暂时不改**(功能不明确,待确认) | `0xA91F88EB`/`0xDC2BD4A6` |

#### overvolt(#5)

| 原名 | 决策 | 底层线 |
|---|---|---|
| `set-overvolt-uv` | 保留 | PSTATES20 V2 OV array |

#### pstate 锁类(#10/11/18)——忠实反映实际锁定优先级

| 原名 | 决策 | 底层线 |
|---|---|---|
| `get/set/reset-pstate-native` | → `get/set/reset-pstate-lock`(**需 admin,最高优先级**) | queryPStateInfo `0x7B30AE0D`等 |
| `set/reset-force-pstate` | → `set/reset-private-forced-pstate-lock-user` | SetForcePstate `0x025BFB10` |
| `set-perf-level` | → `set-private-permanent-pstate-lock-user` | SetPerfLevel `0x75DD3E6A`(免管理员,永久锁) |

注:三级 pstate 锁命名反映优先级 —— `pstate-lock`(admin 最高)/ `private-forced-pstate-lock-user`(强制锁)/ `private-permanent-pstate-lock-user`(永久锁)。只有 get/set/reset-pstate-native 需 admin。

#### clk-domain + vfp-private(#13/15)

| 原名 | 决策 | 底层线 |
|---|---|---|
| `set-clk-domain-offset` | → `set-private-freq-domain-global-offset` | ClockClient `0x20809019`族 |
| `set-vfp-point-private` | → `set-private-vftable-point-offset` | 私有 V/F-POINTS |
| `set-vfp-range-private` | → `set-private-vftable-range-offset` | 私有 V/F-POINTS |
| `reset-vfp-private` | → `reset-private-vftable-offset`(**+支持 `--domain` 选择如 gpc/xbar/host**) | 私有 V/F-POINTS |
| `get-clk-domains` | → `get-private-freq-domain-info` | ClockClient `0x20809019` |
| `get-clk-domain-freq` | → `get-private-freq-domain-status` | ClockClient MEASURE_FREQ |
| `get-clk-vf-points` | → `get-private-vftable`(**+添加 `--domain` 选择器如 --gpc xbar host**) | ClockClient VfPoints |

#### 保留的子族(#6-9,12,14,16,17)

| 命令 | 决策 |
|---|---|
| `get/set-pmgr-arbiter` | 保留 |
| `get-power-mizer` | 保留 |
| `get/set-power-mode` | 保留 |
| `get-rated-tdp` | 保留 |
| `set/reset-perf-freq-caps` | 保留 |
| `get/set-dnotifier` | 保留 |
| `oem-oc-scanner` | 保留 |
| `restart-display-driver` | 保留 |

## 待确认

(全部已确认,无待确认项)

## 命名审查完成状态

所有主族 + 恢复子族命名决策已落盘。决策文件 `cli/RENAME_DECISIONS.md` 是改名的唯一权威来源。

## 完整术语体系汇总

| 前缀/词干 | 含义 | 示例 |
|---|---|---|
| `public-` | 用户可调的公开表/策略 | public-vftable / public-tgp-percent / public-power-limit / public-temp-limit / public-gpc-rail-volt-boost |
| `private-` | 私有/底层控制面 | private-vftable / private-freq-domain-global-offset / private-forced-pstate-lock-user / private-permanent-pstate-lock-user |
| `gpc-` | GPC 域 | gpc-volt-lock / gpc-rail |
| `legacy-` | 老驱动/老接口 | legacy-temp-sensor / legacy-application-freq-lock / legacy-freq / legacy-gpc-rail-overvolt-limit / legacy-gpc-rail-volt-range |
| `freq-lock` / `freq-range` / `freq-offset` | 频率锁/范围/偏移 | set-freq-lock / get-pstate-freq-range / set-pstate-global-freq-offset |
| `temp-limit` | 温度限制(取代 thermal-limit) | set-temp-limit / reset-temp-limit |
| `autoboost-status` / `autoboost-support` | 自动加速状态/支持 | get-autoboost-status / get-autoboost-support |
| `pstate-lock` | admin 最高优先级 pstate 锁 | get/set/reset-pstate-lock |
| `tgp` | 统一功率族 | get-tgp-watt / set-tgp-watt / set-public-tgp-percent |
| `volt-rail-limit` | volt-rail 偏移/目标(合并) | set-volt-rail-limit(--offset/--target) |

注:`offset` 取代 `delta`。单位后缀(-mhz/-uv/-c/-mv/-percent)一律去除,除非消歧义(watt vs percent)。

## 术语规则补充

- 描述性命名风格:命令名可揭示底层机制(如 `set-pstate-lock-via-mem-range` 点明 NVAPI 路径机制)
- `public-` 前缀:用户可调的公开表/策略(public-vftable / public-tgp-percent / public-power-limit / public-temp-limit)
- `gpc-` 前缀:GPC 域(gpc-volt-lock)
- `temp-limit` 取代 `thermal-limit`(温度限制);`-c` 后缀一律去除
- `legacy-` 前缀:老驱动接口/老传感器(get-legacy-temp-sensor / set-legacy-application-freq-lock / set-legacy-freq)
- 恢复的命令已有清晰命名(tgp-watt/volt-rails/thermal-sim/fan-curve/dnotifier 等)多数保留,仅需检查与上述术语体系一致性
