# Verify 回路实机测试指南（release 端）

适用分支：`stressor-enhanced-refactor`（含 FP64 累加修复）。
对象：温度模拟/频率步进下，在临界频率附近验收"校验骑在负载上"回路的检测效果。

---

## 1. 环境前置（必查，否则检测器会静默禁用）

1. 构建 release：
   ```
   cargo build --release -p cli-stressor-cuda-rs --features cuda12
   ```
2. **NVRTC DLL 必须在 exe 旁**（`target/release/`）：
   - `nvrtc64_120_0.dll`（**12.9 版，约 90MB**；44MB/2024 日期的是 12.4 旧版，会让检测器静默禁用）
   - `nvrtc-builtins64_129.dll`
   - 来源：`pip download nvidia-cuda-nvrtc-cu12` 的 wheel 内 `nvidia/cuda_nvrtc/bin/`。
3. **启动后先看两件事**，缺一说明检测器没跑：
   - `[self-test] pattern_clean: OK` / `[self-test] injection_capture: OK` 两行存在；
   - summary 里每个精度下有 `detectors: ops=…` 行。
   - 若出现 `Warning: verify engine build failed`，说明 NVRTC 环境坏了——此时全绿是假的。

---

## 2. 新增开关一览

CLI（覆盖一切）：

| 开关 | 作用 |
|---|---|
| `--no-verify` | 关闭全部检测器（同时跳过自检门）——A/B 对照的"off"侧 |
| `--skip-self-test` | 只跳过注入自检门，检测器照跑 |
| `--json-out <path>` | verdict JSON 落盘（pretty）；stdout 末尾恒有 `VERDICT_JSON: {...}` 单行 |

profile TOML 的 `[verify]` 段（可省略，默认如下）：

```toml
[verify]
enabled = true            # 总开关
self_test = true          # 启动注入自检门
memcpy_every = 1          # 每 N 个 memcpy op 校验一次（seed % N == 0）
memset_every = 8          # 每 8 个 memset 迭代 1 次替换为 pattern fill+verify
gemm_every = 4            # 每 N 个 GEMM op 做一次全量扫描+采样互检
gemm_samples = 512        # 每次 GEMM 校验的采样点数
intalu_samples = 1024     # IntAlu 每窗口 gather 采样数
int8_validate_size = 512  # INT8 精确整数 sidecar 的矩阵边长
```

临界频率附近的实验建议把节奏拉满：`gemm_every=1, memset_every=1`（memcpy 已是 1）——开销上升但检测灵敏度最大；对照组务必用完全相同的配置。

---

## 3. 正常输出形态与判读

**启动期（自检门，HYDRA 式）：**
```
[self-test] pattern_clean: OK (total_errors=0, done=1024 (expected 1024))
[self-test] injection_capture: OK (total_errors=1, idx=44474..44474 (want 44474..44474), xor=0x00400000 (want 0x00400000), bit_hist[22]=1)
```
任一 FAIL → 立即 `exit 1`，verdict `classification="self_test_failed"`。含义：检测管线自证能抓住已知单错；自检不过时的"全绿"不可信。

**运行期（检测到错误即停，与 validation 失败同级）：**
```
[VERIFY] GEMM        | gemm sample check: 12/512 sampled outputs exceed tolerance (first_bad=91, max_abs_diff=8.5e-5, atol=..., rtol=...) | FAIL
```
错误前缀与域的对应：
- `memcpy verify:` / `memset verify:` —— 显存域 pattern 校验（显示 idx 区间 + 首错 exp/act + 位直方图）
- `gemm output scan:` —— C 缓冲全量扫描（non-finite/统计异常）
- `gemm sample check:` —— 采样点双算互检
- `intalu reference:` —— IntAlu 链 host 参考（位精确）

**summary 每精度新增一行：**
```
detectors: ops=6550 checked=1781232966 errors=0 mismatch=0 nonfinite=0
```

**VERDICT_JSON 关键字段：**
- `result`: `pass|fail`
- `classification`: `none | data_error(校验失败) | memory_error(检测器报错) | api_error(运行时错) | self_test_failed`
- `self_test`: 自检门逐项结果
- `precisions[].detectors`: 每精度归因的计数（ops_checked / elements_checked / mismatches / nonfinite / first_error）
- `coverage`: 检测器精确检查/筛查的元素占产出元素比
- `confidence_pct`: 100 × coverage × (1 − min(1, errors))，初版公式

退出码语义不变：0 通过、1 失败、2 参数错误。

---

## 4. 冒烟命令（逐域确认检测器活着）

```bat
:: 自检门 + memcpy 域（2s 内应有 detectors 行、coverage≈1）
cli-stressor-cuda-rs.exe --duration 5 --precisions fp32 --kernel-types memcpy --matrix-sizes 4096 --validate-interval 0 --json-out v-memcpy.json

:: memset 域（注意第 8 个迭代起才出现 fill+verify）
cli-stressor-cuda-rs.exe --duration 5 --precisions fp32 --kernel-types memset --matrix-sizes 4096 --validate-interval 0

:: GEMM 域（stats + 512 点采样互检）
cli-stressor-cuda-rs.exe --duration 10 --precisions fp32 --kernel-types gemm --matrix-sizes 2048,4096 --validate-interval 0

:: FP64（回归验证：double 累加修复后必须 0 mismatch；双转置最苛刻）
cli-stressor-cuda-rs.exe --duration 25 --precisions fp64 --kernel-types gemm --burst-iters 20 --matrix-sizes 4096 --minor-mixture-rate 0 --validate-interval 8

:: IntAlu 参考域（INT32 链逐位对照）
cli-stressor-cuda-rs.exe --duration 5 --precisions int32 --kernel-types intalu --matrix-sizes 1024 --validate-interval 0
```

