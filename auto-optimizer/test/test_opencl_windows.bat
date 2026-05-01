@echo off
for /f %%a in ('echo prompt $E^| cmd') do set "ESC=%%a"

set /a "new_duration=%~2 * 5"

..\cli-stressor-opencl\.venv\scripts\python ..\cli-stressor-opencl\test.py --precisions fp32 --matrix-sizes 2048,4096,8192 --duration %new_duration% --platform-index 0 --device-index 0
