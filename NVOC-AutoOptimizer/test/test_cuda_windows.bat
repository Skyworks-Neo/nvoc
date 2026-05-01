@echo off
for /f %%a in ('echo prompt $E^| cmd') do set "ESC=%%a"

set /a "new_duration=%~2 * 5"

..\NVOC-CLI-Stressor\.venv\scripts\python ..\NVOC-CLI-Stressor\test.py --precisions fp16 --matrix-sizes 2048,4096,8192 --duration %new_duration%
