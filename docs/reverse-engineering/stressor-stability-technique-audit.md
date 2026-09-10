# 压力器稳定性检测技术差距评估 —— 对照逆向归档中的 OC Scanner / HYDRA 等外部技术

日期：2026-09-10 · 分支：cli-arg-organized-more-reversing · 纯评估报告，未改动任何代码

信息源：
- 归档 `reverse/nvoc-gpu-archive-2026-08-28/`（81 篇记忆 + 408 会话 + RE 文档）
- NVIDIA 官方 OC Scanner 逆向：`01-memory/nvidia-oc-scanner-reversed.md`、会话 `98c31eb8-*.jsonl`（行 13197-13619，scanner.dll + gpu_stressor 全逆）
- HYDRA 2.2B PRO 逆向：`01-memory/hydra-22b-pro-reversed.md`、会话 `8b18f9fb-*.jsonl`（GpuStress readback 解码）
- 170HX HBM 取证：会话 `170HX/8f113028-*.jsonl`
- 当前实现：`cli-stressor-cuda-rs/` 全源码通读 + `auto-optimizer/` 扫描判据链

---

## 一、当前判据架构（基线）

nvoc 的自动超频判定回路完全在用户态，stressor 本身只产生负载，判据分两层：

**stressor 层**（cli-stressor-cuda-rs）：
- 失败途径穷举：CUDA/cuBLAS API 错误、GEMM 校验失败、Vulkan sidecar error flag。退出码 0/1/2。
- 唯一数值校验 = `validate_precision`（lib.rs:791-847）：独立生成的 1024² 小矩阵 CPU FP64 参考 vs GPU，逐元素 atol/rtol 比对，默认每 5s 一次。**与正在压测的大矩阵/burst 缓冲完全无关**。
- **从不校验的路径**：memcpy dst、memset 缓冲、atomic 输出、intalu 输出（"intentionally non-referenceable"，lib.rs:294-305）、INT8 GEMM、Vulkan image。asum 是唯一每迭代隐式 D2H 的路径但回传值不比对。
- 无温度/功耗/频率采样、无 ECC、无 XID/TDR、无 sticky error 轮询、无内部看门狗。

**optimizer 层**（auto-optimizer）：
- stressor 退出码 + 超时预算（timeout_loops×15s）+ Windows 事件日志（FECS>3 / TDR>6 / 任何新增事件即 fail，pressure.rs:627-800）+ Linux journalctl Xid。
- 收敛 = StepController 指数步进二分（"Relaxed Finite-Step Binary Approximation"），测试期间持续 V/F 频率抖动暴露边缘点。
- 限流（thrm/pwr capping）占比 >30% 仅告警不判失败；无温度稳态等待（grep settle/steady 零命中）。

**一句话：判据本质是"崩溃/TDR 边界"（二元、事后、粗粒度）。临界频率附近，数据错误先于崩溃出现——这一整段"亚崩溃区间"当前完全不可见。**

---

## 二、外部技术盘点（归档实证）

### 2.1 NVIDIA 官方 OC Scanner（scanner.dll + gpu_stressor，GPUTweakIII/MSI 同源）

