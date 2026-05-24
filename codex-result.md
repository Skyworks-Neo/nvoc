已完成。

变更要点：
- 在 [types.rs](/data/worktrees/job-15/nvoc-core/src/types.rs:3) 定义 core 统一单位：`Millivolts`/`MillivoltsDelta`、`Megahertz`/`MegahertzDelta`。
- core 结构化 API 改为电压用 `mv`、频率用 `mhz`，内部再转换成 NVAPI 需要的 uV/kHz，例如 [operation.rs](/data/worktrees/job-15/nvoc-core/src/operation.rs:517)。
- 编号逻辑未改。
- 更新了 `auto-optimizer` 和 `nvoc-python` 里受 core API 影响的调用点，避免 workspace 编译断掉。

验证：
- `cargo test --package nvoc-core --all-targets`：默认 pinned `1.95.0` 因镜像 404 无法下载。
- `RUSTUP_TOOLCHAIN=stable cargo test --package nvoc-core --all-targets`：通过。
- `RUSTUP_TOOLCHAIN=stable cargo check --workspace --exclude cli-stressor-cuda-rs`：通过。
- `RUSTUP_TOOLCHAIN=stable cargo clippy --workspace --exclude cli-stressor-cuda-rs --all-targets -- -D warnings`：通过。
- `RUSTUP_TOOLCHAIN=stable cargo fmt --all -- --check`：通过。