# Contributing

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

### Repository Areas

| Directory | Description |
|---|---|
| `auto-optimizer/` | Rust CLI core and shared overclocking behavior |
| `cli-stressor-cuda/` | CUDA/PyTorch stress workload |
| `cli-stressor-opencl/` | OpenCL stress workload |
| `cli-stressor-cuda-rs/` | Rust CUDA stress workload |
| `gui/` | Python GUI frontend |
| `tui/` | Python Textual terminal frontend |
| `srv/` | Windows Service wrapper and local control endpoint |

### Development Checks

Run the appropriate checks based on modified files:

```bash
# Rust
cd auto-optimizer && cargo build
cd srv && cargo build

# Python TUI
cd tui && uv run pytest

# Python format & lint
ruff format . --check && ruff check .
```

Additional Rust checks:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --exclude cli-stressor-cuda-rs --all-targets -- -D warnings
cargo test --package nvoc-core --all-targets
```

When Python dependencies change, run `uv sync` before local testing.

### Scope of Changes

- Limit changes to the target component
- Update the corresponding component README when behavior, commands, or installation steps change
- When affecting shared behavior, start with `auto-optimizer/`, then verify GUI/TUI/SRV

### Safety Requirements

Exercise extra caution with changes involving GPU state writes:

- Document the tested GPU generation, driver, and OS
- Indicate whether NVAPI, NVML, CUDA, or OpenCL is used
- Prefer read-only verification before write operations
- Keep recovery/reset behavior descriptions in documentation

### Documentation

- Use monorepo-relative links for internal references
- Standard repository URL: `https://github.com/Skyworks-Neo/nvoc`

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

| 目录 | 说明 |
|---|---|
| `auto-optimizer/` | Rust CLI 核心与共享超频行为 |
| `cli-stressor-cuda/` | CUDA/PyTorch 压力负载 |
| `cli-stressor-opencl/` | OpenCL 压力负载 |
| `cli-stressor-cuda-rs/` | Rust CUDA 压力负载 |
| `gui/` | Python GUI 前端 |
| `tui/` | Python Textual 终端前端 |
| `srv/` | Windows 服务封装与本地控制端点 |

### 开发检查

根据修改的文件运行对应检查：

```bash
# Rust
cd auto-optimizer && cargo build
cd srv && cargo build

# Python TUI
cd tui && uv run pytest

# Python 格式与 lint
ruff format . --check && ruff check .
```

Rust 项目额外检查：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --exclude cli-stressor-cuda-rs --all-targets -- -D warnings
cargo test --package nvoc-core --all-targets
```

Python 依赖变更时，先执行 `uv sync` 再本地测试。

### 修改范围

- 将修改限定在目标组件内
- 行为、命令或安装步骤变更时，更新对应组件 README
- 涉及共享行为时，从 `auto-optimizer/` 开始，再验证 GUI/TUI/SRV

### 安全要求

涉及写入 GPU 状态的改动需特别谨慎：

- 记录测试的 GPU 世代、驱动、操作系统
- 注明使用的是 NVAPI、NVML、CUDA 还是 OpenCL
- 优先在写入操作前进行只读验证
- 在文档中保留恢复/重置行为说明

### 文档

- 内部引用使用 monorepo 相对链接
- 仓库标准 URL：`https://github.com/Skyworks-Neo/nvoc`

### 提交规范

- 使用简短的祈使语气提交摘要
- 常用前缀：`core:` · `fix(clippy):` · `gui:` · `tui:` 等
- PR 应说明组件、行为变更、关联 issue 和已运行的测试

### 许可证

贡献的代码遵循 Apache License 2.0。
