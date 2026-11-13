# VBIOS parser 补齐：DCB 指针子表族 + BIT 'P' v2 功率子表 + token 级子表 + FWSEC DMEM

日期：2026-10-25。分支：`vbios-parser-enhanced`（基于 8353da3）。

## 动机与信源

以「VBIOS 解析器完整性」为镜头全量考察 `reverse/` 五仓（envytools /
gpu-unlock / gpu-unlock-ada / gpu-unlock-4090-48g-64g-bar1 / cmp170hx，总体结论见
[`../gpu-unlock-ecosystem-survey.md`](../gpu-unlock-ecosystem-survey.md)），对照
nvoc-core 的 vbios 解析面得出 gap 清单并补齐。三份一手信源：

1. **envytools nvbios**（`reverse/envytools/nvbios/`，HEAD=946d7113 "RIP,
   Marcin."，覆盖到 Turing 为止）：bit.c/power.c/gpio.c/i2c.c/extdev.c/
   conn.c/mux.c/d.c/mem.c/info.c/nvbios.c/bios.c。
2. **nouveau nvkm/subdev/bios/i2c.c**（torvalds/linux master，Web 抓取）：
   DCB 4.1+ I2C 条目的 PMGR 语义一手定案——envytools 的枚举（0=VGACR、
   4=PCRTC、5=PNVIO、6=AUXCH）停在 Turing 前，不含实测 0x10/0x30 值。
3. **gpu-unlock `kernel/gpu_unlock/vbios_610.c`**（两仓同源，注释声明来自
   NVIDIA `kernel_gsp_fwsec.c` + `kernel_gsp_vbios_tu102.c`）：FWSEC DMEM
   interface / FRTS mapper / 签名槽选择公式。

## 新增模块（core/src/vbios/）

| 模块 | 内容 | 信源 |
|---|---|---|
| [`dcb_ext.rs`](../../../core/src/vbios/dcb_ext.rs) | DCB v3/v4 头指针子表族：I2C 端口表（4.1+=PMGR u32 低 10 位 drive/auxch、0x30/0x40=类型字节）、GPIO 表（v0x10-0x41 全版本位段 + bios.h L282-534 全量 tag 名 + XPIO/XPIODIR）、EXTDEV 外设表（bios.h L643-699 全量类型名：INA3221/INA219/ADT7473/TMP411…）、CONN 表（type/tag + HPD 位图 {12,13,16,17,24,25,26} + DP_EXT {14,15}）、MUX 表（v0x10 四通道） | envytools gpio/i2c/extdev/conn/mux.c + nouveau i2c.c |
| [`power_p2.rs`](../../../core/src/vbios/power_p2.rs) | BIT 'P' v2 u32 槽位子表：**PowerBudget**（P+0x2C，v0x10/0x20/0x30，cap_entry 即 nvidia-smi 功率墙条目）、**PowerSense**（P+0x28，v0x10/0x20，INA219/3221 mohm+config 载荷位）、**PowerBaseClock**（P+0x38，v0x10/v0x20，boost/turbo/rated_tdp 命名条目）、**Voltage**（P+0x0C，v0x10-0x50 全版本几何）、**VoltMap**（P+0x20）、**ThermalDevices**（P+0x14）、**ThermalCoolers**（P+0x18） | envytools power.c + nvbios.c |
| [`bitsubs.rs`](../../../core/src/vbios/bitsubs.rs) | BIT 目录全量枚举 + token 级子表：**'M'**（v2 奇数字节偏移 u16 指针 +1/+3/+5/+9/+0xD，group_count u8@+0，TYPE 表 RAM 类型）、**'I'**（NVINIT，**u16 指针阵列**，槽名表）、**'C'**（PLL limits 指针 u16@+8）、**'d'**（DP INFO v0x40-0x42 头+vswing）、**'2'**（i2c script）、'T'/'U'/'V' 的 u16 槽如实携带 | envytools mem.c/info.c/nvbios.c/d.c |

## 既有模块扩展

- **image_chain.rs**：`RomImage` 增加 `pcir_rev`（2.2/3.0）、`init_blocks`、
  EFI 头 +8/+0xA/+0xC 的 subsystem / machine（0x8664=x64）/ compression
  （1=NVIDIA 自定义压缩容器——1.rom 的 GOP 镜像实测 compression=1，把
  「GOP 压缩存放」的三墙实验结论变成机器可读字段）。
- **legacy.rs**：`FalconUcodeEntry` 增加 `desc_offset`（绝对偏移）与
  `dmem: Option<FwsecDmem>`——按 gpu-unlock `fw.image_off = off + size`、
  `data_off = imem_load_size` 公式定位 DMEM，解析 RM interface 表
  （v1/hdr4/stride8 + (u32 type, u32 dest) 条目）与 type=4 的 FRTS
  mapper（magic "DMAP"、V3、cmd_offset/cmd_size、+44 标记）。TU106 实测：
  FWSEC_PROD interface 双条目 (4→0x380 mapper, 5→0x364)，命令块 ROM 内
  全零（页号/页数由驱动运行时填充——与 gpu-unlock patch_frts 的写路径
  互证）。legacy 新增 `relocate_ptr` 公共助手（EFI 间隙重定位）。
