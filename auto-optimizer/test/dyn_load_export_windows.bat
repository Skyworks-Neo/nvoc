@echo off
for /f %%a in ('echo prompt $E^| cmd') do set "ESC=%%a"

..\target\release\cli-stressor-cuda-rs.exe --config .\test\cli-stressor-cuda-rs-dyn-export.toml
