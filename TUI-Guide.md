# TUI 使用指南

NVOC-TUI 是基于 Python Textual 的终端界面前端，使用 `pynvoc` 原生绑定。

## 运行

```bash
cd tui
uv sync
uv run nvoc-tui
```

## 功能

- **Dashboard**：实时轮询 GPU 状态
- **Overclock**：超频与风扇控制操作
- **VF Curve**：静态 VF 曲线导出/导入/编辑，支持终端绘图
- **Output Console**：原生操作输出显示

## 测试

```bash
cd tui
uv run pytest
```

## 打包

```powershell
cd tui
uv sync --group build
uv run pyinstaller --clean --noconfirm nvoc_tui.spec
```

## 注意

> 代码大部分由 CodeX 生成，功能尚在完善中。
