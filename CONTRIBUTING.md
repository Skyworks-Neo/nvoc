# Contributing to NVOC

## EN

NVOC is a mixed Rust and Python monorepo. Keep changes scoped to the component you are modifying, and update the relevant component README when behavior, commands, or setup steps change.

### Repository Areas

Use the canonical component inventory in [`README.md` → "Components (canonical)"](./README.md#components-canonical). That table defines user-facing products vs internal libraries/modules and is the single source of truth.

### Submodule Forks

The `nvapi-rs` URL in `.gitmodules` is intentionally relative. If you fork NVOC,
also fork `nvapi-rs` into the same account or organization so the relative URL
resolves to your fork. If you only fork NVOC, override the submodule URL locally
before initializing it:

```bash
git config submodule.nvapi-rs.url git@github.com:Skyworks-Neo/nvapi-rs.git
git submodule update --init
```

Do not check out `v0.2.x` or another branch inside the submodule. The
superproject's pinned commit is the source version validated by CI.

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

组件清单请以 [`README.md` 的 “Components (canonical)” 章节](./README.md#components-canonical) 为准。该清单区分了用户向产品与内部库/模块，并作为唯一权威来源。

### 子模块 fork

`.gitmodules` 中的 `nvapi-rs` URL 特意使用相对路径。fork NVOC 时，请同时将
`nvapi-rs` fork 到同一用户或组织下，使相对 URL 指向你的 fork。如果只 fork
NVOC，请在初始化子模块前于本地覆盖其 URL：

```bash
git config submodule.nvapi-rs.url git@github.com:Skyworks-Neo/nvapi-rs.git
git submodule update --init
```

请勿在子模块内另行切换到 `v0.2.x` 或其他分支。CI 验证的是主仓库固定的
submodule commit。

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
