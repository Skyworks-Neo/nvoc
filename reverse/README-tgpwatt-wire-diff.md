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
  `(policy_id, subtype, min, def, max)`、每一步的返回状态,外加:
  - `--- NVAPI ID 归属 ---`:每个私有 ID 的实现落在哪个模块(judge 毒族实现体
    在 nvapi64.dll 还是 nvapi64_impl.dll)。
  - `ntdll 传输 hook 安装` / `LIVENESS ...`:旁路 hook 是否装上、是否**被调到**
    过。存活自检为 false 时,"无内核传输"的结论无效(见下)。
  - `TRANSPORT[OPx] ...`:每一步 NVAPI 调用区间内落到**显卡设备**上的 ioctl
    条数。空 = 该步没有下行内核传输。归因按设备句柄过滤,进程里其它设备的
    DeviceIoControl 噪声单独计数,不会混进结论。
  - `CHANGED[...]` / `CARRIES[...]`:改动节里定位到的偏移,以及写入值(原值 /
    µW / Q16 / 低 16 位都试一遍)是否在改动字节里出现 —— 直接说明载体是明文
    还是索引/编码。
- `index.txt`:**每条 DeviceIoControl 的 ioctl 号、设备句柄、输入/输出尺寸、
  GUID 描述符原始字节**,并带 `seq=` 全局序号与 `tid=` 线程号;`--- TAG>`
  / `--- TAG<` 打点把每条 ioctl 精确归因到某一次 NVAPI 调用(写 / 回读 /
  control 探测分得清)。`via=ntdll` 前缀的行来自**绕过 kernel32 的系统调用
  旁路钩子**,`via=` 缺失的行来自 kernel32 内联钩子 —— 两者一起看才能回答
  "这次写走的是哪条下行通道"。
- `changed-*.txt`:毒写入前后**内容发生变化的映射节字节**(前/后 hex 对照 +
  `^` 标记 + 前后各 16 字节上下文)。毒 payload 不走在 ioctl 缓冲里,只能靠
  这种"前后指纹差分"定位;不依赖任何驱动私有 stamp,换机换驱动都成立。
- `SUMMARY.txt`:上面各项的汇总,单独回传这一个文件即可,配合
  `changed-*.txt`。
- `esc-*.hex` / `sec-*.hex` / `mapped-*.txt`:每次 ioctl 的原始输入输出缓冲、
  共享节快照。

## 传输通道判定(本次新增,回答"新驱动为什么不走 kernel32")

r590/r610 上 kernel32!DeviceIoControl 全程看不到任何写路径调用,只有两种解释:
新驱动的 SET 已变成共享节直写(无内核传输),或者它绕过 kernel32 直接调
`ntdll!NtDeviceIoControlFile`。为了能判定,抓包同时挂了两层:

1. kernel32!DeviceIoControl 的**内联钩子**(记录带 `dev=h=... cmd=...`);
2. `ntdll!NtDeviceIoControlFile` 的 **IAT 旁路钩子** —— 不写死模块清单,而是
   扫遍进程内所有确实导入该函数的模块(实测命中 kernel32 / kernelbase /
   advapi32;kernelbase 才是现代 Windows 上 DeviceIoControl 的实现体,只补
   kernel32 会整条漏掉)。

hook 内**不做 `println!`**:这个钩子会在任意线程上被调,在钩子内 `println!`
会以 `0xc0000005` 崩掉进程(与补丁无关,纯透传/计数/格式化都正常),所以记录
先攒在内存,测试末尾统一落盘。

附带结论:**只要 kernel32 内联钩子还在,`ntdll` 侧一定会重复记一次** —— 同一次
写入在 index.txt 里出现两行(kernel32 那行 + `via=ntdll` 那行)属正常,读的时候
按 `via=` 区分通道即可。

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
