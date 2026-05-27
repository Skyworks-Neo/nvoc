@echo off
for /f %%a in ('echo prompt $E^| cmd') do set "ESC=%%a"

set /a "new_duration=%~2 * 5"

..\target\release\cli-stressor-cuda-rs.exe --config .\test\cli-stressor-cuda-rs.conf --duration %new_duration%