每条命令判读：`self-test` 两行 OK + summary `OK` + `detectors: ops>0` + VERDICT `result=pass`。
`checked` 数值量级 sanity：memcpy 5s@4096 应在 10⁹ 量级。

---

## 5. 温度偏移 × 临界频率实验协议

**目标**：证明检测器在"崩溃/TDR 之前的亚崩溃区间"能看到 CUDA-only 判据看不到的东西。

**稳定性偏移手段**（任选其一，按你现有工具链）：
- 锁频步进：`nvoc` 的 `set-perf-freq-caps`（需管理员）把核心锁在临界带附近（先由 autoscan 找到大致临界点，再 ±1~2 步），配合风扇/加热改变温度；
- thermal-sim NVAPI 三件套（若该 SKU 未被 VBIOS Secured Overrides 拦截）直接注入虚拟温度；
- 或物理升温（功率预烤 + 关风扇）拉高结温等效于降裕量。

**A/B 对照协议（关键：除 verify 外全同）**：
```
:: ON 侧（同 seed、同 duration、同尺寸）
cli-stressor-cuda-rs.exe --seed 777 --duration 30 --precisions fp32 --kernel-types gemm --matrix-sizes 4096 --minor-mixture-rate 0 --validate-interval 0 --json-out on.json
:: OFF 侧
cli-stressor-cuda-rs.exe --no-verify --seed 777 --duration 30 --precisions fp32 --kernel-types gemm --matrix-sizes 4096 --minor-mixture-rate 0 --validate-interval 0 --json-out off.json
```

**在每个频率点上记录**：
1. ON 侧 verdict：`classification`（memory_error/data_error）+ `precisions[0].detectors.{mismatches,nonfinite,total_errors}` + first_error 全文（含 idx 区间/位直方图）；
2. OFF 侧：只有"撑过/崩溃(TDR/死机)"两种结果；
3. 事件日志：FECS/TDR 事件号与时刻（和 auto-optimizer 现有轮询一致）；
4. 温度/实际核心频率（nvoc 打点）。

**期望现象（检测器生效的判据）**：
- 临界带内，ON 侧先出现 `gemm sample check: N/512 …` 或 `memcpy verify: …`，且随频率/温度偏移单调恶化（mismatch 数上升）——**给出连续的"错误率 vs 频率"曲线**；
- OFF 侧同频率点要么全绿、要么直接跳到崩溃——中间没有任何信号；
- 自检门在任何频率都应 OK（若临界点上自检门都 FAIL，说明已经深到检测器自身也在错——这本身就是极强的失稳信号，记下来）；
- 若 ON 侧报错但同点 OFF 侧多轮全绿，把 first_error 全文发我，先排除参考语义类误报（FP64 已修，FP32/16 的容差余量大）。

**边界提醒**：`--no-verify` 侧在临界带内有真实死机/TDR 风险（这正是对照的意义），请确保 PnP 恢复脚本/重启预案就位；ON 侧因为错误会提前退出，通常先于崩溃，风险显著更低。

---

## 6. 已知语义注记（判读时勿误报）

- 设备 C 的实际语义是 row-major **B·A**（cudarc 原样透传 cuBLAS 列主序调用所致）；采样互检与 INT8 参考已按该语义对齐，旧 sidecar 的 CPU 参考因同一错法而历史一致——这不是检测器误报。
- FP64 采样用 double 累加（本分支修复：float resum 会注入 ~1e-4 误差造成临界频率以下的全面误报）；FP32/16/INT8 用 float/int32。
- FP32 的采样容差按 √(K/1024) 缩放，若在临界点附近看到 **个位数** mismatch 且 max_abs_diff 贴着容差量级（如 1e-2 相对），先加倍容差复跑确认是真 SDC 还是边缘计算噪声——真 SDC 的特征是 mismatch 数随偏移量陡增且 idx/位分布集中。

---

## 7. OpenCL 移植验收清单（下一步）

移植后按同一协议回归，清单：
1. 自检门双 gate（同 0xADBA/bit22 常量）；
2. 采样互检与 OpenCL gemm kernel 的索引语义一致（OpenCL 版是自写 kernel，教科书 row-major A·B，无 cudarc 陷阱，但要与 transpose 分支核对）；
3. FP64 用 double 累加（同本修复）；
4. 显存域：常驻 pattern 块周期校验（等价 CUDA 版 memcpy/memset 域）；
5. `VERDICT_JSON:` 单行 + `--json-out`，classification 枚举一致；
6. 退出码与 auto-optimizer 的接法不变；
7. 对照实验重复第 5 节协议，得到 OpenCL 线自己的"错误率 vs 频率"曲线——若该曲线与 CUDA 线在临界带内重合度高，即证明 OpenCL 判据被"恢复到基本可用"。
