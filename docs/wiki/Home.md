# NVOC Wiki Home

NVOC is a Rust + Python monorepo for NVIDIA GPU overclocking, validation, and frontend orchestration. `nvoc-cli` owns direct NVAPI/NVML control commands, `auto-optimizer` orchestrates autoscan workflows, and GUI/TUI/SRV wrap operational flows for different environments.

> ⚠️ **Safety first**: Overclock writes can crash drivers, reset GPUs, and cause data loss. Start with read-only commands, verify cooling and power limits, and ensure you know recovery steps before applying write operations.

This wiki is split by component responsibilities, architecture, build/test commands, compatibility, and operational safety. Each page is mapped to one or more source-of-truth files to avoid drift.

## Navigation

- [Components](./Components.md)
- [Getting Started](./Getting-Started.md)
- [Build and Test](./Build-and-Test.md)
- [Container Usage](./Container-Usage.md)
- [Architecture](./Architecture.md)
- [Auto Optimizer](./Auto-Optimizer.md)
- [Stressors](./Stressors.md)
- [Frontends](./Frontends.md)
- [Compatibility Matrix](./Compatibility-Matrix.md)
- [Safety and Recovery](./Safety-and-Recovery.md)
- [Contributing](./Contributing.md)
- [FAQ](./FAQ.md)
- [中文首页](./Home-zh.md)

---

*Maintained from: `README.md`, `cli/README.md`, `auto-optimizer/README-en.md`, `auto-optimizer/README.md`.*