- 负载：单一 CUDA GEMM（sgemm/hgemm/tensor-core 变体 × _L1.._L6 展开档），三参数调制：K(8..216)=FMA 归约深度、internalLoops=展开档、injectZeros=占空比（空转内核替换的迭代比例）→ **功耗闭环把功耗拉进 [100%,110%] power budget**。
- 校验：**两遍 GPU 直方图**（`gpu_hist_gpu_first<T>` + `__shfl_down_sync` warp 归约）——先往 VRAM 写已知分布数据，压测后对同一块显存重新统计，任何位翻转都会使直方图偏离期望分布 → `CorruptMemoryError`。**把内存校验从 CRC/ECC 搬到统计域，直方图内核兼任负载与检测器**。另有 CURAND 卡上填阵 + `gpu_check_gpu<T>` 逐元素校验（half/float/double/int/char 五模板）。
- 判定：监控线程轮询限流位（power/temp/voltage/noLoad/sliSync），任一激活 >25% 采样窗口即本步 FAIL；Welford 方差的四采样器（电压/频率/功耗%/温度）。
- 扫描：quantum 量化校准（offset 清零 → +1MHz 步进直到实际 MHz 跳变，通常 15MHz）；锁电压点回退 30mV 只扫 4 点；最终值 = 最后通过 − 5×quantum（≈75MHz 硬裕量）。
- 错误报告：`StressErrorType` 枚举（None/DataError/CorruptMemoryError/RuntimeExceptionError/TdrError/ApplicationCrashed/SystemReboot/Interpolated…）+ **置信度百分比**（非二元 pass/fail）。

### 2.2 HYDRA 2.2B PRO（HydraGpuStress.exe，D3D12 显存 memtest）

- **错误定位结构：每窗口 400B readback**（compute shader GPU 侧归约，CPU 从不扫描整块显存）：`+260 total_errors / +264 idx_min / +268 idx_max（出错元素索引区间）/ +272 exp / +276 act / +88 bit_pos[22]（22 位错误位置直方图）/ +0 magic 0xADBA0000ADBA`。定位粒度 = UAV 字节索引；22 位直方图 → 推断 GDDR6 DQ 数据线/地址位失效分布。
- **cross-contamination 检测**：写窗口 A 时窗口 B 出错 = 行列耦合/刷新问题。
- **检测管线自检门**：先注入已知单错位（(exp^act)==0x400000），验证 readback 比对 + 直方图 + cross-contamination 全链路工作，失败报 "error detection pipeline is broken"——防检测器自身假阴性。
- **Dwin 自适应**：persistent victim 模式下动态收缩 disturbance window，保持错误隔离粒度。
- **~30 个 GDDR6 pattern 目录**（march_c 系 / modulo20 / own_address / bit_fade / RetentionHeat / research_gddr6_halfgrad_ramp 镜像对打 bus turnaround / victim 邻扰 / vk↔cbg 交叉写读打 bank conflict…），每个 pattern 映射一类失效模式。
- **稳态/保持参数**：`--retention-ms`、`RetentionHeat`、`--epoch-seconds`、`--read-granularity`、`--self-test-seconds`——写入后放置再读回的保持测试（显存稳态方向）。
- 输出：NDJSON verdict（total_errors/result/self_test_failed…）+ `GetDeviceRemovedReason` 退出分类。
- 前端 NVML 监控清单：`nvmlDeviceGetMemoryErrorCounter`（DRAM/L1/L2/SRAM/REG/TEXTURE × corrected/uncorrected）、`GetPcieReplayCounter`、`GetViolationStatus`、`GetCurrentClocksThrottleReasons`。

### 2.3 其他

- **170HX HBM 取证会话**：`nvidia-smi -q -d ECC/PAGE_RETIREMENT/ROW_REMAPPER`（InfoROM 掉电保留）为显存缺陷最硬证据；无 ECC 时用 cuda_memtest/memtest_vulkan 全量多 pattern。**诊断逻辑：某地址区间稳定报错 = 确定性坏区；全随机或仅高压负载出错 = PHY/时序/电压裕量问题**——"错误确定性 vs 随机性"判定法。
- **AmpereOC**：温度衰减公式 `atten = round((T−35)/6)×15 MHz`（2.5MHz/°C 阶梯）+ Reference Temp 显示 + 锁电压点抗温漂。与 nvoc 已有温漂模型（ΔV=(α·V+β)·ΔT）与 set-vfp-voltage-lock 重叠，公式本身需实测才可采信。
- **EVGA PX1 / MSI AB / OCCT / Green Curve**：无新压测判据技术（扫描外包驱动侧 OC Scanner 或 legacy 路径；OCCT GpuMemtest 是 OpenCL 无独特算法）。

