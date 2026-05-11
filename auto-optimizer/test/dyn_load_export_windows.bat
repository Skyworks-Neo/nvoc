@echo off
for /f %%a in ('echo prompt $E^| cmd') do set "ESC=%%a"

...venv\scripts\python ..\cli-stressor-opencl\test.py --precisions fp32 --matrix-sizes 10240 --duration 45 --platform-index 0 --device-index 0
