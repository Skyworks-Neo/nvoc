# Contributing

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

### Repository Areas

Keep changes scoped by component.

| Directory | Description |
|---|---|
| `auto-optimizer/` | Rust CLI core: autoscan, V-F curve import/export, result fixing |
| `cli/` | nvoc-cli: direct NVAPI/NVML control commands over `nvoc-core` |
| `core/` | nvoc-core: shared Rust overclocking/domain library |
| `cli-common/` | Shared CLI support layer for Rust command-line components |
| `nvoc-python/` | pynvoc: native Python bindings (PyO3 + maturin) |
| `cli-stressor-opencl/` | OpenCL stress workload (Python) |
| `cli-stressor-cuda-rs/` | Rust CUDA stress workload |
| `gui/` | Python GUI frontend (customtkinter) |
| `tui/` | Python Textual terminal frontend |
| `srv/` | Windows Service wrapper and localhost HTTP control layer |

> The former Python/PyTorch stressor `cli-stressor-cuda/` was removed in #200.

### Development Checks

Run the local command set aligned with CI for the paths you touched:

```bash
# Rust workspace build (excludes the CUDA-Rust stressor)
cargo build --workspace --exclude cli-stressor-cuda-rs

# Rust format & lint
cargo fmt --all -- --check
cargo clippy --workspace --exclude cli-stressor-cuda-rs --all-targets -- -D warnings

# Core (nvoc-core) tests
cargo test --package nvoc-core --all-targets

# Python format & lint
ruff format . --preview --check --output-format=github && ruff check . --output-format=github

# Python frontends
cd gui && uv sync && uv run pytest
cd tui && uv sync && uv run pytest
```

When Python dependencies change, run `uv sync` before local testing.

### Scope of Changes

- Limit changes to the target component (`auto-optimizer`, `core`, `cli`, `gui`, `tui`, `srv`, stressors)
- Update the corresponding component README when behavior, commands, or installation steps change
- When affecting shared behavior, start with `auto-optimizer/`, then verify the GUI/TUI/SRV wrappers that invoke it

### Safety Requirements

Exercise extra caution with changes involving GPU state writes:

- Document the tested GPU generation, driver, and OS, and the hardware assumptions
- Indicate whether NVAPI, NVML, CUDA, or OpenCL is used, and the safety/recovery behavior of the change
- Prefer read-only verification before write operations
- Keep recovery/reset behavior descriptions in documentation

### GPU CI

- Use path filters to avoid unnecessary GPU CI
- Trigger GPU CI only when relevant paths or labels require it

### PR Checklist

- Component + behavior summary
- Linked issue(s)
- Tests/lints/build commands run
- GPU availability note and any skipped GPU-only checks

### Wiki Documentation Workflow

Long-form documentation is maintained in `docs/wiki/` inside the monorepo:

1. Edit docs in `docs/wiki/*.md` and open a PR in `nvoc` for technical and language review.
2. After merge, the approved markdown is synced to this GitHub Wiki.
3. If a wiki page differs from `docs/wiki/`, treat `docs/wiki/` as the source of truth.

### Commit Conventions

- Use short imperative commit summaries
- Common prefixes: `core:` · `fix(clippy):` · `gui:` · `tui:` etc.
- PRs should describe the component, behavioral changes, linked issues, and tests run

### License

Contributed code is licensed under Apache License 2.0.

---

<a id="chinese"></a>

## 中文

### 仓库区域

将修改限定在单个组件内。

| 目录 | 说明 |
|---|---|
| `auto-optimizer/` | Rust CLI 核心：autoscan、V-F 曲线导入导出、结果后处理 |
| `cli/` | nvoc-cli：基于 `nvoc-core` 的直接 NVAPI/NVML 控制命令 |
| `core/` | nvoc-core：共享 Rust 超频/设备领域库 |
| `cli-common/` | Rust 命令行组件的共享支撑层 |
| `nvoc-python/` | pynvoc：原生 Python 绑定（PyO3 + maturin） |
| `cli-stressor-opencl/` | OpenCL 压力负载（Python） |
| `cli-stressor-cuda-rs/` | Rust CUDA 压力负载 |
| `gui/` | Python GUI 前端（customtkinter） |
| `tui/` | Python Textual 终端前端 |
| `srv/` | Windows 服务封装与 localhost HTTP 控制层 |

> 原 Python/PyTorch 压力测试工具 `cli-stressor-cuda/` 已在 #200 中移除。

### 开发检查

针对改动的路径，运行与 CI 一致的本地命令：

```bash
# Rust workspace 构建（不含 CUDA-Rust 压力测试）
cargo build --workspace --exclude cli-stressor-cuda-rs

# Rust 格式与 lint
cargo fmt --all -- --check
cargo clippy --workspace --exclude cli-stressor-cuda-rs --all-targets -- -D warnings

# 核心（nvoc-core）测试
cargo test --package nvoc-core --all-targets

# Python 格式与 lint
ruff format . --preview --check --output-format=github && ruff check . --output-format=github

# Python 前端
cd gui && uv sync && uv run pytest
cd tui && uv sync && uv run pytest
```

Python 依赖变更时，先执行 `uv sync` 再本地测试。

### 修改范围

- 将修改限定在目标组件内（`auto-optimizer`、`core`、`cli`、`gui`、`tui`、`srv`、stressors）
- 行为、命令或安装步骤变更时，更新对应组件 README
- 涉及共享行为时，从 `auto-optimizer/` 开始，再验证调用它的 GUI/TUI/SRV 封装

### 安全要求

涉及写入 GPU 状态的改动需特别谨慎：

- 记录测试的 GPU 世代、驱动、操作系统，以及硬件假设
- 注明使用的是 NVAPI、NVML、CUDA 还是 OpenCL，并说明改动的安全/恢复行为
- 优先在写入操作前进行只读验证
- 在文档中保留恢复/重置行为说明

### GPU CI

- 使用路径过滤（path filters）避免触发不必要的 GPU CI
- 仅当相关路径或标签需要时才触发 GPU CI

### PR 清单

- 组件 + 行为变更摘要
- 关联 issue
- 已运行的测试/lint/构建命令
- GPU 可用性说明，以及跳过的 GPU-only 检查

### Wiki 文档工作流

长篇文档统一维护在 monorepo 的 `docs/wiki/` 目录：

1. 在 `docs/wiki/*.md` 中编辑文档，并在 `nvoc` 仓库提交 PR 进行技术与语言审阅。
2. 合并后，将审阅通过的 markdown 同步到本 GitHub Wiki。
3. 若 Wiki 页面与 `docs/wiki/` 不一致，以 `docs/wiki/` 为准。

### 提交规范

- 使用简短的祈使语气提交摘要
- 常用前缀：`core:` · `fix(clippy):` · `gui:` · `tui:` 等
- PR 应说明组件、行为变更、关联 issue 和已运行的测试

### 许可证

贡献的代码遵循 Apache License 2.0。
