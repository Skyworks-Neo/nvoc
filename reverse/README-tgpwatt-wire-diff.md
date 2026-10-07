# TGPWatt 写入传输对照抓包(30 系 / 毒机对比)

目标:在**毒路径可用**(30 系)的对照机上,用同一条命令序列复现我们的写入,
回收它的 ioctl 传输、身份版本、以及毒写入真正改动的映射节字节,与毒机
(RTX 2070 / 驱动 610.47)的抓包逐字节对比,定位代际差异。

## 跑法

必须**提权**(否则写路径在 `Nvapi64_impl` 下发 ioctl 前就以
`InvalidUserPrivilege` 返回,抓到的只有 GET,没有对比价值 —— 测试开头会打印
`elevated = true/false` 并在未提权时给出警告)。

```powershell
pwsh -File reverse\run-tgpwatt-wire-diff.ps1
```

脚本会清空并重建 `%TEMP%\nvoc-esc-capture\`,以管理员身份运行
`cargo test -p nvapi --test esc_capture tgpwatt_wire_diff -- --ignored --nocapture`。
等效的手工命令(先 `$env:NVOC_ESC_CAPTURE` 指向输出目录):

```
cargo test -p nvapi --test esc_capture tgpwatt_wire_diff -- --ignored --nocapture
```

`--test esc_capture` 只编译运行这一个 test 目标,不会牵动其它测试。

## 抓了什么

- `report.txt`:GPU 型号/架构/PCI ID、NVAPI 与驱动版本、`nvapi64.dll` /
  `nvapi64_impl.dll` 的文件版本、每个 power channel 行的
  `(policy_id, subtype, min, def, max)`、以及每一步的返回状态。
- `index.txt`:**每条 DeviceIoControl 的 ioctl 号、设备句柄、输入/输出尺寸、
  GUID 描述符原始字节**,并带 `seq=` 全局序号与 `tid=` 线程号;`--- TAG>`
  / `--- TAG<` 打点把每条 ioctl 精确归因到某一次 NVAPI 调用(写 / 回读 /
  control 探测分得清)。
- `changed-*.txt`:毒写入前后**内容发生变化的映射节字节**(前/后全文 hex)。
  毒 payload 不走在 ioctl 缓冲里,只能靠这种"前后指纹差分"定位;不依赖任何
  驱动私有 stamp,换机换驱动都成立。
- `SUMMARY.txt`:上面各项的汇总,单独回传这一个文件即可,配合
  `changed-*.txt`。
- `esc-*.hex` / `sec-*.hex` / `mapped-*.txt`:每次 ioctl 的原始输入输出缓冲、
  共享节快照。

## 本次序列(与毒机逐字节对齐)

1. GET 基线:`tgp_watt_range` / `tgp_watt_status` / `power_channel_policies` /
   `power_channel_control`(纯读)。
2. OP1 安全对照:`set_power_limit(100%)` —— 历史上的 percent 接口。
3. OP2 **毒**:`set_tgp_watt(180, idx 2)` —— 绝对 TGP 写(`0xAFFC2279`)。
4. OP3 **毒**:OCP 电流行绝对写(`(6,19)`,写 205860 mA)。
5. OP4 **毒**:板功率行绝对写(`(0,9)`,写 195000 mW)。
6. 复位 percent 100% + 复位 OCP。

## 注意

- 在**毒机**上每个毒写会挂死 GPU 并触发一次 TDR(约 4.2 s),一次跑完约 4 次;
  两次运行之间请间隔 ≥30 s。在 30 系对照机上应全部秒回、无 TDR。
- 测试末尾会把 TGP 复位为 percent 100%;OCP 行会复位为默认值。
