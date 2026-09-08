# Safety and Recovery

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

Write operations are the highest-risk surface of NVOC: they change clocks, voltage/frequency points, power limits, and fan behavior directly on the hardware. Read-only validation first, writes second, recovery always visible.

## High-risk operations

- Any write operation to clocks, voltage/frequency points, power limits, fan settings.
- Bulk scan loops without thermal/power observation.
- Writes issued from inside containers — container isolation does not isolate GPU hardware state (see [[Container-Usage]]).

## Recovery controls

- TDR registry strategy (Windows) for driver timeout behavior.
- `auto-optimizer/systemd/` units and scripts for controlled startup/recovery on Linux.
- Reset command path (`reset`) as first-line rollback.
- After any write test, verify final state with read-only readback commands before declaring success.

## Do-not-OC cases

- Unknown cooling or unstable PSU.
- Production workload without maintenance window.
- Unsupported/untested GPU + backend combination — check [[GPU-Support-Matrix]] before relying on a combination.

## Related pages

- [[Auto-Optimizer-Guide]] — autoscan aggressiveness and conservative baselines.
- [[CLI-Guide]] — reset and read-only command families.
- [[Safety-and-Recovery]] practices also apply to stress testing: see [[Stress-Testing]].

---

*Maintained from: `docs/wiki/Safety-and-Recovery.md`, `auto-optimizer/README.md`, `auto-optimizer/systemd/`, platform recovery docs/scripts.*

<a id="chinese"></a>

## 中文

写入操作是 NVOC 中风险最高的部分：它们会直接改变硬件上的频率、电压/频率点、功耗限制和风扇行为。正确的顺序是：先做只读验证，再执行写入，并始终保证恢复路径可见。

## 高风险操作

- 任何针对频率、电压/频率点（V-F point）、功耗限制、风扇设置的写入操作。
- 没有温度/功耗观测配合的批量扫描循环。
- 从容器内部发出的写入 —— 容器隔离并不能隔离 GPU 硬件状态（见 [[Container-Usage]]）。

## 恢复手段

- TDR 注册表策略（Windows），用于控制驱动超时行为。
- `auto-optimizer/systemd/` 的 unit 与脚本，用于 Linux 上受控的启动/恢复。
- 重置命令路径（`reset`）作为第一道回滚手段。
- 任何写入测试之后，先用只读回读命令核对最终状态，再宣告成功。

## 不应超频的情形

- 散热情况不明或电源不稳。
- 没有维护窗口的生产负载。
- 未支持/未测试的 GPU + 后端组合 —— 依赖某个组合前先查阅 [[GPU-Support-Matrix]]。

## 相关页面

- [[Auto-Optimizer-Guide]] — autoscan 激进程度与保守基线。
- [[CLI-Guide]] — 重置与只读命令族。
- 压力测试同样适用这些安全实践：见 [[Stress-Testing]]。

---

*维护来源：`docs/wiki/Safety-and-Recovery.md`、`auto-optimizer/README.md`、`auto-optimizer/systemd/`、各平台恢复文档与脚本。*
