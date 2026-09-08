@echo off
setlocal
rem nvoc fresh-machine bootstrap: install rustup / uv when missing, verify the
rem MSVC linker prerequisite, then hand off to `cargo xtask setup`.
rem Extra arguments are forwarded, e.g.:  setup.cmd --dry-run

where cargo >nul 2>nul
if errorlevel 1 goto :install_rustup
goto :check_msvc

:install_rustup
echo [bootstrap] cargo not found - installing rustup via winget...
winget install --id Rustlang.Rustup -e --accept-source-agreements --accept-package-agreements
if errorlevel 1 (
    echo [bootstrap] winget install failed - install rustup from https://rustup.rs and re-run this script.
    exit /b 1
)
set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

:check_msvc
rem A fresh machine lacks link.exe, which would make `cargo run` fail before
rem the xtask doctor gets a chance to explain; probe and guide first.
set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" goto :msvc_missing
"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>nul | findstr . >nul
if errorlevel 1 goto :msvc_missing
goto :check_uv

:msvc_missing
echo [bootstrap] MSVC C++ Build Tools are required to build Rust binaries.
echo [bootstrap] Install them with:
echo     winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
echo [bootstrap] then re-run this script.
exit /b 1

:check_uv
where uv >nul 2>nul
if errorlevel 1 (
    echo [bootstrap] uv not found - installing via winget...
    winget install --id astral-sh.uv -e --accept-source-agreements --accept-package-agreements
    set "PATH=%LOCALAPPDATA%\Microsoft\WinGet\Links;%PATH%"
)

cargo run --quiet -p xtask -- setup %*
