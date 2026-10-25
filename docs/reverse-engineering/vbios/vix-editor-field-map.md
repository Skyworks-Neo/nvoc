# vix-editor GUI 字段 ↔ nvoc 解码字段映射

对照物：`reverse/vix/nvidia-vbios-vix-editor-main` release 0.1.21 GUI
（`nvbios` GUI 窗口，Advanced XOC fields 开启），样本 ROM
`4090_48G_AD102_MAX_450W_1075mV_95023C0002.rom`（Ada，BIOS 95.02.3C.00.02）。
nvoc 侧入口：`nvoc-cli get-vbios --verbose`（human）与 `-O json` 的
`structural` 段。核对命令见文末；**全部映射已用该 ROM 逐值验证**。

## Power（3x 功率策略，`structural.tables_v3.power_policy.rows`）

行定位：NVAPI `GetPwrCurInfo` 的 policy Index = 表行号、Subtype = 行的
channel 字节（同构验证见 git 999b5df 提交说明）。角色名 = 通道字节模板
匹配（`PowerChannelEntry::role_name`，模板来自 vix `tables.py`
`ADA_POLICIES`/`SLOT_CHANNEL`/`INPUT_12V_CHANNEL`/`NVVDD_*_CHANNEL`）。

| vix GUI 字段 | vix key | nvoc 行（4090_48G 实测） | nvoc 字段 |
| --- | --- | --- | --- |
| Board power ceiling | `power_max_w` | `#2 total-board 150000..450000..450000 mW` | `rows[2].maximum_raw`（TGP 行，`tgp_index`） |
| Boot default board power | `power_default_w` | 同上 | `rows[2].default_raw` |
| Board power floor | `power_min_w` | 同上 | `rows[2].minimum_raw` |
| PCIe slot power ceiling / rated | `slot_power_max_w` / `slot_power_rated_w` | `#5 pcie-slot 10000..66000..66000 mW` | `rows[5]`（Ada row 5；kind4 ch4） |
| NVVDD output power threshold ceiling / rated (advanced) | `nvvdd_power_threshold_max/rated_w` | `#16 nvvdd-power 10000..600000..900000 mW` | `rows[16]`（class5 mW on NVVDD 输出通道 ch18） |
| （GUI 未显示）NVVDD output power | `nvvdd_power_*` | `#6 out/5 10000..450000..450000 mW` | `rows[6]`（class0 on NVVDD，= 板功率镜像） |
| （GUI 未显示）NVVDD output current 75A | — | `#8 out/5 1..57500..75000 mA` | `rows[8]` |
| （GUI 未显示）shunt multiplier | `shunt_multiplier` | `#1 conn/12 1..1001000..1001000` | 电流缩放相关行（GUI 顶部的 Shunt multiplier ×1） |
| （只读，Enrollment 区）NVVDD output OCP current ceiling | `nvvdd_output_ocp_max_ma` | `#13 nvvdd-current 1..378000..378000 mA` | `rows[13]`（378 A） |

## Current and protection

| vix GUI 字段 | vix key | nvoc 行 | nvoc 字段 |
| --- | --- | --- | --- |
| 12 V input current threshold, rated (advanced) | `input_current_rated_ma` | `#9 12v-input 1000..45000..45000 mA` | `rows[9]`（45 A；Ada rows 9/10 = 阈值 0/1） |
| 12 V input current threshold ceiling (advanced) | `input_current_max_ma` | `#10 12v-input 1000..45000..45000 mA` | `rows[10]` |

## Voltage（rail/VFE，`structural.tables_v3.rails`/`rel_limits`/`vmin`）

| vix GUI 字段 | vix key | nvoc 输出（4090_48G 实测） | nvoc 字段 |
| --- | --- | --- | --- |
| NVVDD REL fixed voltage 1050 mV | `nvvdd_rel_fixed_mv` | `rel limits: NVVDD REL max 1050.0 mV (Constant)` | `rail_limits[].output_maximum_uv`（常数方程 = clamp min=max=value） |
| NVVDD ALT fixed voltage 1070 mV | `nvvdd_alt_fixed_mv` | rail 行 ALT 选择器指向的常数方程（`rails[].selectors`） | 同上路径 |
| NVVDD REL and ALT follow OV (XOC) | `nvvdd_rel_alt_follow_ov` | rails 选择器 REL/ALT == OV 指向（`rail_equations` kind 判定） | 选择器序校验放行 `REL = OV` 形状 |
| 只读 REL equation ceiling | `nvvdd_rel_equation_max_mv` | `rel_limits` | 同 REL 行 |
| 只读 OV equation ceiling 1200 mV | `nvvdd_ov_equation_max_mv` | `rails[].selectors` OV → VFE 方程 clamp | `vbios_vfe` 方程表 |
| 只读 VMIN Unknown（"used by another equation or role"） | `nvvdd_vmin_mv` | `vmin: NVVDD: is used by another equation or role`（措辞与 vix 同源） | `rail_vmin[].diagnostic` |
| 只读 rail INFO VMIN-slot ceiling | `nvvdd_vmin_equation_max_mv` | rails 选择器 `VMIN_INFO`（row byte 11 / GB20x +0x21） | `RailRole::VminInfo` |
| 只读 NVVDD output OCP 378 A | `nvvdd_output_ocp_max_ma` | `#13 nvvdd-current 378000 mA` | `rows[13]` |

## Clock offsets（GV HAL 时钟表，`structural.*.clocks`）

