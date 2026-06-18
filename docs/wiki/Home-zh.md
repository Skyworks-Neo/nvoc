# NVOC Wiki 中文首页

NVOC 是一个围绕 NVIDIA GPU 超频与稳定性验证的 Rust/Python monorepo。`nvoc-cli` 负责直接的 NVAPI/NVML 控制命令，`auto-optimizer` 负责 autoscan 工作流编排，GUI/TUI/SRV 提供不同运行环境下的操作入口。

> ⚠️ **安全警告**：超频写入可能导致驱动崩溃、GPU 重置、系统不稳定甚至数据丢失。请先做只读验证，确认散热/供电/回退方案后再执行写入。

## 导航

- [组件总览](./Components.md)
- [快速开始](./Getting-Started.md)
- [构建与测试](./Build-and-Test.md)
- [架构](./Architecture.md)
- [自动优化器](./Auto-Optimizer.md)
- [压力测试器](./Stressors.md)
- [前端](./Frontends.md)
- [兼容性矩阵](./Compatibility-Matrix.md)
- [安全与恢复](./Safety-and-Recovery.md)
- [贡献指南](./Contributing.md)
- [发布路线图](./Release-Roadmap.md)
- [FAQ](./FAQ.md)

---

*维护来源：`README.md`、`cli/README.md`、`auto-optimizer/README.md`、`auto-optimizer/README-en.md`。*
