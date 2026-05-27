# Contributing to NVOC

## EN

NVOC is a mixed Rust and Python monorepo. Keep changes scoped to the component you are modifying, and update the relevant component README when behavior, commands, or setup steps change.

### Repository Areas

- `auto-optimizer/`: Rust CLI core and shared overclocking behavior.
- `cli-stressor-cuda/`: CUDA/PyTorch stress workload.
- `cli-stressor-opencl/`: OpenCL stress workload.
- `gui/`: Python GUI frontend.
- `tui/`: Python Textual terminal frontend.
- `srv/`: Windows service wrapper and localhost control endpoint.

### Development Checks

Run the checks that match the files you changed:

```bash
cd auto-optimizer && cargo build
cd srv && cargo build
cd tui && uv run pytest
```

For Python components, run `uv sync` before local testing when dependencies have changed.

### Safety

Changes that write GPU state need extra care. Document the tested GPU generation, driver, operating system, and whether the change uses NVAPI, NVML, CUDA, or OpenCL. Prefer read-only validation before write operations, and keep recovery/reset behavior visible in the docs.

### Documentation

Use monorepo-relative links for internal references. The canonical repository URL is:

```text
https://github.com/Skyworks-Neo/nvoc
```

## 中文

NVOC 是一个 Rust 与 Python 混合的单仓库项目。请将修改范围限定在你所编辑的组件内；当行为、命令或安装步骤发生变化时，更新对应组件的 README。

### 仓库区域

- `auto-optimizer/`：Rust CLI 核心与共享超频行为。
- `cli-stressor-cuda/`：CUDA/PyTorch 压力负载。
- `cli-stressor-opencl/`：OpenCL 压力负载。
- `gui/`：Python GUI 前端。
- `tui/`：Python Textual 终端前端。
- `srv/`：Windows 服务封装与本地控制端点。

### 开发检查

运行与你改动的文件对应的检查：

```bash
cd auto-optimizer && cargo build
cd srv && cargo build
cd tui && uv run pytest
```

当 Python 组件依赖有变化时，先执行 `uv sync` 再进行本地测试。

### 安全

涉及写入 GPU 状态的改动需要特别谨慎。请记录测试的 GPU 世代、驱动、操作系统，以及改动是否使用 NVAPI、NVML、CUDA 或 OpenCL。优先在写入操作前进行只读验证，并在文档中保留恢复/重置行为说明。

### 文档

内部引用请使用单仓库相对链接。仓库的标准 URL 为：

```text
https://github.com/Skyworks-Neo/nvoc
```