---

## 三、值得引入的技术评估（按优先级）

评估口径：是否让"临界频率/稳定性问题"被**更早（崩溃前）、更细（量化而非二元）、更准（区分失效机理）**地捕捉。

### P0 —— 直接补上"亚崩溃检测"空白，成本低收益大

| # | 技术 | 来源 | 落地位置 | 理由 |
|---|------|------|---------|------|
| 1 | **压测数据的 GPU 侧统计校验（直方图/归约 readback）** | Scanner 两遍直方图 + HYDRA 400B 归约 | memcpy/memset 路径写已知 pattern → GPU 归约成 400B 错误结构（total_errors/idx_min/max/exp/act/bit 直方图）→ D2H 只读 400B | 当前显存扫描（mem_oc 1MHz 步进）的判据同样只有崩溃；数据错误先于崩溃出现，这是收紧临界显存频率最直接的信号。GPU 侧归约意味着校验本身也是负载，不降低烈度 |
| 2 | **把校验从 sidecar 挪到真实压测对象** | Scanner（直方图内核兼任负载与检测器） | GEMM burst 的 C 缓冲 burst 结束后做统计域校验，而非只测独立 1024² 小矩阵 | 现在的校验矩阵与压测矩阵毫无数据关系——崩溃前真实缓冲里的错误完全不可见 |
| 3 | **检测器自检门** | HYDRA（注入 magic+已知错位） | 启动时对检测 kernel 注入单错位，验证 readback/直方图全链路 | ~20 行成本，消除"检测器坏了所以全绿"的假阴性——引入任何 readback 校验的前置条件 |
| 4 | **机器可读 verdict 输出（JSON/NDJSON：错误计数+分类+置信度）** | Scanner 置信度% + HYDRA NDJSON | stressor 汇总从纯文本升级为结构化；optimizer 解析之 | StepController 现在只吃退出码。没有量化通道，后续一切"错误计数驱动的收敛判据"都无法接入 |
| 5 | **INT 路径校验** | （自有技术债） | intalu 的 LCG 链（b·1664525+1013904223, int_alu.rs:46）是确定性的，host 可算参考值；INT8 GEMM 同理可 CPU 参考 | INT 路径现在 0 校验，注释"intentionally non-referenceable"不成立——链式 PRNG 恰恰是最容易校验的 |

### P1 —— 提升判定质量与诊断分辨率

| # | 技术 | 来源 | 理由 |
|---|------|------|------|
| 6 | **StressErrorType 式错误分类枚举**（DataError/CorruptMemory/Tdr/Runtime/Crash…） | Scanner | 事件日志阈值（FECS>3/TDR>6）+退出码混在一个二元结果里；分类后 optimizer 可以对不同失败类型走不同恢复/收敛策略 |
| 7 | **NVML ECC 计数 + PCIe replay + violation status 监控**（每测试窗采样） | HYDRA 前端清单 | corrected ECC 计数是显存亚崩溃的硬件级量化信号（不必依赖软件比对）；PCIe replay 捕捉总线级临界。放 optimizer 父进程侧，与现有事件日志轮询同层 |
| 8 | **错误确定性 vs 随机性判定**（跨迭代聚合错误地址/位分布） | 170HX 会话 | 同一 idx/位稳定复现 = 结构性坏区；随机 = 时序/电压裕量——直接指导"降频解决 vs 停止扫描"的分叉决策 |
| 9 | **功耗闭环负载调制**（injectZeros 占空比 + 强度档，把功耗压进 [100,110]% budget） | Scanner K 表 | 固定负载测出的稳定点≠满载最坏 VRM 压降场景；闭环选档让每个频率点都在真实最坏功耗下验证。K 表（17 档 {K,loops,injectZeros}）是现成强度标尺 |
| 10 | **频率 quantum 量化校准**（+1MHz 步进探测实际跳变） | Scanner | 20 行例程；验证 7.5MHz min_step 与驱动实际 quantum（通常 15MHz）的关系，避免在不可表示的频率点上收敛出虚假精度 |

