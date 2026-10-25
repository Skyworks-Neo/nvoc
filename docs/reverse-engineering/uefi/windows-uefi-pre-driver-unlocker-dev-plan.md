# Windows UEFI 预驱动解锁器——开发规划与环境评估

- 日期：2026-10-25
- 性质：规划书（本文档本身即交付物；M0 起的代码与环境安装属后续阶段）
- 已定稿决策（可改）：技术栈 **Rust + uefi-rs**；里程碑边界 **M0–M3**（3060 载荷点火=M4，单独立项）
- 信息源：[TOTORONG/AD103_win_unlocker](https://huggingface.co/TOTORONG/AD103_win_unlocker)（INSTALL/ANALYSIS 实抓）；`reverse/gpu-unlock*` 三仓（事务范本与 mock 测试模式）；本会话 `1.rom` 实测（见 [tu106-fwsec-ucode-decode.md](../vbios/tu106-fwsec-ucode-decode.md)）
- 关联：[gpu-unlock-ecosystem-survey.md](../gpu-unlock-ecosystem-survey.md)

## 1. 目标与定位

做一个 TOTORONG 式的**通用复刻**：Windows 机器上、NVIDIA 驱动加载**之前**，以 EFI 应用执行预驱动事务（枚举→只读侦察→[门控]固件事务），完成后链式交回 Windows Boot Manager。与 NVOC 现有 NVAPI/kmd 调参车道**正交分层**：一个改"卡是什么"（能力解锁/寄存器权限），一个调"卡跑多快"。

两个硬前提（三重独立证据：4090 fork 主机写被无视、TOTORONG 强制 SEC2、3060 残局"写了不动钳"同构投影）：

1. PLM 是 PRI 总线的硬件门，主机/kmd 物理写在能力层结构性无效——越过它的唯一路径是认证固件内部（SEC2 heavy-secure）。
2. Windows 没有用户可及的"驱动加载前"钩子（boot-start driver 需签名）——唯一等价时窗是 UEFI DXE 阶段。

## 2. TOTORONG 工艺拆解（参考实现的已知事实）

**交付与运行**：FAT32 U 盘根目录 `EFI\BOOT\BOOTX64.EFI`；Secure Boot 关闭+启动菜单选 U 盘，**无常驻引导项**（一次性）；版本确认屏+单键 U 触发+30s 超时取消；显示在固件驱动断连期间可能消失或冻结末帧。

**事务序列**（模型卡原文步骤）：校验设备/ROM 映射/资产 hash→预载 Windows Boot Manager→断开 GPU 固件驱动（GOP）→准备 FWSEC/WPR2→**一次** SEC2 START 执行四 PLM 链（WPR_CFG 0x1fa7cc→FBPA 0x9a0148→WPR 0x1fa7c4→FEAT 0x823804）→查 marker/SEC2 HALT/DEBUGINFO/必需 PLM 读回（FBPA 卡 0xffffff0f 记**非致命**）→分开写并读回 SS0 0x82381c/SS1 0x823820→清 BME→FLR→验证 PCI/GFW/WPR2 恢复→释放 DMA→重连 GOP→观察选择器→启动 Windows。日志阶段名：SELECTOR RESULT / POST-FLR / POST-GOP / FINAL。

**工具链（实抓，推翻 edk2 假设）**：Python 3 + **Zig 0.14.1**（`zig cc` 出 freestanding PE/EFI），`python3 source/build.py --zig <path>`；模拟测试另需 GCC；资产管线 `tools/prepare_assets.py` 钉 SHA-256 下载 NVIDIA 610.43.03 输入，要求 **Python 3.14+**；EFI 成品钉 SHA-256（`9f821b8e...`）、ROM 钉 hash（`ebb63e82...`）单卡。

**安全纪律（原文）**："STOP 出现后同会话勿重试，固件可能仍活着——完全断电后走正常 Windows 引导恢复"；替换 U 盘 BOOTX64 前先保留原有；"勿覆写 Windows ESP 的 bootmgfw.efi"；"FINAL status=0 单独不构成提速证据"（基准对照+断电间隔）；"勿隐藏不匹配或移除门禁；钉住的下载失败时不得换用其他固件版本"。

## 3. 硬件现实评估

| 机器 | 判定 | 依据 |
|---|---|---|
| **TU106（2070 桌面）** | **实验机**：事务复刻可行，无已知解锁目标 | GSP+SEC2 在位；VBIOS 带 FWSEC V2 双轨（本会话实测，IMEM 0x9600/DMEM 0x410）。20 系无张量半速 Segmentation；**FEAT 簇（0x823804–0x823820）在 Turing 的存在性本身是开放 RE 问题——M1 的采样直接回答**，无论答案如何都是一手产出 |
| GA106（3060） | M4 单独立项 | gpu-unlock 支持表内（BF16 +80% 实测），社区 GA10x V67 载荷现成；但这是残局工作机，风险承诺需单独决策 |
| P100（GP100） | 不适用 | 无 GSP（pre-Turing），本技术无支点 |

TU106 专属坑（ada 教训）：booter/gadget 几何**逐芯片不同**（GA102 载荷在 AD102 上 HALT 且不开权限），M2 必须从钉 hash 的 Turing 驱动源自抽资产；GA10x 载荷不可直接用。TU106 GSP 为 RISC-V（NVIDIA 官方口径，Turing 起换代），FWSEC 走 V2 BOOT_WITH_LOADER。

## 4. 环境评估：Windows 上开发 EFI 是否合适

**结论：合适，且本机已具备大半。** 三方案对比：

| 方案 | 构建 | 优势 | 劣势 | 本机缺口 |
|---|---|---|---|---|
| **Rust + uefi-rs（选定）** | cargo + rust-lld，`x86_64-unknown-uefi` 目标，Windows 原生 | 与 NVOC 同栈；协议 API 已验证全覆盖（`boot::locate_handle_buffer(ByProtocol(&PciIo::GUID))`+`open_protocol_exclusive::<PciIo>`+`boot::disconnect_controller/connect_controller`+`LoadImage/StartImage`）；host 侧单测可直接复用 gpu-unlock 的 mock 事务模式；cargo 钉版本=复现纪律 | 与 TOTORONG 参考实现异语言（载荷是数据不受影响；loader 自研本就要重写） | `rustup target add x86_64-unknown-uefi`；QEMU |
| Python + Zig 0.14.1 | `build.py --zig`，Zig 单文件工具链 | TOTORONG 直系保真（build 流程一致）；Zig 一个 zip 即用 | C 生态、与 NVOC 栈分离、无 Rust 测试基建 | Zig 下载 |
| edk2 + VS2022 | base tools+NASM+ASL+SDK | 厂商样例生态最大 | 环境最重、迭代最慢 | NASM/ASL |

**本机体检（2026-10-25）**：在位=VS2022 Community（MSVC 14.44）、Windows SDK 10.0.26100、**Python 3.14.5**（恰合 TOTORONG 资产管线的 3.14+ 要求）、Rust 1.95（stable-msvc）、WSL2（archwsl，备 GCC 环境）。缺=**QEMU for Windows**（VM 冒烟必需，~300MB，自带 OVMF 固件）、Zig（仅备选方案要）、NASM（仅 edk2 要）。

**测试闭环**：QEMU+OVMF 冒烟（无 NVIDIA 设备→优雅退出路径=天然的"负控"测试）→实机 U 盘。磁盘余量 C: 134G/D: 115G 充裕。

## 5. 里程碑

### M0 骨架（0.5–1 天）

新**独立 crate**（不入 nvoc workspace：uefi 目标与 std 依赖隔离；资产校验纯函数后续按需以 no_std 形式复用）。内容：QEMU/OVMF 冒烟（`cargo run` 产物进 U 盘镜像或 `-drive`）；实机 2070 U 盘路径：枚举 vendor 10de→打印 PCIe caps/ReBAR→`LoadImage/StartImage` 链回 `bootmgfw.efi`。
**验收**：实机 U 盘启动到 app、显示枚举结果、无缝进 Windows；**全程零 GPU 写**（grep 审查无 PciIo 写调用）。
**风险/回滚**：链式启动失败→黑屏→强制重启走原引导项即恢复（未动任何固件配置）。

### M1 只读冷态采样（1–2 天）

PciIo 映射 BAR0（16MiB）→读：`PMC_BOOT_0`（芯片 ID）、MMU PLM 0x1fa7c8、WPR2 0x1fa824/28、FEAT 31 字簇 0x823804–0x823820（存在性=首个开放题）、SS0/SS1、熔丝簇 0x8203f0/0x8241c0/0x824140、FEAT_READOUT 0x823818；badf 毒值分类（0xBADF5040=priv violation / 0xBADF1100=undecoded / 0xBADF20xx=floorswept——勿混桶）。结果屏显+写 U 盘 FAT 文件（SimpleFileSystem 协议）。
**验收**：采样文件落盘；与 Linux 侧（若有）/已知 GA10x 冷值表对照分类；TU106 FEAT 簇存在性有明确答案。
**风险**：纯读，无新增风险。

### M2 资产管线 + mock 事务（2–4 天，无 GPU）

- 钉 hash 下载 Turing 驱动源/nvidia-utils 包（沿 gpu-unlock 的 artifacts 清单惯例，本地再生不分发）；
- Rust 重写 `extract_boot_assets`（ELF 段+BINDATA 抽取）+复用既有 FWSEC V2 描述符解码（`core/src/vbios/legacy.rs`，提交 8353da3）；
- **host 侧 mock 事务测试**：移植 gpu-unlock `tests/test_unlock_adapter.c` 的行为级假 MMIO 模式（WPR2 仅在模拟 FWSEC 后出现、SS 掩码、FLR 时序、DMA 簿记）到 Rust 单测——此件同时反哺 kmd 车道。
**验收**：资产逐级 SHA-256 校验通过；mock 下全事务状态机绿（含故障注入路径：marker 不符/HALT 不置位/WPR2 预启/尺寸不符）。

### M3 2070 事务复刻（1–2 周，进入前门控确认）

FWSEC **BOOT_WITH_LOADER** 点火（GSP）→WPR2 建立→**Booter 原签加载（无载荷）**→GFW/邮箱验证→FLR→交棒原厂驱动。这是"完整机器不带 exploit"的复刻：证明我们能在 Windows 上徒手走完社区在 Linux 上走完的链。
**验收**：内核级等价证据（WPR2 HI.VAL 建立又随交棒清零、GSP 状态、dmesg/事件日志无驱动抱怨）+读回门禁全绿。
**风险**：中途失败=黑屏至断电（gpu-unlock 同款风险面）；BME/脏状态清退先于事务。**回滚**：正常 Windows 引导项（未装任何常驻引导项）。

### M4（不在本计划）

3060 GA106 社区 V67 四写链点火。前置：M0–M3 全绿+显式立项决策+变砖承受评估（按 AGENTS.md 高危写入准则）。

## 6. 安全纪律（成文，随代码库演进）

1. **读回是唯一成功判据**——状态码/返回值/中断一律不作数（TOTORONG："FINAL status=0 单独不构成证据"）。
2. STOP/异常后同会话**不重试**；断电后走正常 Windows 引导恢复。
3. 只读链式加载 ESP 的 `bootmgfw.efi`，**永不覆写**；U 盘替换 BOOTX64 前保留原有。
4. 实验机 Secure Boot 关闭，或自签证书入 db（优先后者，留 audit 痕迹）。
5. 全部外部资产钉 SHA-256、本地再生、不入 git；密钥/载荷解密产物永不入库（沿 ada 仓纪律）。
6. GPL 参考实现只借鉴行为，Rust 独立实现（洁净性）。
7. 每阶段产物（采样文件/mock 日志/事务读回记录）按 evidence-as-commits 归档（`.gitattributes` 标 `-text`）。

## 7. 开放问题（进入对应里程碑前必须收敛）

- TU106 FEAT 簇存在性与冷值（M1 自答）；
- TU106 GSP/SEC2 的 Falcon-vs-RISCV 双核选择与 BCR 切换是否与 GA10x 同构（M2 资产抽取时核对 FALCON2 0x668 存在性）；
- uefi-rs 对 DriverBinding Stop() 路径（GOP 断连）的错误码保真度——若包装层吞细节，降级到 raw system table 调用（M0 验证）；
- 2070 机器 Secure Boot 现状（M0 实机前查）。
