@echo off
for /f %%a in ('echo prompt $E^| cmd') do set "ESC=%%a"

:: powershell -ExecutionPolicy Unrestricted -Command "Set-ExecutionPolicy Unrestricted -Scope CurrentUser"

if not defined NVOC_AUTO_OPTIMIZER_BIN set "NVOC_AUTO_OPTIMIZER_BIN=..\target\release\nvoc-auto-optimizer.exe"
if not defined NVOC_CLI_BIN set "NVOC_CLI_BIN=..\target\release\nvoc-cli.exe"

"%NVOC_CLI_BIN%" get-info

setlocal enabledelayedexpansion

set "logfile=.\ws\vfp.jsonl"
set "vfptemfile=.\ws\vfp-tem.csv"
set "startpoint=0"

if not exist ".\ws" (
 mkdir ".\ws"
 echo %ESC%[1;92m Folder created: .\ws %ESC%[0m
)
if not exist "%logfile%" (
 echo. > "%logfile%"
 echo %ESC%[1;92m Log file created: %logfile% %ESC%[0m
)

echo Detecting GPUs in system...
"%NVOC_CLI_BIN%" list-gpus
echo.
set /p GPU_ID=Input target GPU id to be scanned:

echo.
echo Selected GPU: %GPU_ID%
echo.

"%NVOC_CLI_BIN%" --gpu=%GPU_ID% reset-pstate-clock-offsets
"%NVOC_AUTO_OPTIMIZER_BIN%" --gpu=%GPU_ID% reset-vfp
"%NVOC_CLI_BIN%" --gpu=%GPU_ID% reset-vfp-lock

if not exist ".\ws\vfp-init.csv" (
  echo exporting default data...
  "%NVOC_AUTO_OPTIMIZER_BIN%" --gpu=%GPU_ID% export-vfp .\ws\vfp-init.csv
)
if "%~1"=="1" (
    :: If para is 1, clear the log file
    copy nul "%logfile%" > nul
    copy nul "%vfptemfile%" > nul
)

echo  =================================================================
echo %ESC%[1;93m ===================DISCLAIMER======================= %ESC%[0m
echo %ESC%[1;91m vfp scan may consistently trig your GPU safe limit and crash... %ESC%[0m
echo %ESC%[1;91m WARNING: SYSTEM HUNG or CRASH IS EXPECTED!!!!!!!!! %ESC%[0m
echo %ESC%[1;96m IF SYSTEM HUNG FOR MORE THAN 3 MIN YOU ARE SUPPOSED TO FORCE REBOOT!!!!!!!! %ESC%[0m
echo %ESC%[1;96m IF THAT OCCURS, FORCE RESTART and RUN THE BAT AGAIN!!!!! %ESC%[0m
echo %ESC%[1;92m The scanner WILL CONTINUE from breakpoint AUTOMATICALLY. %ESC%[0m
echo %ESC%[1;92m This will NOT DAMAGE your GPU, the scan result is SAFE to use. %ESC%[0m
echo %ESC%[1;93m If crash is unacceptable on your current situation, use Ctrl-C to exit scanner. %ESC%[0m

pause

"%NVOC_AUTO_OPTIMIZER_BIN%" --gpu=%GPU_ID% autoscan-vfp
"%NVOC_AUTO_OPTIMIZER_BIN%" --gpu=%GPU_ID% fix-vfp-result -m 1
"%NVOC_AUTO_OPTIMIZER_BIN%" --gpu=%GPU_ID% import-vfp .\ws\vfp.csv
"%NVOC_AUTO_OPTIMIZER_BIN%" --gpu=%GPU_ID% export-vfp .\ws\vfp-final.csv

echo %ESC%[1;92m All VFP Scan Finish Please Close this Window and please check in file ws\vfp-final.csv %ESC%[0m