| vix GUI 字段 | vix key | nvoc 输出 | nvoc 字段 |
| --- | --- | --- | --- |
| Core (GPC) offset ceiling/floor ±1000 | `gpc_offset_max/min_mhz` | `clock ranges: GPC -1000..1000` | `clock_ranges[].min/max_mhz`（C token +8） |
| XBAR / SYS / VID ±1000 | `xbar/sys/vid_*` | 同名行 | 同上 |
| Memory offset ceiling 6000 · floor −2000（"Windows: 3000/−1000"） | `memory_offset_max/min_mhz` | `memory offset (NVML x2 scale): -2000..6000 MHz (table -1000..3000)` | DRAM 行 ×2（NVML/Linux 轴 = 表值 ×2；MSI Afterburner 显示表值） |
| （GUI 无对应行）HOST −1000..0 | — | `clock ranges: HOST -1000..0` | private freq domain HOST 域运行时 cap（实测与 40 系 HOST 无法正偏移一致） |

## Fans（五表，`structural.*.fans`）

| vix GUI 字段 | vix key | nvoc 输出（4090_48G 实测） | nvoc 字段 |
| --- | --- | --- | --- |
| Fan channels 2 | `fan_channels` | `fan channels [0, 1]` | `FanReport.channels`（cooler 头通道列表） |
| Fan PWM frequency 27 kHz | `fan_pwm_frequency_hz` | `27000Hz` | `FanChannelRow.pwm_frequency_hz`（cooler 行 +11..13） |
| Fan N maximum/minimum speed 6000/2000 | `fan_N_max/min_rpm` | `2000..6000rpm` | `rpm_maximum/minimum`（cooler +14..17） |
| Fan N maximum/minimum level 100/30 % | `fan_N_max/min_pct` | `30..100%` | `level_maximum/minimum`（cooler +2/+3） |
| Fan N curve point K temperature/speed/level | `fan_N_curve_K_*` | 曲线 `(56C,2000rpm,30)(61,4000,66)(69,6000,100)` | `curve.levels[3]` + `curve.points[3]`（policy 条目 +14：levels 3B、(1/32°C,RPM)×3） |
| Extra fan curve 1/2（98..108°C / 92..102°C 热保护曲线） | `fan_extra_M_curve_*` | `fan extra curves: extra1: 98C/2000 100C/4000 108C/6000 \| extra2: 92C/2000 97C/4000 102C/6000` | `FanReport.extra_curves`（policy 表中活跃通道数之外的使能条目） |

## 头部元信息

| vix GUI | nvoc 输出 |
| --- | --- |
| `Format: pci-bit-npds-v3x` | `image_chain`（PCIR/NPDS 枚举）+ `tables_v3`（BIT v2 'P' token） |
| `Integrity: Verified`（逐镜像加性和 = 0） | `tables_v3.integrity`（GB20x 为 SHA384 对象目录，`gb20x.integrity`） |
| `PCI 10de:2684` / `ada` | `architecture.pci_device/family`（615.71.09 产品表门） |
| BIOS 95.02.3C.00.02 | `architecture.bios_versions` |

## Shunt multiplier（vix 显示端换算 ↔ nvoc ROM 内校准解码）

vix GUI 顶部的 "Shunt multiplier (x)" 是**纯显示端换算**（帮助文本
"Solo informativo: la VBIOS guarda los valores de los campos"——用户手填
倍率，SHUNT_KEYS 字段显示"真实量 = 显示量 × 倍率"），vix 未从 ROM 解出
任何 shunt 字段。nvoc 的增量发现：**功率通道条目 bytes 6..8（u16 Q4.12
定点，0x1000 = 1.0）就是每通道的感测倍率校准**——原始感测值 × scale =
实际值，shunt mod 改物理电阻后改这里即可恢复读数（无需 UI 换算）。
注意 vix 的通道模板（SLOT_CHANNEL 等）把该位钉死为 `00 10`，恰好漏掉
全部非标样本；nvoc 模板只校验 12V 电压前缀（bytes 2..6）。

实测分布（`shunt/sense scales` 行，只列 12V 感测族 ≠1.0 通道）：

| ROM | 通道 | scale |
| --- | --- | --- |
| 3090（含原版 371W） | ch19 NVVDD 相位输出（nvvdd-current） | **4.0**（出厂：相位级 shunt 阻值小） |
| 4070S 非公 320W | ch3 12V-2x6 连接器（12v-connector，rail 0xF7） | **2.0** |
| 4090 CC07B | ch2 12v-connector | **1.3333**（0x1555；shunt mod 特征） |
| 参考 4090/4060/2070 等 | 全通道 | 1.0 |

其它 class（如 class-2 求和通道）的同位字节语义未证实（vix 观察到求和
通道在该位置存别的值），不标 shunt。

## 未映射（vix 有、nvoc 只读解析刻意不带）

- 编辑/写回语义（vix 是编辑器：bounds/ordering group/opt-in 闸门）——
  nvoc 定位只读，字段以 `maximum_offset` 等原始偏移呈现，不做写回。
- GB20x 专属字段（output OCP rows13/14、slot row 18/19、`fan_zero_rpm`、
  override profiles 选中链）——nvoc 对应解码在 `structural.gb20x`。

## 核对

```sh
cargo run -p nvoc-core --example vbios_structural -- reverse/VBIOS/4090_48G_AD102_MAX_450W_1075mV_95023C0002.rom
cargo build -p nvoc-cli && ./target/debug/nvoc-cli.exe get-vbios \
  -i reverse/VBIOS/4090_48G_AD102_MAX_450W_1075mV_95023C0002.rom --verbose
```

集成断言：`core/tests/vbios_structural_roms.rs`（4090_48G：TGP
150/450/450、slot 66W、NVVDD threshold 600/900、VMIN shared 只读）。
