# Frontends

[English](#english) | [中文](#chinese)

<a id="english"></a>

## English

NVOC ships three user-facing frontends on top of the shared optimizer/CLI core. Pick one per machine: a desktop GUI, a terminal UI, or a service + HTTP control layer.

## GUI

- Path: `gui/`
- Start: `cd gui && uv sync && uv run python main.py`
- Requires Python Tk support (`tkinter`); on Linux this may require `tk`, `python3-tk`, or `python3-tkinter`.
- Use case: desktop operational control with visual tabs and live output.
- Details: [[GUI-Guide]]

## TUI

- Path: `tui/`
- Start: `cd tui && uv sync && uv run nvoc-tui`
- Use case: terminal-first operation, remote/SSH friendly.
- Details: [[TUI-Guide]]

## SRV

- Path: `srv/`
- Use case: service lifecycle and localhost HTTP control in managed environments.
- Details: [[SRV-Guide]]

## Config & CLI discovery

Frontends should explicitly resolve `auto-optimizer` executable path/config and expose it in settings to avoid hidden path failures.

---

*Maintained from: `docs/wiki/Frontends.md`, `gui/README.md`, `tui/README.md`, `srv/README.md`, frontend config sources.*

<a id="chinese"></a>

## 中文

NVOC 在共享的 optimizer/CLI 核心之上提供三个面向用户的前端。按机器场景选择其一：桌面 GUI、终端 UI，或服务 + HTTP 控制层。

## GUI

- 路径：`gui/`
- 启动：`cd gui && uv sync && uv run python main.py`
- 需要 Python Tk 支持（`tkinter`）；Linux 上可能需要安装 `tk`、`python3-tk` 或 `python3-tkinter`。
- 适用场景：在桌面环境下进行可视化操作，带选项卡视图和实时输出。
- 详见：[[GUI-Guide]]

## TUI

- 路径：`tui/`
- 启动：`cd tui && uv sync && uv run nvoc-tui`
- 适用场景：以终端为主的操作方式，适合远程/SSH 环境。
- 详见：[[TUI-Guide]]

## SRV

- 路径：`srv/`
- 适用场景：在受管环境中提供服务生命周期管理和 localhost HTTP 控制。
- 详见：[[SRV-Guide]]

## 配置与 CLI 发现

各前端应显式解析 `auto-optimizer` 的可执行文件路径/配置，并在设置界面中展示出来，以避免隐藏的路径失败问题。

---

*维护来源：`docs/wiki/Frontends.md`、`gui/README.md`、`tui/README.md`、`srv/README.md`、各前端配置源码。*