### P2 —— 显存专项 / 大工程

| # | 技术 | 来源 | 理由 |
|---|------|------|------|
| 11 | **显存稳态保持测试**（写入 → retention-ms 放置/叠加热负载 → 读回） | HYDRA `--retention-ms`/RetentionHeat | 即"显存稳态"方向：刷窗/保持类失效在纯吞吐压测中不显形，对显存 OC 临界点价值最高 |
| 12 | **GDDR6 pattern 目录**（march_c/modulo/own_address/halfgrad 镜像对/victim/cross-contamination） | HYDRA ~30 pattern | 把显存扫描从"memcpy 大块"升级为失效模式定向；pattern→失效模式映射表是归档已提炼的纯知识资产。可先移植 3-5 个经典 pattern（march_c、own_address、modulo20、cross-contamination） |
| 13 | **温度稳态门 + 温漂补偿** | 自有温漂模型 ΔV=(α·V+β)·ΔT | 扫描点之间无热稳态等待，相邻点热历史不一致会污染判据；已有温漂模型可直接换算"本点实测温度下的等效裕量"。AmpereOC 的 35/6/15 公式不必单独引入（需实测且与自有模型重叠） |

### 明确不学（归档已有定论，维持）

- **限流位 >25% 采样即 FAIL**：撞功耗/温度墙 ≠ 不稳定，只意味着该点不再有效——nvoc 与官方扫描器的方法论分歧点。
- **−75MHz 硬编码安全裕量**：官方定位出厂安全曲线（法务裕量）；nvoc 定位真极限，裕量由用户/温漂模型决定。
- **低强度流式负载**：实测证明测出的点不代表满载恶劣场景（归档记录了压力小/PCIe 流量大/测出幅度低三观测与设计的因果链）。

---

## 四、顺带发现的 stressor 实现缺陷（与本评估无关但值得记档）

1. **profile `[kernel_params.X].weight` 是无效字段**：`FileKernelParam`（main.rs:303-314）只认 precisions/precision_weight/precision_mixture/matrix_sizes/warmup_iters/burst_iters/transpose_prob/minor_mixture_rate，serde 静默忽略 weight——各 profile 里的 `weight = 0.40` 全部不生效，实际混合比只由顶层 `kernel_mixture` 决定。
2. **FP8 是名义精度**：可解析、有 SM 门控，但 backend upload/gemm 直接报错（backend.rs:245-247, gemm.rs:473-475）。
3. `run_stress_for_precision`（lib.rs:1007）pub API 全仓库无调用者。
4. `empty_cache()` 是 no-op（backend.rs:480-482）。
5. dyn-export 内嵌 profile 跑 FP32，而 external 模式的 50 系 toml（auto-optimizer/test/cli-stressor-cuda-rs-dyn-export-50series.toml）实际跑 FP16——两者负载强度不一致，作对照时需注意。
6. 早期优化的 tiled_random_bytes（4MiB tile memcpy 路径）已随被删的 PyTorch stressor（#200）消失；Rust 侧对应物仅剩 rayon 逐 chunk 种子派生 + burst C 缓冲复用（#214）。

---

## 五、落地路线建议（供后续讨论，未实施）

若按"最小改动撬动判据升级"排序：

1. **第一步（P0#3+#4）**：给 stressor 加 JSON verdict 输出 + 检测自检门骨架——先打通量化通道，optimizer 侧暂仍用退出码（向后兼容）。
2. **第二步（P0#1）**：memcpy/memset 路径加 GPU 归约 readback 校验（pattern 写入 + 400B 错误结构），显存 OC 扫描判据从"崩溃"升级为"数据错误"。
3. **第三步（P0#2+#5）**：GEMM C 缓冲统计校验 + INT 路径确定性参考。
4. **之后**：optimizer 接错误计数/分类做收敛判据（P1#6/8），ECC/replay 监控并入现有事件轮询循环（P1#7），功耗闭环与 quantum 校准（P1#9/10）。
5. **显存专项**（P2#11/12）可独立成支线：一个 `--memtest` 模式（pattern 目录 + retention + cross-contamination + 自检门），复用 HYDRA 的 NDJSON verdict 形状。

