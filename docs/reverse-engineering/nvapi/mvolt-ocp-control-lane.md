# mVolt+ 引擎二 Board-power CONTROL 车道全解（OCP Apply 调用链回溯）

日期：2026-11-16 · 样本：mVolt-0.50.13.exe（IDA idb，地址基 0x140000000）
实测：RTX 2070 / TU106 / 621.24（dev box，只读探针全通）

## 0. 一句话结论

mVolt 的 OCP/TGP 运行时写 = **同一对 QI ID（0x8B3E7343 GET / 0xAFFC2279 SET）
的第三种 stamp 形态**——"Board-power CONTROL 大包"（class-1 Turing 域
0x2786E0/0x2486E0、class-2 Blackwell 域 0x5B0B0/0x4B0B0），request 写点
entry+68；前置大表回读走 **0xC12EB19E**（不是 0x33AB0353——上一轮
「Turing v3|3240 全 -9」判决探错接口，作废）。Turing 实测：GET 全通，
SET = InvalidUserPrivilege（提权即通，非 fault——与 compact 0x0001_0A4C
在 pre-Ampere 上的 nvlddmkm 14/153 fault 行为形成代际对照）。

## 1. UI→写面完整调用链（IDA 反编译溯源）

```
OCP limit / Power 面板 Apply（mVolt UI）
 └→ GPU power 宿主构造 sub_14026A010（← sub_1401F5C00 ← UI proc/WinMain）
 │   · 四个 QI fnptr（各自 VM blob 独立解码，见 §2）
 │   · 引擎二 sub_14027EF30(a1+48)（8 fnptr mini blob）
 │   · sub_14026ABE0 双读一致性校验 → 绑定缓存 a1+248..360
 └→ Apply = sub_14026B4D0（仅经 vtable @0x1404415d0/0x1404415dc 分派；
    pending 选择器位图 @a2+120..124）
 │   Restore = sub_14026B960（"Board-power restoration did not retain
    the original requests."——按 selector 3→2→1→0 逆序重放保存值）
 └→ RMW 核心 sub_14026AFF0（每 selector 一轮）
     ├─ selector 0/1 → §3 CONTROL 大包车道
     └─ selector 2/3 → §5 F8/FE 功耗请求车道
```

执行器 sub_140280C00 = "Power-request records changed during readback"
双读一致性包装（实读在 sub_140280E70，身份缓存 engine+104..112）。
写后保留性判决：CONTROL 车道 12 轮 ×100ms 轮询（a1[47]=Sleep）连续 2 次
一致；F8/FE 车道 16 轮 ×100ms。

## 2. 四个 QI ID（host 层，VM blob 解码）

构造器 `sub_14026A010` 的解码器 `sub_1401D2EE0(blob, key)`：seed =
`key | 0x891F610E00000000`，keystream = `scramble(seed - 0x61C8864680B583EB·(pos+1))
>> 56`（splitmix 族：a=0xBF58476D1CE4E5B9·(x^x>>30)，b=a^(a>>27)，
ks=0x94D049BB133111EB·b >> 56）；opcodes 2=XOR varint4 / 67=ADD varint4 /
79(0x4F)=RET / 132=ROL 单字节 / 193=MOV varint4，≤18 字节；varint = 4B LE。
解码脚本 reverse/mvolt+/decode_host_blobs.py。

| host 槽 | key | QI ID | nvid.rs 名 | mVolt 角色 |
|---|---|---|---|---|
| +16 | 0xF9E545D3 | **0x67F31384** | ClientPowerPoliciesGetInfoPrivate | INFO 类探测（vtable[0]） |
| +24 | 0x7E4401A4 | **0xC12EB19E** | PowerMonitorGetInfo | 大表回读 GET（v3\|3240，stamp 199848） |
| +32 | 0x5097E0D8 | **0x8B3E7343** | ClientTgpWattGetStatus | CONTROL 大包 GET |
| +40 | 0xC1F3D087 | **0xAFFC2279** | ClientTgpWattSetStatus | CONTROL 大包 SET |

fnptr 调用约定 = `fn(handle, buf)` 双参直调（sub_1401D3C40）；SET 前过
packet 校验器 switch `sub_1401D32C0(kind, ...)`（kind 27 = CONTROL 大包，
kind 20/21 = v1|1320 F8/FE，kind 22 = thermal 10444B，kind 0/2/3/… = 其他族）。
0x33AB0353（引擎 mini blob B）/0x17695269（mini blob C）= 引擎级
ClientPwrPolicies GetControl/SetControl（v1|1320 读回/写），**不是**大表回读。

## 3. CONTROL 大包车道（selector 0/1）

### 3.1 类探测与几何（描述符 unk_140374B60=class1 / unk_140374AD0=class2）

`sub_140251EE0(&{fnptr,handle,err,buf}, class)`：INFO stamp（desc+8）前置进
零缓冲（长 desc+16）GET 0x67F31384；**错误码 == -9
（INCOMPATIBLE_STRUCT_VERSION）→ 返回 1 → mVolt 自动降级另一 class**；
其他错误 → 硬失败（"Power-policy INFO failed (NVAPI N)."）。

