@echo off
for /f %%a in ('echo prompt $E^| cmd') do set "ESC=%%a"

set /a "new_duration=%~2 * 5"

..\cli-stressor-cuda\.venv\scripts\python ..\cli-stressor-cuda\test.py --precisions fp16 --matrix-sizes 2048,4096,8192 --duration %new_duration%
