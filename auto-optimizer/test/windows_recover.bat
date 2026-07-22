@echo off
if not defined NVOC_AUTO_OPTIMIZER_BIN set "NVOC_AUTO_OPTIMIZER_BIN=..\target\release\nvoc-auto-optimizer.exe"
if not defined NVOC_CLI_BIN set "NVOC_CLI_BIN=..\target\release\nvoc-cli.exe"

if "%~1"=="" (
    echo Detecting GPUs in system...
    "%NVOC_CLI_BIN%" list-gpus
    echo.
    set /p GPU_ID=Input target GPU id to recover:
) else (
    set "GPU_ID=%~1"
)

"%NVOC_CLI_BIN%" --gpu=%GPU_ID% get-uuid > "%TEMP%\nvoc_uuid.tmp" 2>&1
for /f "usebackq tokens=*" %%a in (`findstr /r /i "[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-" "%TEMP%\nvoc_uuid.tmp"`) do set "UUID=%%a"
del "%TEMP%\nvoc_uuid.tmp" 2>NUL

if "%UUID%"=="" (
    echo ERROR: Failed to resolve GPU UUID. Aborting.
    exit /b 1
)

set "WSDIR=.\GPUScan-%UUID%"

if not exist "%WSDIR%" (
    mkdir "%WSDIR%"
)

"%NVOC_CLI_BIN%" --gpu=%GPU_ID% reset-core-offset-mhz
"%NVOC_CLI_BIN%" --gpu=%GPU_ID% reset-memory-offset-mhz
"%NVOC_AUTO_OPTIMIZER_BIN%" --gpu=%GPU_ID% import-vfp "%WSDIR%\vfp-init.csv"
