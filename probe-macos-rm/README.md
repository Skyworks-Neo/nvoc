# probe-macos-rm — NullMoth NVRM kext RM 控制面验证套件

默认构建为 **idle-only**，用于空闲查询准备：拒绝 `--write`、阶段 3/5，跳过内存冒烟、I2C 和 legacy 控制探测，不主动提交 GPU 工作负载。查询仍可能唤醒设备。需要内存、通道或性能实验时，单独构建 full profile；写入仍需 `--write`。

针对 [nullmoth/nvidia-macos-driver](https://github.com/nullmoth/nvidia-macos-driver)（本地 `reverse/nvidia-macos-driver-main`）的用户态 RM 控制面**分阶段验证工具**。配套分析：`reverse/nvidia-macos-driver-oc-feasibility.md`（可行性报告）；逐项矩阵与判读见 [VALIDATION-MATRIX.md](./VALIDATION-MATRIX.md)。

**目标**：在一台装好 NullMoth 驱动的黑苹果（macOS 15, x86_64, NVIDIA Turing+）上，实证回答可行性报告的 M0–M1 问题，并为 M2/M3 收集基线：

1. 用户态 → `NVRMUserClient` selector 1 → `rm_ioctl` 直通是否可达（报告 §3.3）；
2. RM 对象链（root client → attach → device → subdevice）是否完整可用；
3. 内存映射路径（sysmem/VRAM 分配 → MAP_MEMORY → BAR1 读写）与**通道执行路径**（VASPACE → CONTEXT_DMA → GPFIFO → 门铃 → 信号量）是否完整；
4. GSP-RM 对**遥测类** FINN 控制与 **RUSD 遥测页**的接受度（M0）；
5. GSP-RM 对**写入类**控制（PERF_BOOST / SET_POWERSTATE / UPDATE_EDPP_LIMIT）的接受度与可逆性（M1）；每次写入均为"回读值回写"的零位移往返，退出时恢复。

## 前置条件

GTX 1650 SUPER / macOS 15.8.1 的当前结果见 [RM 访问实测](../docs/reverse-engineering/macos-gtx1650s-access-2026-10-16.md)：连接和对象链已通过，RUSD 分配成功但映射未通过。缺失遥测的 `0` / `-1` 输出是占位，不能作为实际读数。

- macOS 15 (x86_64) + NullMoth 驱动四件套已加载（`kmutil showloaded --list-only | grep nullmoth` 有 4 行）；桌面出画面说明适配器已过 pass 2。
- Xcode 命令行工具（clang）。
- NVIDIA open-gpu-kernel-modules **610.57.04** 源码树（仅头文件用途，与驱动构建同源）：

```bash
git clone --depth 1 --branch 610.57.04 \
    https://github.com/NVIDIA/open-gpu-kernel-modules ~/ogkm610
```

## 构建

```bash
sh build.sh                 # 产物 ./probe-macos-rm-idle（默认 OGKM 路径 ~/ogkm610）
OGKM=/path/to/ogkm sh build.sh
```

非 macOS 机器上可以做纯语法/类型检查（IOKit 层走桩，其余用真实 OGKM 头）：

```bash
OGKM=/path/to/ogkm LINT=1 sh build.sh
```

CPU 模拟回归检查（不连接显卡），覆盖禁用操作在连接前被拒绝、独立阶段依赖、重复阶段、fd 0 和显式 gpuId 0：

```bash
OGKM=/path/to/ogkm python3 tests/test_idle_cli.py
```

## 运行

默认 idle-only 构建只运行连接和空闲遥测；驱动初始化正常后可使用：

```bash
./probe-macos-rm-idle --stages 0 --jsonl connection.jsonl
./probe-macos-rm-idle --stages 012 --sample 10 --jsonl idle.jsonl
```

以下为完整套件命令，先使用 `PROFILE=full sh build.sh` 构建。应先完成连接和遥测验证，再逐项运行内存、通道和性能实验；阶段 3 的 EDPp 恢复语义仍需核实，暂不执行该整组写入实验。

```bash
./probe-macos-rm --jsonl run1.jsonl                    # 阶段 0+1+2，含私有内存写读回，不调频
./probe-macos-rm --stages 0125 --jsonl run1.jsonl      # + 通道冒烟（GPU 执行证明，非调频写入）
./probe-macos-rm --stages 4 --sample 30 --jsonl load.jsonl  # 30 s 遥测采样
```

- 退出码：`0` 阶段调度完成，具体支持情况仍需查看每项 verdict；`1` 传输/对象链或阶段失败；`3` full profile 中有 --write 门控项未授权；`2` 用法或 profile 限制错误。
- 每个条目输出 `[OK|REJECTED|SKIP|GATED|HARD]` 判定 + `NV_STATUS` 名；`--jsonl` 同步落盘（结果 + 采样流），供与 Linux/Windows 实机三方对照。

### 安全须知（务必读）

- 完整构建的阶段 1 包含私有内存写读回，阶段 3 包含性能写入；EDPp 的原始策略恢复尚未核实，不能将回写相同功耗值当作完整恢复。开始性能测试前需单独解决该问题。
- 若用 `-nvrmnogo` 手动分阶段启动，需先 `nvrmctl go 2`，否则阶段 0 会在 card-info 处 HARD 失败并给出提示。
- 当前驱动已恢复真实 UID 管理员检查；客户端可打开不代表任意控制均获授权，权限以实机返回为准。本工具用于实验环境。

## 代码结构

```
src/nvx_xnu.c    IOKit 传输：IOServiceOpen(NVAccel 0x4E560000|n → NVRM n) + BINDFD + esc/MAP_MEMORY
                 （移植自 nvk-macos.patch 的 nvrm_xnu.c，即报告 §3.5 的参考实现）
src/nvx_rm.c     rm_alloc/rm_control/rm_free + NV01_ROOT_CLIENT→DEVICE_0→SUBDEVICE_0 链
                 （kms.inc:93-100 同款 dance）
src/nvx_suite.c  阶段 0–4 实现；全部 Tier-A 常量直接 #include OGKM 头，零转写
src/nvx_defs.h   仅收录开源树不含的 Tier-B legacy ID（逐条标注出处）
src/main.c       CLI / 阶段调度 / JSONL
```

常量出处纪律：**能进 OGKM 树的不进 nvx_defs.h**（编译期即权威）；树外 ID 必须带 campaign 文档出处注释；SET 面的 legacy 命令（`0x20801ACF`、`0x2080D031` 等）v1 只在矩阵文档记录、代码不发送。
