# ExtendedLimits 面测试流程（xOCD 2.0 `power_command` / `power_graph_roles` / `power_control_input` / `set_power_command`）

用途：给用户一套可直接照跑的分级验收流程，覆盖本批新增的 ExtendedLimits 底层函数，用于判断**是否封装**以及**是否追加写入封装测试**。流程分五级——离线单测（L0）、只读活体（L1/L2）、写入活体（L3）、以及封装前的裁决门（L4）。写臂全部双门控 + 自恢复；只读臂安全可随时跑。

关联：审计报告 [`xocd-oc-tool-audit.md`](./xocd-oc-tool-audit.md) §18.6；2.0 增量报告 [`xocd-2.0.0-capability-delta.md`](./xocd-2.0.0-capability-delta.md) §4。底层实现：`nvapi-rs/src/gpu.rs`（`power_graph_roles` @3164、`power_command` @3098、`set_power_command` @3133、`power_control_input` @3332）。

## 0. 被测函数清单（本次新增）

| 函数 | 线 | 类型 | 安全性 |
|---|---|---|---|
| `power_graph_roles()` | 0x2BA030（-9 回落 0xF4BF4 合成） | GET | 只读 |
| `power_command(ch, cmd)` | 0x33AB0353 / GET 0x10528 包 | GET | 只读 |
| `power_control_input(board, shared, root)` | 0x8B3E7343（Modern 0x2786E0 / Legacy 0x5B0B0） | GET | 只读 |
| `set_power_command(ch, cmd, value)` | 0x17695269 | SET | **可写**（0xFE 系内核 power-cap 请求，危险） |

包契约（GET/SET 共用）：stamp `0x10528`=v1\|1320，channel ≤ 31，cmd ∈ {0xF8 观测, 0xFE 请求}，值槽 @40*(ch+1)，0 与 0xFFFFFFFF 为 unset 哨兵被拒。详见实现注释与 sys 包定义。

## 1. L0 — 离线单测（无需 GPU，任何机器）

```bash
cd nvapi-rs
cargo test -p nvapi --lib power_graph_decode      # xocd2_power_graph_decode_tests
cargo test -p nvapi --lib xocd2_extended_limits   # 字节回放：包几何/通道门/哨兵拒绝
```

判据：全绿即封装几何（stamp/通道/命令/值槽/类型标签）正确。此级只验字节布局与拒绝逻辑，不验驱动行为。

## 2. L1/L2 — 只读活体探针（需目标 GPU，建议提权）

探针：`nvapi-rs/tests/xocd_gap_probe_live.rs::e9_extended_limits_surface`（`#[ignore]`，默认只读；写臂另需环境变量）。

```bash
cd nvapi-rs
cargo test -p nvapi --release --test xocd_gap_probe_live \
  e9_extended_limits_surface -- --ignored --nocapture
```

输出与判读按代际（Pascal/Turing = 优雅降级，是**预期**而非失败）：

- **power_graph_roles**：Ada/Blackwell 应返回 family + board/shared/root/core（可能带 memory）非空角色集；pre-Ada（Pascal/Turing/Ampere）预期 `Err(ArgumentRange)`——驱动无角色拓扑，被正确拒绝。若返回角色但 `power_control_input` 后续标 -9，说明图与输入面版本不同步，记录。
- **power_command 扫描**（本探针遍历 ch 0..31 × cmd {0xF8,0xFE}）：每条要么 `value=`，要么 `Err`。判读=**哪些 (ch,cmd) 被接受**。Pascal 实测仅 ch0(0xF8) 接受且值=unused 哨兵；Ada/Blackwell 预期更多通道接受，值随负载/功耗态有意义。
- **power_control_input**：仅当图读到 shared 角色才跑；返回 `stamp/mask/board/shared/root` 即该几何（Modern 或 Legacy）被驱动接受；两个几何都 -9 → `Ok(None)`。
- 快照落 `reverse/xocd/e9-extended-limits.json`（含 roles/command 数组/control_input/write）。

**本面在 Pascal 上 inert 的既有结论**（§18.6）：E9 只读臂在 P100 上优雅降级，`power_graph_roles=Err`、`power_command` 仅 ch0、`power_control_input` 跳过。**真正的验收需要一台 Ada（RTX 40）或 Blackwell（RTX 50）机器。**

## 3. L3 — 写入活体（`set_power_command`，双门控 + 自恢复）

唯一 SET。默认不跑；显式开启后探针执行：identity 写 baseline → 扰动 +1 → 恢复 baseline，每步读回校验，恢复失败会打印 `!! RESTORE FAILED`（响亮失败，不静默）。

```bash
cd nvapi-rs
# Windows (bash)
NVOC_ALLOW_PWR_CMD_WRITE=1 cargo test -p nvapi --release \
  --test xocd_gap_probe_live e9_extended_limits_surface -- --ignored --nocapture
```

安全约束（务必先读）：

- 写臂**只写 0xF8（观测）通道**；0xFE（内核 power-cap 请求）不在自动写臂内，需调用方显式 arm。
- 通道 baseline 为 0 / 0xFFFFFFFF 哨兵时**跳过**（无可安全扰动目标）——Pascal 因此直接跳过，属预期。
- 提权：非提权会拿到 -137/NoPermission，属预期优雅路径，不算失败。
- 写臂挑**第一个可写 0xF8 通道**即停（不遍历全部），最小扰动面。

L3 判据（决定是否值得封装写入）：identity 写被接受且读回一致 = SET 通路成立；扰动写若 `Err` 且随后 baseline 恢复成功 = 驱动钳位/拒绝（记录窗口）；恢复失败 = **红旗**，封装前必须查清。

## 4. L4 — 封装裁决门（用户决策）

只有当 L1/L2 在目标机（Ada/Blackwell）**至少一个面上有实质非哨兵数据**、且 L3 identity 写成立时，才建议封装。裁决清单：

1. **读面**（graph_roles / power_command / control_input）是否返回真实语义数据（非 0/哨兵）？是 → 封装只读命令（如 `get-power-graph`/`get-power-command`）。
2. **写面** identity SET 是否被驱动接受并读回？是 → 封装写入命令（建议镜像 `set-pwr-cur-limit` 的 `TARGET VALUE` 形态，加高危提示）。
3. 扰动/恢复是否符合单一显式窗口语义（而非静默丢弃）？若像 VF 表那样静默重建归零 → **可写面判定为无效**，不封装写。
4. 是否需要给 0xFE 单独一条命令/开关（内核 power-cap 请求语义）？默认不建议，先只封 0xF8。

结论落地：在审计报告写一段封测结论 + 若封装则补对应 `*/tests/*` 与 CLI/TUI 触点。

## 5. 一键命令汇总

```bash
# L0 离线（任何机器）
cd nvapi-rs && cargo test -p nvapi --lib xocd2

# L1/L2 只读活体（目标机）
cd nvapi-rs && cargo test -p nvapi --release --test xocd_gap_probe_live \
  e9_extended_limits_surface -- --ignored --nocapture

# L3 写入活体（目标机，双门控）
cd nvapi-rs && NVOC_ALLOW_PWR_CMD_WRITE=1 cargo test -p nvapi --release \
  --test xocd_gap_probe_live e9_extended_limits_surface -- --ignored --nocapture
```
