# GPU 解锁生态五仓全量考察——结构、技术基础差与可借鉴项

- 日期：2026-10-25
- 信息源：`reverse/envytools`、`reverse/cmp170hx`、`reverse/gpu-unlock`、`reverse/gpu-unlock-ada`、`reverse/gpu-unlock-4090-48g-64g-bar1`（本地检出）+ [TOTORONG/AD103_win_unlocker](https://huggingface.co/TOTORONG/AD103_win_unlocker)（HuggingFace 模型卡与 INSTALL/ANALYSIS 文档）
- 方法：每仓库独立深挖（源码/文档/git 考古）+ 本机 `1.rom`（TU106 RTX 2070, 90.06.18.00.EE）实弹拆解实验
- 关联：[tu106-fwsec-ucode-decode.md](vbios/tu106-fwsec-ucode-decode.md)（本轮 parser 实战）、[windows-uefi-pre-driver-unlocker-dev-plan.md](uefi/windows-uefi-pre-driver-unlocker-dev-plan.md)（后续规划）

## 1. 执行摘要

这套生态与 NVOC 长期互不依赖的根本原因是**层级错位**：他们攻击 NVIDIA 硅片自身的固件引导协议（驱动加载*之前*的状态，跨驱动版本恒定），我们操作驱动运行时的私有数据结构（NVAPI/RM、kmd 宽表，随驱动版本轮换）。他们"不需要电气/硅经验"是真命题——这是纯数字系统的能力解锁问题，弹药来自二进制安全而非模拟电子；其边界也恰好死在真正需要硬件工程的地方（PCIe Gen3/4 熔丝、NVLink、ECC）。对我们最有价值的单项不是具体技术，而是**验证工程与认识论纪律**（mock 硬件测试、只信读回、死结台账）。

## 2. 谱系与角色

| 仓库 | 角色 | 规模/时间线 | 技术内核 |
|---|---|---|---|
| `envytools` | 14 年知识底座 | 5,330 commits / 71 人 / 2009–2023 | rnndb XML 寄存器库（~33k 行，带芯片变体标注，止于 Maxwell）；envydis（fuc0–fuc6 全系 Falcon ISA）；nvbios（BIT/boost/cstep/power 表解析）；nva 探针工具箱；demmt（blob 驱动 ioctl 追踪 oracle） |
| `cmp170hx` | 方法论范式 | 社区战役 2026-03→07，五周出可用 64G 解锁；wiki 5 commits/5 个月=纯发布事件 | 27.8 万字技术百科：对签名 SEC2 Booter 的 data-only ROP 全解；`appendix/methodology.md`（2,398 声明→1,458 正典事实、代码>文档>人言权威序、507 死结台账、266 开放问题按关闭成本排序） |
| `gpu-unlock`（Zagitta） | 工程化交付 | 23 commits / 2026-09-28→10-24 | Linux kmod：initramfs 里抢在 `nvidia` 绑定前，徒手重放完整安全引导（GSP Falcon FWSEC-FRTS→SEC2 V67 ROP 开 `FEAT_OVR_PLM` 0x823804→主机写 SS0=0x88888888@0x82381c / SS1=0x8@0x823820→FLR 交还原厂驱动）；行为级假 MMIO 62 场景 mock 测试；全链 SHA-256 钉源 |
| `gpu-unlock-ada`（JustAResearcher） | 验证者极致样本 | 6 commits / 4 小时发布窗（2026-10-23） | Ada 四写 PLM 链（载荷源自 TOTORONG Windows UEFI 解锁器，哈希钉死后逐字节复现+独立栈审计）；纠正公开 gist 的 AES 零控制向量错误；实测 TF32 也翻倍、FP8/INT4 不动；解锁=易失覆盖（熔丝仍读半速，FLR 后存活，断电即失） |
| `gpu-unlock-4090-48g-64g-bar1`（Ajax Dong） | 24h fork 扩展 | 8 commits / 2026-10-23→25，技术增量 1 commit +1006 行 | 魔改 4090 48G 的 64GiB BAR1：主机写 CYA 0x88724/CFG 0x88dcc 被硬件无视→必须 SEC2 载荷内越权写；ReBAR 三态（能力通告/尺寸控制/OS 分配）分立验证；嵌套 switch 下 ENOSPC→手排桥窗口（每口 128G、根口 576G） |
| TOTORONG/AD103_win_unlocker | Windows 侧源头 | HF 发布，v0.4.18/RUN19 | BOOTX64.EFI U 盘交付，事务与 Linux 版逐字同构；独创性全在载具层（详见规划文档） |

git 历史多为"发布事件"而非工作历史——真正的载体是社区频道+哈希钉死的制品，仓库只是快照。

## 3. 生态为何独立存在（四因）

1. **操作层级**：驱动之下（硅片引导协议）vs 驱动之侧（运行时结构）。GA100 booter_load 在 580→610 五个驱动分支间逐位相同——攻击版本恒定的层天然免疫驱动更新，这是 gpu-unlock 不需要随驱动更新打补丁的结构性原因，也是它不需要依赖任何超频工具的原因。
2. **目标性质**：一次性能力解锁（产品 Segmentation 反转：张量全速/64G BAR1/显存几何）vs 持续参数调节（功耗/频率/电压曲线）。功能集不相交，只有先后关系：卡先"变强"，才轮到 OC 工具调它。
3. **平台底座**：Linux initramfs/DKMS/内核 PCI 分配器 vs Windows NVAPI/ring0 kmd。Windows 侧等价物是 UEFI 阶段方案（TOTORONG），两边互不可移植。
4. **知识载体**：公共复利资产（XML 寄存器库、方法论附录、死结台账）vs 会话式逆向笔记。同量级智力投入，复利结构强一档。

## 4. 开发者技术基础比我们多出的五项

1. **二进制安全/攻击工程**（最大单项缺口）：对*签名映像*做 ROP（哈佛架构→"未签名指令从未执行"）；无界 DMA 漏洞的精确算术（DMEM 0x0800+0xF800=0x10000 恰好越过 64KiB 顶端）；canary 均匀性攻破（同一常量 0xc0deca7e 覆盖全局与全部保存槽→每个尾声比出 V==V）。
2. **GSP 引导链全景**：devinit→FWSEC-FRTS→WPR2→SEC2 Booter→GSP-RM 完整时序；Falcon v4 微架构（CPUCTL/邮箱/DMA 引擎/FBIF/BCR 切换）；PLM 位模型（[3:0] 读层/[7:4] 写层，默认 0xffffff8f=仅 L3 heavy-secure 可写）；熔丝三层与**覆盖优先级高于熔丝**；FEAT_OVR_PLM 是唯一常开岛 PLM→解释了算力解锁为何先于显存几何。
3. **PCIe 总线层教科书功力**：ReBAR 三态、FLR 语义（尺寸控制打回、能力存活）、桥窗口手工编码、BME 生命周期、ENOSPC 的对齐数学。
4. **复现工程纪律**：行为级假 MMIO（SS1 按硬件掩 4 位、WPR2 仅在模拟 FWSEC 后出现）+62 故障注入场景+独立走 radix 树逐字节校验 84MB 映像——**整个解锁事务在没有 GPU 的机器上开发出来**；输入/产物全链 SHA-256、生成不符即硬失败。
5. **认识论纪律**：权威序"出货源码>分支快照>独立复现>单人报告>推理"（项目自己的文档故意排在测量之下）；死结按复发模式归纳；LLM 输出必须附读回/对照/跨卡证据。

等价项（双方各有千秋）：差分逆向功力、ring0 工艺（我们独有 Windows 内核侧作品）、LLM 增强工作流。**差距不在智力和逆向手艺，在作战层级、平台底座与验证制度化。**

## 5. 可借鉴清单

**直接可用**：envydis+rnndb（Falcon 反汇编+寄存器语义参考，注意现代卡新寄存器不在库里，一手来源是 open-gpu-kernel-modules 源码）；nvbios BIT/boost/cstep 解析（与 nvflash/NVGI 工作对照校验）；demmt（Linux blob ioctl 追踪=独立第二源 oracle）；mock 硬件测试基建（kmd 宽表车道最值得抄的单件工程）；`tools/read_sm_rates.py`（ctypes 裸控件 0x20801203 GET_SM_RATES，与 nvapi-rs 同款手艺，可作能力实验的驱动侧独立确认通道）。

**流程可学**：死结台账与 refuted/disputed 状态制；按关闭成本排序的未解清单；"读回是唯一成功判据"成文为贡献规约；evidence-as-commits 发布模式（`.gitattributes -text` 保证证据逐字节跨平台一致）；`ada-port.md` 研究日志格式（superseded 假设留档+负面结果同等篇幅）。

**参考架构（远期，按 AGENTS.md 高危准则另行立项）**：4090 fork 的"固件侧特权写只通告能力、其余交 OS 分配器"模式；"主机物理写在 PLM 面前无效"判死 kmd 物理写在本层的适用性；Windows 能力域正解=UEFI 阶段（见[规划文档](uefi/windows-uefi-pre-driver-unlocker-dev-plan.md)）。

**明确不借鉴**：张量解锁/BAR1/显存几何本身（产品能力变更、超 scope）；Linux kmod 交付路径（平台错位）；重推 V67 漏洞链（已完成工作）。

## 6. IDA/idalib 对 Falcon 固件的可用性判决

**结论：不能，且无变通路径。** fuc0–fuc6 无官方处理器模块，社区也无维护中的模块（REAmalgam REA 6.7.0 的 Ghidra 原生架构列表同样没有）。正解三件套：**envydis**（fuc0–fuc6 全系，`reverse/envytools`，725 行参考实现+17 篇 ISA 文档）、**marysaka/ghidra_falcon**（Ghidra 处理器模块）、**vbe0201/faucon**（fuc5-only，有 asm/dis、无可用模拟器）。

本机 `1.rom` 实弹三墙实验（2026-10-25）：

| 成分 | 状态 | idalib 结果 | 墙 |
|---|---|---|---|
| EFI GOP 驱动（71KiB, code_type=3） | 压缩存放（熵 7.6–7.8，无 PE 签名/标准 LZMA 头） | raw 载入、0 函数 | 容器墙：须先解压 |
| FWSEC（GSP 安全微码 0x9a10） | IMEM 熵 **7.99**（AES 密文） | 无法分析 | 加密墙：IDA 与 envydis 一同致盲 |
| PRE_OS（PMU 0x6774） | **明文 fuc5**（熵 6.63） | 0 函数；`define_code` 报 "Failed to create instruction" | 处理器墙：IDA 退化为 hex viewer |

意外收获：IDA 字符串扫描在 PRE_OS 的 DMEM 区抓到 devinit 脚本记号（`bw.../sw.../2d2t`）——数据面仍有辅助价值。**IDA 领地**：x64（nvapi64/RTCore64/驱动，现有车道主力）+ **RV64**（GSP-RM 的 gsp ELF，IDA 原生支持；建议抽段增量分析）+ x86-16（VBIOS legacy 段）。自写 IDA 处理器模块估 2–4 周（fuc5 子集，语义参考 envydis/faucon），但只买到线性反汇编买不到 Hex-Rays，已判不立项。

## 7. 学习清单（完整版）

**① PCI/PCIe 总线层（P0，2–3 周）**。概念：配置空间与能力链（0x34 指针→PCIe cap→0x100 扩展链，ReBAR=cap 0x15）；BAR 尺寸探测算法（写全 1 读回）；64 位 BAR 对与 prefetchable 语义；桥窗口（PREF_MEMORY_BASE/LIMIT+Upper32）与 Linux `struct resource` 树；ReBAR 三态；FLR（DEVCAP bit 15）与 BME 生命周期。材料：内核 `Documentation/PCI/*`→`docs/bar1-64g-report.md`（bar1 仓）→`tools/server-161-bar1-layout/bar1_layout.c` 逐行。实践：`lspci -vv` 盲读自查；NVOC 加 ReBAR 三态读取。验收：能解释 FLR 后三态各自去向、手推桥窗口编码、说明 64G 对齐为何需 2 倍窗口。

**② NVIDIA GSP 引导链+Falcon（P0，4–6 周）**。概念：微控制器地图（PMU/GSP[RISC-V,Turing 起]/SEC2/引擎 Falcon）；引导时序 GFW 等待（0x00118128/0x00118234/0x00110100）→FWSEC-FRTS（VBIOS PROM 0x300000，BIT 0x85）→WPR2（0x1fa824/28）→SEC2 Booter（0x840000，邮箱 0x40/0x44）→GSP-RM；Falcon 界面（CPUCTL/DMA 0x110–0x128/boot base 0x104/FBIF）；PLM/熔丝/WPR2 语义；debug 密钥 #37（MD5 IV+密钥号，轮密钥逆序）。材料：`How Ampere was unlocked.md`（gpu-unlock docs，先读三遍）→cmp170hx `unlock/` 四篇→open-gpu-kernel-modules `kgspBootGspRm`（对着 gpu-unlock 注释互为字典）→envytools `docs/hw/falcon/`。实践：追一遍 `kgspBootGspRm` 全流程；envydis 反汇编 PRE_OS。验收：画出时序图标注每步执行者/特权级；解释"算力先于显存几何"与 V67 为何只需一次 HS 写。

**③ Linux kmod+早期启动（P1，3–4 周）**。概念：`module_init` 主动扫 vs `pci_driver` probe；initramfs 模块排序（gpu-unlock 用 drop-in 而非 softdep 的原因）；DKMS；devres/DMA mask/PM notifier 优先级。材料：gpu-unlock `kernel/gpu_unlock/main.c`+`packaging/boot/*`。实践：**零硬件入门=WSL2 里跑通 `make test` mock 套件**。验收：解释 devres 留尾巴如何炸 probe。

**④ 固件容器格式（P1，1–2 周）**。概念：ELF 段抽取（.fwimage/.fwsignature_ga10x/_ad10x）；BINDATA C 数组（raw-DEFLATE+gzip 尾）；VBIOS 链（0xAA55/PCIR/BIT tokens）；签名槽选择。材料：`envytools/nvbios/`、gpu-unlock `firmware/*.py`+`vbios_610.c`、我们自己的 [FWSEC 文档](vbios/tu106-fwsec-ucode-decode.md)。实践：nvflash 抓本机 VBIOS→nvbios 解 boost/cstep。验收：说出 gsp_ga10x.bin 到可点火映像的转换链。

**⑤ 二进制安全·读与审计（P2，3–4 周）**。概念：栈帧解剖（返回槽 0xFF50/canary 0xFF4C/全局 canary 0x6140-GA102、0x5F50-AD102、0x18 帧栅格）；无界 DMA；canary 均匀性；gadget 与哈佛架构含义；marker 0x67b00001 执行证明；KPA。材料：cmp170hx `rop-chain.md`+gpu-unlock-ada `tools/{trace_booter_stack,compare_booter_gadgets,inspect_v67_prefix}.py`。实践：手工解码 GA102 V67 的 19 个补丁字。验收：讲清"报错 0x31 但解锁成功"为何成立。

**⑥ UEFI 基础（P2，2–3 周）**。概念：SEC→PEI→DXE→BDS；BOOTX64.EFI；PciIo/GOP/LoadedImage 协议；GOP 断连重连；Secure Boot。材料：UEFI spec 第 7/12 章、TOTORONG INSTALL/ANALYSIS、`reverse/GOP Updater`（GOP 驱动即断连对象，与收藏一箭双雕）。实践见[规划文档](uefi/windows-uefi-pre-driver-unlocker-dev-plan.md) M0–M1。验收：说清 Linux initramfs kmod 的 Windows 等价物及各自约束。

**不学**：模拟/电气/硅工艺——两个生态的 frontier（Gen3/4 熔丝毛刺、NVLink 焊接、ECC、HBM 训练）都死在需要实验室设备处，与软件路线无交集。
