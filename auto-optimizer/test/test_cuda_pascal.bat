@echo off
set /a "new_duration=%~2 * 5"
..\target\release\cli-stressor-cuda-rs.exe --config .\test\cli-stressor-pascal.toml --duration %new_duration%