关键出处索引：
- `reverse/nvoc-gpu-archive-2026-08-28/01-memory/nvidia-oc-scanner-reversed.md`、`hydra-22b-pro-reversed.md`
- 会话原文：`02-conversations/nvoc/98c31eb8-*.jsonl` 行 13197-13619（Scanner）；`8b18f9fb-*.jsonl` 行 1321-1359（HYDRA readback）；`170HX/8f113028-*.jsonl` 行 249/266（ECC 取证）
- 当前实现：`cli-stressor-cuda-rs/src/lib.rs:791-847`（校验 sidecar）、`lib.rs:294-305`（INT 不校验声明）、`int_alu.rs:46`（LCG）、`main.rs:303-314`（profile 字段）；`auto-optimizer/src/oc_scanner/pressure.rs:627-800`（事件日志判据）、`scan_strategy.rs:147-341`（StepController）


---

## 六、P0 实施定稿与 IDA 一手核实（2026-09-11 追记）

### 6.1 IDA 一手核实结论（非记忆转述）

对一手二进制重新做了反汇编/反编译核实（`MSIAfterburnerSetup467Beta2/Bundle/OCScanner/` 与 `HYDRA 2.2B PRO (2202)/`）：

**gpu_stressor.exe**（3.4MB，2019 CUDA 构建，纯 driver API + 内嵌 cubin `maxwell/volta/turing_sgemm_*_L1..L6`）：
- RunLoadTest 工作循环（每精度一函数，如 float 版 `sub_1400E8D10`）：`loop{ 查停止标志 → 一次 GEMM 迭代 }`，退出后立即对**同一负载对象的成员矩阵**（A@`this+0x416`、B@`this+0x41E`、C@`this+0x442`）校验（门控字节 `this+0x119`），golden 是由 A/B 算出的指纹、写入 C 结构头 `+0x7C`。**全程无独立静态 golden 显存区**。
- 直方图管线（assert 字符串即源码证据）：`cudaMalloc(&hist, sizeof(hist_cpu)*sms)` → `cudaMemset(hist,0,…)` → `gpu_hist_gpu_first/second` 在 `m_workload_stream` 归约 → `cudaMemcpyAsync(&hist_cpu, hist, sizeof(hist_cpu), D2H, m_workload_stream)`。
- `StressErrorType` 精确枚举 0-9（`sub_1400B9820`）：None/DataError/CorruptMemoryError/RuntimeExceptionError/RuntimeError/RobustChannelError/TdrError/ApplicationCrashed/SystemReboot/Interpolated；IPC 28B 消息，`RunLoadTestReturnValue` 回执携带 errorType。
- `inject zeros into A and B matrix`——injectZeros 为 A/B 置零占空比。

**HydraGpuStress.exe**：
- 注入自检门（`sub_1400097B0` 反编译级）：注入已知单错——目标元素 **0xADBA**、翻转 **bit22**（`(exp^act)==0x400000`）；通过门 = `total_errors==1 && idx_min==idx_max==0xADBA && 单一 bit 位命中 && bit_count==1 && done==1 && 其余全部窗口 total_errors==0`，否则 `Self-test injection FAILED` + `cross-contamination: window N has M errors`。**修正本报告早期记忆转述：0xADBA 是注入元素索引（idx_min/max 的期望值），并非"+0 magic"。**
- per-window 错误块 stride = 100 dword = 400B（`v91[100*window+65]` = 该窗 total_errors）。
- **窗口主动轮转证实**（pattern 纪元函数 `sub_14001AFC0`）：`for i in 0..window_count { 绑 UAV 切片 base+i*stride → 绑该窗错误块 gpu_va+400*i → 派发 pattern kernel(window=i) }` → execute → `Map errReadback` → 解析+带宽统计 → Unmap。每纪元所有窗口被主动写+校验；victim 窗受 Dwin 自适应管理，retention（`MemtestRetentionHeat` + `--retention-ms`/`--core-heat-iters`）是独立模式。

