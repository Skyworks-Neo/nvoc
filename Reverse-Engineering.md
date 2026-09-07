# Reverse Engineering

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

NVOC keeps reverse-engineering material to document coverage decisions and avoid repeating failed investigations. The raw notes and generated evidence tables live in the monorepo under `docs/reverse-engineering/`. They are internal evidence snapshots, not a supported user-facing API contract.

Related pages: [[GPU-Support-Matrix]] (documented backend coverage per GPU generation) and [[Safety-and-Recovery]] (recovery rules that also govern RE-adjacent experiments).

## Current project boundary

NVOC production code prefers documented NVAPI and NVML interfaces. Undocumented NVAPI handlers may be useful for read-only discovery, but their structure versions and behavior can vary by driver, GPU, board, and privilege level. Direct PCI/MMIO access is even less portable and would require privileged platform support plus a per-device hardware database, so it is not part of the supported NVOC path.

The archived investigations currently support these decisions:

- GPU-Z's per-rail telemetry investigation found a direct PCI/MMIO path through a kernel driver. Separate NVAPI PowerMonitor handlers can expose useful descriptors or status on some systems, but the available layouts are not a portable product contract.
- Memory-hotspot MMIO offsets and decoding are architecture- and board-specific. A value observed on one GPU must not be generalized without independent verification.
- NVAPI and NVML coverage audits are point-in-time inventories. Always compare them with current bindings and the current driver before treating a listed gap as open.

## Safety requirements

Reverse-engineering work must start read-only. Raw probes stay ignored by default and require explicit compatible-hardware selection. Private setters are not promoted from an observed signature alone: they need a recovery plan, cross-hardware validation, and a focused review of failure semantics.

Never commit proprietary NVIDIA binaries, GPU-Z executables, disassembler databases, firmware dumps, or machine-specific probe output. Repository ignore rules cover the common local artifacts.

## Archive navigation

Inside the monorepo:

- `docs/reverse-engineering/README.md` — archive index and promotion checklist (promoting archive work into NVOC).
- `docs/reverse-engineering/gpu-z/` — GPU-Z and direct-hardware access investigations.
- `docs/reverse-engineering/nvapi/` — NVAPI audits and generated evidence.
- `docs/reverse-engineering/nvml/` — NVML export coverage.

These paths are internal to the monorepo and are not published on this wiki.

---

*Maintained from: `docs/wiki/Reverse-Engineering.md`, dated investigations under `docs/reverse-engineering/`, and current project safety policy.*

<a id="chinese"></a>

## 中文

NVOC 保存逆向工程材料，用于记录覆盖范围决策，避免重复已经失败的调查。原始笔记和生成的证据表格存放在 monorepo 的 `docs/reverse-engineering/` 目录下。它们是内部的证据快照，不是受支持的用户侧 API 契约。

相关页面：[[GPU-Support-Matrix]]（各 GPU 世代已记录的后端覆盖情况）与 [[Safety-and-Recovery]]（同样约束逆向相关实验的恢复规则）。

## 当前项目边界

NVOC 的生产代码优先使用有文档的 NVAPI 和 NVML 接口。未公开的 NVAPI handler 对只读探测可能有用，但其结构体版本和行为会随驱动、GPU、板卡和权限级别而变化。直接 PCI/MMIO 访问的可移植性更差，需要特权的平台支持以及按设备维护的硬件数据库，因此不属于 NVOC 的受支持路径。

现有的存档调查支撑以下决策：

- GPU-Z 每电源轨（per-rail）遥测调查发现了一条经由内核驱动的直接 PCI/MMIO 路径。独立的 NVAPI PowerMonitor handler 在部分系统上可以暴露有用的描述符或状态，但已有的数据结构布局并不构成可移植的产品契约。
- 显存热点（memory-hotspot）MMIO 偏移与解码方式依赖具体架构和板卡。在某一块 GPU 上观测到的数值，未经独立验证不得推广。
- NVAPI 与 NVML 覆盖审计只是某一时点的清单。在把某个列为空缺的能力当作仍然空缺之前，务必与当前 bindings 和当前驱动重新比对。

## 安全要求

逆向工程工作必须从只读开始。原始探测（raw probes）默认被忽略，需要显式选择兼容硬件才会启用。私有 setter 不能仅凭观测到的签名就转为正式实现：它们需要恢复方案、跨硬件验证，以及对失败语义的专项评审。

严禁提交专有的 NVIDIA 二进制文件、GPU-Z 可执行文件、反汇编数据库、固件转储或特定机器的探测输出。仓库的忽略规则已覆盖常见的本地产物。

## 存档导航

在 monorepo 内部：

- `docs/reverse-engineering/README.md` — 存档索引与晋升清单（将存档成果纳入 NVOC 的流程）。
- `docs/reverse-engineering/gpu-z/` — GPU-Z 与直接硬件访问调查。
- `docs/reverse-engineering/nvapi/` — NVAPI 审计与生成的证据。
- `docs/reverse-engineering/nvml/` — NVML 导出覆盖情况。

这些路径属于 monorepo 内部资料，不在本 wiki 上公开。

---

*维护来源：`docs/wiki/Reverse-Engineering.md`、`docs/reverse-engineering/` 下的带日期调查记录，以及当前项目安全策略。*