- **dcb.rs `primary()` 修复**：优先 legacy 镜像 +0x36 处 u16 指针锚定的
  DCB（envytools dcb.c 语义）。此前在 TU106 上启发式扫描**把 EXTDEV 表
  误判为 DCB**（ver/hlen/cnt/rlen 同形、其后 CONN 表恰巧通过 connector
  行检查，而真块 +0x521F 紧跟的 I2C 表被误拒）——get-vbios --verbose 的
  dcb 段在 TU106 系 ROM 上此前输出的是 EXTDEV 的内容。

## 与 envytools 的三处分歧定案（TU106/GP102 实测）

1. **BIT 'I' 是 u16 指针阵列**：17×u16=0x22 与 TU106 token len 精确吻合；
   envytools 按 le32 重叠读（eoff/eoff+2/…）在 Turing 上出垃圾。
   envytools 自己的 some_script@+14 / init96@+16 偏移恰为 u16 槽 7/8，
   支持 u16 读法。TU106 槽值：init_script=0x462F、condition=init_function
   =0x4633、io_flag_condition=0x4A0B、some_script=0xD37D、init96=0x4A0B。
2. **BaseClock v0x20 子条目时钟编码**：envytools 记 ×4，GP102 实测
   boost/turbo 槽 u16×2 才落 1480/1582（1080 Ti FE 标称 base/boost）——
   解析器只报告原始 u16 不做 MHz 断言（`subentries_hex`）。
3. **VOLT MAP v0x30**（TU106 P+0x20）：envytools 版本族外，只报几何
   （hlen 9/rlen 0x19/20 条目），条目不臆测。

## PowerBudget v0x30 实证（TU106 1.rom，RTX 2070）

cap_entry=2 → 条目 tag 0x21：min 105000 / avg 175000 / peak 219000 mW，
与该卡 175W 标称、219W 上限（+25%）、105W 下限（-40%）完全吻合；其余
条目为分通道预算（0x0A OCP 5001000=哨兵、0x04 通道 63000/74000 等），
与 tables_v3 power_channels 的通道角色可交叉对照。

## 实测对照面（回归锚）

- TU106（`../1.rom`，RTX 2070 90.06.18）：DCB v4.1 @0x521F；I2C 15 端口
  全 PMGR；GPIO 32 行含 FAN_TACH@13、FAN_PWM@16（param=1）、HPD_0/2/3/4/5、
  VID_PWM0；EXTDEV 32 条全 UNUSED；CONN 5 非空 = DP×2/HDMI/USB-C/HDMI。
- GP102（`../reverse/gpuz/GP102.rom`，1080 Ti 级）：EXTDEV 唯一非空 =
  **INA3221@0x80**（板载功率计，与 get-i2c 扫描可交叉验证）；GPIO v0x41
  rlen=5。
- 新测试：`dcb_ext::*_when_present`、`power_p2::power_p2_tu106_when_present`、
  `bitsubs::bitsubs_tu106_when_present`、`legacy::falcon_turing_fwsec_when_present`
  扩展 DMEM 断言。

## CLI 呈现

`get-vbios --verbose` 新增三段（JSON + 人读）：

- `dcb_ext`：header_ptrs / i2c 端口（PMGR drive/aux）/ gpio 命名线 /
  extdev 外设 / conn / mux；
- `power_v2`：budget（cap_entry 三元组）/ sense / base_clock（roles+条目）/
  voltage / volt_map / thermal_devices / thermal_coolers；
- `bit_tokens`：16 token 目录 / 'M'（groups+RAM 类型分布）/ 'I' u16 槽 /
  'C' PLL / 'd' DP INFO / '2' / 'T'/'U'/'V' u16 槽。

image_chain 每镜像新增 pcir_rev / efi_subsystem / efi_machine /
efi_compression；falcon 条目新增 desc_offset + dmem（interface/mapper）。

## 顺带修复

- **nvapi-rs shim**：bd2cff8 把 `nvapi::hi::AdcDevices` 做成了
  `AdcDevicesInfo` 的别名，而丢失的 8fc2fbe 形状是 `{info, status}` 组合
  ——pynvoc `query_adc_devices` 因此挂掉。子模块内修复为真结构
  （info=0x68789E2A 目录 + status=0x43D9B26A 读数），gitlink 随本提交更新。
- **pynvoc 导出面**：`nvapi_doctor`/`query_adc_devices`（916bbf8 引入）
  未登记进 `__all__`/`EXPECTED_EXPORTS`/原生 re-export，补齐。
- 本机 `cargo xtask ci` 的 GUI 侧 `test_bare_pre_map_geometry_survives_map`
  因 Python 3.14 环境的 Tcl/Tk 安装损坏（`Can't find a usable tk.tcl`）
  失败——环境问题，与本改动无关，CI 正常环境不受影响。