**MSIOCScanner_x64.exe**：MFC GUI 前端（MACMSharedMemory 连 Afterburner；`Test completed, confidence level is %d%%`、`Scan succeeded, average overclock is %dMHz`、Dominant limiters）——纯编排/显示层，无测试逻辑。

### 6.2 P0 已按两域拆分实施（分支 `stressor-enhanced-refactor`）

- **P0-A 核心域 SDC**：GEMM 输出全量扫描（non-finite/零值/|C| 和，块归约）+ 采样点双算互检（从同一 A/B 现场重算 dot-product，double→float 累加，容差 = `choose_tolerance`×√(K/1024)）+ IntAlu gather 采样 vs 逐位镜像 host 参考（LCG 链 + DP4A 按 SM≥6.1 门控）。
- **P0-B 显存域 SDC**：memcpy/memset 缓冲改为确定性 device 端 pattern（splitmix 终结子，索引现算期望、零 golden 存储），memcpy 每 op 校验 dst；memset 每 8 次迭代 1 次替换为 fill+verify；错误归约进每 lane 紧凑 ErrorReport（total_errors/idx_min/max/首错 exp/act/32-lane 位直方图/done 完整性计数），仅数百字节 D2H。
- **P0#3 自检门**：HYDRA 式注入（idx=0xADBA、bit22）双 gate——clean pattern 期望 0 错 + 注入期望恰好 1 错且位置/位/直方图全中；失败即 abort（exit 1，`--skip-self-test` 可跳）。
- **P0#4 verdict JSON**：`VERDICT_JSON:` 单行（optimizer 输出转发已剥 ANSI 可 grep）+ `--json-out` 落盘；字段含 self_test/detectors/coverage/confidence_pct/classification（snake_case）；退出码语义不变（0/1/2）。
- **P0#5 INT 域**：INT8 GEMM 精确整数 sidecar（CPU i32 参考、逐元素严格相等、`[verify].int8_validate_size`=512）；int16/32 由 IntAlu host 参考覆盖。

### 6.3 实测开销与默认值（GTX 1070 实机）

- 生产语义注记：cudarc 原样透传 cuBLAS 列主序调用 → row-major 视角设备 C = **B·A**（旧 sidecar 的 CPU 参考因 dgemm 参数同样按列主序解读而错得一致，故历史全绿——采样互检/INT8 参考均已按该语义对齐）。
- 实测开销：GEMM 域 ~7% wall（WDDM 提交惩罚主导，非算力）、显存域 ~12%（验证读本身即主动测试载荷）；总 η：每 burst 数百点精确互检 + 全量统计筛查 + 显存域全量 pattern 校验，对 sidecar（每 3s 一个 1024² 独立矩阵）为数量级级提升。
- 默认：`gemm_every=4, gemm_samples=512, memcpy_every=1, memset_every=8, intalu_samples=1024, int8_validate_size=512`；`[verify]` 段可调；`--no-verify`/`--skip-self-test`/`--json-out` CLI 覆盖；dynamic-export profile 显式关闭以保持 VFP 导出负载形态。
- 环境注记：NVRTC 为运行时依赖（intalu 旧有），本机需 `nvrtc64_120_0.dll` + `nvrtc-builtins64_129.dll` 在 exe 旁（wheel `nvidia-cuda-nvrtc-cu12` 可提取；注意 release 目录曾残留 12.4 旧版 nvrtc 导致检测器静默禁用的告警）。
