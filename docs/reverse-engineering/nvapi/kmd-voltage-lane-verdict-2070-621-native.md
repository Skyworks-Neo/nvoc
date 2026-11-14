# kmd 电压道判决:native-RM(2070/621.01)host 侧穷尽扫描阴性(2026-11-14)

**快照**:RTX 2070(TU104)/ 621.01 / GSP 固件模式关闭(native-RM)/ 本机 64GB。
动机:5060 Ti 电压墙 melonVolt payload 写全死(见 probe 判决),用户裁定试 kmd
内核道强制绕过用户态门。本判决 = kmd 道在 native-RM 下的可行性测定。

## 1. 方法论(全 host 可达域,六种手段)

工具:`core/tests/kmd_volt_policy_diff_live.rs`(scan/major/diff2)+
`core/tests/kmd_gsp_pool_sweep_live.rs`(values/poke/fixedregion/ptscan/
poke2/poke4/stepsdiff);传输 = PMXDRV_NEW(Intel 2019 签名款,sc 手工拉起)。

| # | 手段 | 域 | 扰动源 | 结果 |
|---|---|---|---|---|
| 1 | 镜像可写节扫描(动态节表) | nvlddmkm .data 等 9 节 1597 页 | 墙值元组 | 零候选(GSP 开/关两态一致) |
| 2 | 全镜像 before/after diff(diff2) | 同上 | slot2 +6250 + boost 100 | 365/514 变化点全为时序/fifo/计数噪声,零墙值 delta |
| 3 | board 定位器(Major 锚 + 指针跳 + 池邻域) | Major ±1 跳可达域 | 墙元组 | 1 静态描述符页(boost 扰动 STATIC) |
| 4 | 大池值扫查(SystemBigPoolInformation 全标签) | ≥1 页块 0.63 GiB 全量 | 墙 needle | 1 镜像页(target@0x390/vbios@0x3B0 poke 不随动=info 行) |
| 5 | 大池 delta-diff(poke4) | 同上 | slot2 +43750 | **零站点** |
| 6 | 页表全帧扫描(ptscan,CR3 四级手走) | 全内核映射帧 1.87M chunks | 墙 needle + 偏移 needle 双针 | 121 站点,26 个活体跟随(还原即回零),patch 全部 → 墙不随动;其余 856 = 陈旧历史环 |

## 2. 关键事实

1. **slot2 写在 Turing/621 完全生效**(retained + status vrm 墙 1:1 跟随,
   步进量化 6250µV 网格,+43750=7 步不被钳)——写车道活着,但**执行态不在
   任何 host 可枚举域**。
2. 找到的全部 host 副本分两类:①墙值镜像(info 行,poke 不随动);②偏移
   遥测环(856 个历史条目,patch 不随动)。**26 个活体跟随站点(还原即回
   零)patch 同步 +100mV → status 墙纹丝不动** —— 它们是遥测镜像不是消费源。
3. 步数编码判别(delta 1→7 交叉)仅命中 2 个毫秒计时器,patch 不随动。
4. 墙值镜像帧 **0x010EA10000 跨 GSP 开关两代重启同址** = 固定物理区域结构
   (非随机池分配);其 ±4 页在 slot2 扰动下零 delta。
5. USER 侧 A/B 对照:mVolt+/GUI 的 boost %(40B/0x10028 legacy 面)是唯一
   5060 Ti 上被消费的电压控制;slot2 在 2070 native 生效但在 5060 Ti(GSP
   默认开)死。

## 3. 判决

**native-RM 下 Turing 电压墙执行态不可由 host 内存修改触达** —— 与功率墙
(宽表在 NVRM 池,可 patch)形成代际+类别双重分化。机理推定:电压偏移在
SET 时被推给卡内控制器(PMU/电压控制器微码态),host 只留 GET 回显镜像与
遥测环;「墙」是 GET 时的计算值(基墙镜像 + 偏移),不存在可 patch 的单一
存储位。残余居所(若确有)= MDL carveout 中非直接映射域或卡侧硬件态,均已
超出内存修改车道边界。

**kmd 电压道判决:不可行(本代驱动)**。5060 Ti 电压墙抬升的可行车道收敛
到 vBIOS 输入层(mVolt+ BIOS override 同型:改 host 侧 vBIOS 派生限值 →
重启让驱动/GSP 重新消费),或下一代驱动布局复检。

## 4. 方法论资产(可复用)

- ptscan:CR3 四级手走 + 大页分块 + 双针 + 区域级即时还原验证 —— 页表枚举
  即安全白名单,无 RESOURCEMAP 残缺问题;本判决 1.87M chunks ≈ 617s。
- 坑×3:①CTL 帧掩码位数(PML4E 帧字段 bits 12-51,移位后 40 位;PKEY 位
  59-62 会污染短掩码);②read_page 收帧号要 <<12(地址/帧号单位错位);
  ③1GB 大页路径 PA 算错即 segfault(process 死,stdout 缓冲丢失 —— 进度走
  stderr)。
- 测试 exe 文件名按 target 稳定(非内容 hash),`ls -t` 选新的是对的,按字
  母序会拿旧二进制。

## 5. 对照系

- GSP 开启时同车道死得更快更彻底(判决书 gsp-mode-kmd-lane-verdict-2070-621-01.md);
- 3060/591.86 regtab「值不在可达域」判决(regtab-fifoctx-verdict-591-86.md)
  同构 —— RM 类策略态的多代际「host 不可达」演化在电压类上同样成立。
