@echo off
for /f %%a in ('echo prompt $E^| cmd') do set "ESC=%%a"

..\target\release\cli-stressor-cuda-rs.exe --precisions fp32 --matrix-sizes 10240 --duration 45
