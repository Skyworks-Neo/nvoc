# msys2 clang64 (x86_64-pc-windows-gnullvm) 兼容性台账

状态：**已验证可用**（2026-09-16，本机实测）。msys2 是 nvoc 在 Windows 上的第二构建面：
msvc 面（rustup + MSVC Build Tools，CI/release 的出货面）之外，clang64 环境可以完整
构建并运行整个 workspace。xtask doctor 按解析到的 rustc host 三元组自动分流，两面并存。

## 实测环境

- 参照结构：`C:\msys64`（msys2 官方安装器，winget 包 ID `MSYS2.MSYS2`）
- 环境：`clang64`（UCRT 运行时；ucrt64/mingw64 等其他环境未填充）
- 工具链：clang/LLVM/lld 22.1.8 + rustc/cargo **1.98.1**（MSYS2 Rev1, 2026-08-05），
  host `x86_64-pc-windows-gnullvm`；`clang64/lib/rustlib/` 仅含 gnullvm 一个目标
  ——**没有 msvc 目标的 std，两个构建面互斥，不能在同一工具链内切换目标**
- 无 rustup：msys2 的 cargo 不读 rust-toolchain.toml，1.95.0 钉版不生效；
  1.98.1 ≥ workspace MSRV 1.95（edition 2024 ✓），版本随 `pacman -Syu` 演进

## 试建结果（`PATH=/c/msys64/clang64/bin:$PATH`，dev profile）

| 步骤 | 结果 |
| --- | --- |
| `cargo check --workspace`（169 单元，含 pyo3 0.29.2/pynvoc、compio、nvapi-rs、cudarc、ash/naga、windows-sys/windows-service） | ✅ EXIT 0，59.6s |
| `cargo build --workspace`（全部 bin + pynvoc `_native.dll` cdylib 最终链接） | ✅ EXIT 0，58.78s |
| pynvoc 扩展链接（pyo3 → MSVC python import lib，lld 消化） | ✅（clang64=UCRT 与 uv 管理的 CPython 同 CRT 是关键） |
| 产物运行冒烟（`nvoc-cli --help`） | ✅ 需 `clang64\bin` 在 PATH |
| 外部 DLL 依赖（ntldd -R） | 仅 `libunwind.dll`（`C:\msys64\clang64\bin` 内） |
| 噪音 | 本机已知 os error 5 incremental 收尾告警（与 gnullvm 无关，见 memory 台账） |

产物落在 `target/debug/`，与 msvc 单元靠不同 `-C metadata` 哈希共存。

## 工作流注意事项

- **双面切换会互相作废指纹**：cargo 指纹含 rustc 版本串，msvc(1.95.0)/gnullvm(1.98.1)
  交替构建同一个 `target/debug` 会每次全量重编。偶发用第二面时建议
  `export CARGO_TARGET_DIR=~/nvoc-target-gnullvm` 隔离；固定在 clang64 shell 里干活的
  用户无感。
- 运行 gnullvm 产物需要 `C:\msys64\clang64\bin` 在 PATH（libunwind.dll）——clang64
  shell 内天然满足；普通 Windows shell 运行请用 msvc 面产物。
- uv 默认不在 msys2 minimal PATH 上：用 `msys2_shell.cmd -clang64 -use-full-path`
  启动（doctor 的 uv 检查会给出同款提示）。
- CUDA：cudarc 运行时 dlopen，gnullvm 链接不需要 CUDA toolkit（与 msvc 面一致）。

## 工具链二选一：LLVM vs GCC

`setup.cmd --msys2 [llvm|gcc]`（缺省 llvm）：

| 选项 | msys2 环境 | 包前缀 | rust host | 运行时 | 状态 |
| --- | --- | --- | --- | --- | --- |
| `llvm`（默认） | `clang64` | `mingw-w64-clang-x86_64-*` | `x86_64-pc-windows-gnullvm` | UCRT | **本文档全量实测 ✅** |
| `gcc` | `ucrt64` | `mingw-w64-ucrt-x86_64-*` | `x86_64-pc-windows-gnu` | UCRT | 已实现，待实机验证（需在自有 msys2 安装上跑 `setup.cmd --msys2 gcc`） |

GCC 面选 **ucrt64 而非 mingw64**：ucrt64 与 uv 管理的 MSVC CPython 同用 UCRT
（pynvoc/pyo3 链接成立的前提，clang64 面实测的关键正是同 CRT）；mingw64 是 msvcrt
旧运行时，有 CRT 混用风险，不提供。两面的 rust 包版本同源（实测 ucrt64 仓库
rust 1.98.1-1 与 clang64 同版），包名与 toolchain 组已对 pacman 源只读复核存在。

doctor 对 gcc 面零额外改动：`windows-gnu` host 本就归类 gnu-like，native 检查按
clang/gcc 在位情况探测（gcc 面命中 gcc）。windows-gnu 的 mingw 链接组件由工具链
自带（self-contained），与 gnullvm 同样不需要 Visual Studio。

## 落地的兼容改动

- `setup.cmd --msys2 [llvm|gcc]`：可选引导（winget 装 MSYS2 → keyring init + 双跑
  `-Syu` → 按选项装 `mingw-w64-{clang,ucrt}-x86_64-toolchain` + 对应 rust 包）；
  `--msys2` 下缺 MSVC Build Tools 不再拦截。
- `xtask` doctor：`rustc -vV` 的 host 三元组分流——`*-msvc` 走原 vswhere 门，
  gnu-like（gnullvm/gnu）改查 clang 是否在位（warn-only，链接走 rust-lld）并提示
  运行期 PATH 需求；非 rustup 工具链的版本提示改为指向 pacman；
  `check_uv` 在 uv 不在 PATH 时探测 WinGet Links / `~/.local/bin` 安装位并给定向提示。
- CI/release 面不变（msvc）；gnullvm 是本机开发能力，不是出货目标。
