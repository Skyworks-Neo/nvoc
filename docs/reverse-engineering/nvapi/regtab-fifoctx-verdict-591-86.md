# 3060 / 591.86 regtab 通道判决档案(2026-10-08)

RTX 3060(Ampere,GA106)/ 591.86 驱动的功率窗逆向收官档案。结论:**内存写
武器全部打不到窗顶读源,30 系窗顶(212 W)是本代驱动的结构极限**;后续若
继续,方向是用户裁定的 **PCI MMIO 卡侧寄存器伪造**(限制在卡上,而非内核态
驱动内存),见文末「前瞻」。工具:`core/tests/kmd_regtab_probe_live.rs`
(提交 708f867/3442984,REGTAB_DENSE=1 密采)。

对照:2070/610.47(Turing)与 P100/582.41(Pascal)的宽表抬窗在本轮
**不受影响**——两者验收是行为级(`-q` Max 读数 + NVML 到达),且本轮负结果
恰好支持其模型:610.47 上 regtab 通道不承载窗值,窗钳读源在宽表。

## 1. 静态公式链(591.86,nvlddmkm__591.86.sys,idalib)

regWrite032 = `sub_14011FAD0`,头部 `mov rbx,[rcx]` —— a1 是**指向大对象的
指针**,`*(obj+0x4280)` = fifoctx 指针;**fifoctx 本体即 RM regtab ctx**
(旧结论"再解一层 +0x4280"作废)。

- **region 表**:`region = *(fifoctx + 0x1F00 + 8*chanIdx)`,chanIdx 0..12
  (≥13 拒;chan0 未建时惰性 = `fifoctx + 0xDB8`,内联)。
- **写路径**(慢路径,fast-flag=0):`sub_140309BB0` 按 (chan, subchan,
  addr∈[lo,hi]) 走 **region+40 回调链**(节点 `{next@0, w8@8, mask,
  chan@+12, sub@+16, addrLo@+20, addrHi@+24, fn@+32}`);成功后
  `sub_1400E3530` 镜像:**`*(u32*)(region + 4*(regaddr>>2)) = val`**。
- **快速路径**:`fifoctx+0x8AB flag==1` → `+0x20E0` backend →
  `backend+0x3B50` wrapper → `wrapper+0x50` reg 对象 **vtable[+64]
  write32(addr,val) 直写硬件**。
- 0x140DA5710 = RM 对象类注册链头(0x20 步距巨型 class 表
  `{classId, paramsSize, link, handler, flags}`),非 cmd 分发器;
  0x2080A625 的 cmd 字节全图无命中 = control call 的 cmd 由 wire 运行期
  带入,静态表里本来就没有。

## 2. 活体验证(自指签名,零额外读)

**签名**:页 P 内 `qword[i] == P + i*8 − 0x1148`(0x1F00−0xDB8)⇔ 槽 =
fifoctx+0x1F00、槽值 = fifoctx+0xDB8。命中后 **fifoctx = 槽值 − 0xDB8**
(首版探针 push 了槽值本身,dump 错位到内联区、全 0 假象——教训)。

| 机器 | fifoctx | 备注 |
|---|---|---|
| 2070/610.47 | **Major+0x90** | chan0 内联 region @Major+0xE48 ✓;回调 1 个 [0x400000,0x600002] fn RVA 0xba0910 |
| 3060/591.86 | **Major+0x0** | 同族偏移不同;chan0 内联 ✓;回调 **7 个**,fn 同 RVA 0xb28890 |

3060 的 7 个回调范围:`[0xcc00,0xcc18]`、`[0x10c000,0x10e0ff]`、
`[0x120800,0x127fff]`、`[0x132800,0x1ff1ff]`、`[0x244000,0x27bfff]`、
`[0x400000,0x5fffff]`、`[0x1018300,0x11befff]`。

## 3. 差分实验(3060,`-pl 150`)

扩展域(root/Major/宽表三锚一跳,~440 页)扫到**第二活体副本**:
页 `0xFFFFDD0B967E1000` —— cur **212000→150000 @+0x900/0x904 跟动**,
min 100000×4 @+0x7dc..0x7f4(步距 8);**无 def/max**(差分后 212000 在此页
0 命中)→ {min×4, cur×2} 状态页形态,非完整窗行。宽表(0xFFFFDD0B9674C000)
ctrl 跟动 @+0xA6C/0xA78 照旧。两处跟动副本都是写面;"跟动≠读源"(2070
echo cell 教训)。

## 4. 判决:窗顶 max 无 RAM 副本

判决轮(REGTAB_DENSE=1,cur=150 保持识别度)对 regtab chan0 shadow 全可达域
扫描,**全部零命中**:region 首页、≤4KB 回调范围精读(含 0xCC00 小窗——
密采样空档,槽值 k 从 0x10000 起步)、4KB..64KB 补扫、16MB 密采样(步长
64KB)、backend 首页。残留缺口:`[0x1018300,0x11befff]` 的 16–18.7 MB 尾部
(FIFO/引擎区形态,置信度影响极小)。

判决:**max 读源在 FIFO 事务/硬件寄存器后面**——与静态结论(mode 1 =
regtab 抽象、快速路径 vtable 直写硬件)吻合。已证伪的写面:宽表 max 槽
(写了 -pl 仍拒 [100,212])、regtab shadow(无值可写)、NVML/percent/watt
SET(全被 [100,212] 钳,瓦特 SET Applied 212000)。root 武装路线与 2070 同
死路(root 未武装、board 配置缺席)。

## 5. 前瞻:PCI MMIO 卡侧路线(用户裁定)

内存魔改不生效的本质是**读源在卡上,不在内核 RAM**。后续方向 = 伪造寄存器
访问经 **PCI MMIO** 向卡发送控制指令,把修改限制在卡侧而非内核态驱动内存。
PMXDRV 传输层天然支持:BAR 映射可经 `MAP_PHYS` 按物理地址直达
(`docs/../kmd/README.md` IOCTL 码表),无需新传输。立项前需先解决:
BAR 基址发现(PCI_CFG IOCTL 未接线)、窗寄存器的 MMIO 偏移(从 591.86
regtab 快速路径 vtable 实现回溯)、以及与 shunt mod 硬件面的配合验证。
风险面与 4060L 任务书 §0.4 同纪律:identity → 扰动 → 读回 → 恢复。

## 6. 证据

- 3060 判决轮探针输出:`evidence/regtab-probe-59186-verdict.txt`
- 2070 三轮验证输出:会话留档(本地 reverse/,未入库)
- 静态轨迹:idalib 会话 326cce04(nvlddmkm__591.86.sys.i64)