| 字段（desc 偏移） | class 1（Turing/pre-Blackwell 域） | class 2（Blackwell 域） |
|---|---|---|
| +0 class id | 1 | 2 |
| +4 掩码 dword 数 | 8（256 通道） | 1（32 通道） |
| +8 INFO stamp | 0x2BA030（2,859,056） | 0x0F4BF4（1,002,484） |
| +16 INFO buf | 0x2BA030 | 0x54BF4（347,124） |
| +48 CONTROL stamp | 0x2786E0 | 0x5B0B0 |
| +56 CONTROL buf | 0x2486E0（2,393,824） | 0x4B0B0（307,376） |
| +64 掩码偏移 | 0x88 | 0x88 |
| +72 entry 偏移 | 0xA68 | 0xA30 |
| +80 entry stride | **0x2448（9,288 B）** | 0x2448（同） |

### 3.2 packet 构造（sub_14026A340）

GET：stamp@+0 → 单通道掩码 `1<<ch` @mask_off+4·(ch/32) → entry 头
request = 0xFFFFFFFF → 0x8B3E7343。SET：RMW 后 patch entry+68 → 写掩码
保持单通道 → 0xAFFC2279（kind-27 校验器 sub_1401D2640 要求：恰 1 通道置
位、entry+0 identity ∈ {0,23}、entry+68 > 0、identity=23 时
sub_1401D2310 过 + entry+1116 > 0）。

### 3.3 2070 实测 identity 地图（2026-11-16 只读探针）

| ch | identity | request | 备注 |
|---|---|---|---|
| 0 | 6 | 9999000 (0x989298) | = **9999 W**（用户此前 9999W 写残留） |
| 1 | 3 | 1001000 | |
| 2 | **0** | 9997750 (0x988DB6) | identity-0 行；= v1|1320 FE 读回同值（两车道共享底层状态） |
| 3 | 3 | 73997 | 电流型行 |
| 4 | 3 | 92994 | |
| 5 | 3 | 166994 | |
| 7/8/9 | 4 | 16499/21748/33747 | |
| 11 | 11 | 0 | |
| 13/14/15 | 6 | 9206000/9999000/9999000 | identity-6 族共 4 行 |
| 其余 | — | — | 拒（-1） |

**identity-23 在 Turing 不存在**（全 32 通道扫描无命中）——mVolt 的
dword_1403C0870 = [0, 23] 双选择器中 23 是 Blackwell 面行；Turing 上面板
实际映射待大表解析通道号确认（v3|3240 表已 GET 成功，解析待下一轮）。
INFO class1/class2 在 Turing **双通**（mVolt 语义取 class1）。

### 3.4 SET 判决

非提权 shell：`InvalidUserPrivilege`（干净权限门，**不 fault**）。
mVolt 提权运行 → 写通（与用户实测 Index 13 被写入吻合）。对照：compact
stamp 0x0001_0A4C（v1|2636）的 SET 在 pre-Ampere 上是 nvlddmkm 14/153
驱动 fault——**同 ID 异 stamp 的代际行为分野**：Turing 消费大包形态。

## 4. 大表回读（A670 前置）

`sub_14026A670`：INFO 类探测 → `memset 3236 + stamp 199848 (v3|3240)` →
0xC12EB19E GET → 失败则 A670 整体失败（无 fallback）→ 双读一致性 →
CONTROL 双通道读 → min/max 收集 + 绑定校验（"Board-power range or policy
binding is not supported."）。**2070 实测：0xC12EB19E + 199848 = OK**
（3240B 全填充）；0x33AB0353 + 同 stamp = -9（勘误实证）。

## 5. F8/FE 功耗请求车道（selector 2/3）

`sub_140281500(engine, value_watts, 248|254, …)`：v1|1320 模板
（sub_14027EE40）→ 值写 `40·(ch+1)` = entry+32，类型码 248（F8 = "Power
cap must be positive and no higher than the driver's current maximum (N
W)."）或 254（FE = "An absent ordinary power request cannot be restored
exactly."）@ entry+36 → SET 0x17695269（kind 20/21 校验：mask 恰 1 位、
entry+36 == 248/254、entry+32 ≠ 0/0xFFFFFFFF）→ 16 轮 ×100ms 读回（F8 读
entry+32、FE 读另一 dword）。2070 实测：ch0 FE = 9997750（= CONTROL ch2
identity-0 同值）。**OCP 不走这条**——F8/FE 只管功耗请求。

## 6. nvoc 落地

- `nvapi-rs/src/gpu.rs`：`PowerControlGeometry` + `POWER_CONTROL_CLASS1/2`
  常量；`power_readback_table_v3()`（0xC12EB19E）、
  `power_info_table_probe()`（0x67F31384 类探测）、
  `power_control_table()` / `set_power_control_entry()`（大包 GET/SET）
- 探针 `nvapi-rs/tests/power_control_lane_probe.rs`（GET-only 默认；
  `NVOC_POWER_CONTROL_WRITE=echo|<u32>` + `NVOC_POWER_CONTROL_WRITE_CH`
  门控写臂，echo = 零改动保留性验证）
- 待办：提权 echo 验证 SET 保留性 → identity-23/OCP 行定位（v3 表解析）
  → CLI 封装（对应 set-pwr-cur-limit 的 class-1 替代车道）

## 7. 勘误链

1. 「Turing v3|3240 GET 全 -9」（power_policies_v3_scan）——探的是
   0x33AB0353（v1|1320 读回接口），真接口 0xC12EB19E 实测 OK。判决作废。
2. 「pre-30 OCP 唯一路径 = romOverride patch」——CONTROL 大包车道在
   Turing GET 全通、SET 仅权限门，提权即通。判决软化：romOverride 仍是
   持久化路径，但**运行时**路径存在（= mVolt 实证）。
3. 「set-pwr-cur-limit 打爆 30 系前的卡」——那是 compact 0x0001_0A4C
   形态的行为；大包形态不 fault。
