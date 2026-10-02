@echo off
setlocal
rem nvoc fresh-machine bootstrap: install rustup / uv when missing, verify the
rem MSVC linker prerequisite, then hand off to `cargo xtask setup`.
rem Extra arguments are forwarded, e.g.:  setup.cmd --dry-run
rem `setup.cmd --msys2 [llvm|gcc] [args...]` additionally bootstraps an MSYS2
rem toolchain environment before the standard flow: llvm = clang64 (gnullvm
rem face, default, verified), gcc = ucrt64 (windows-gnu face, same UCRT runtime
rem so pynvoc keeps linking against the MSVC CPython).

set "MSYS2_REQUEST="
set "MSYS2_FLAVOR=llvm"
set "FORWARD="
:parse_args
if "%~1"=="" goto :args_done
if /i "%~1"=="--msys2" (
    set "MSYS2_REQUEST=1"
    if /i "%~2"=="gcc" (
        set "MSYS2_FLAVOR=gcc"
        shift
    ) else if /i "%~2"=="llvm" (
        set "MSYS2_FLAVOR=llvm"
        shift
    )
) else (
    call :forward "%~1"
)
shift
goto :parse_args
:args_done
if defined MSYS2_REQUEST (
    call :msys2_bootstrap
    if errorlevel 1 exit /b 1
)

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
rem Minimal set for the Rust msvc target: compiler/linker plus a Windows SDK
rem (UCRT + Win32 import libraries). The compilers are *recommended* members of
rem the VCTools workload, so an explicit component list beats the workload +
rem --includeRecommended combo, which would also pull CMake tools we never use.
rem Skipped under --msys2: the gnullvm face links through the msys2 clang/lld
rem toolchain and needs no Visual Studio components.
if defined MSYS2_REQUEST (
    echo [bootstrap] MSVC Build Tools not found - skipped, the --msys2 gnullvm face builds without them.
    goto :check_uv
)
echo [bootstrap] MSVC C++ Build Tools are required to build Rust binaries.
echo [bootstrap] Install them with:
echo     winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.26100"
echo [bootstrap] then re-run this script.
exit /b 1

:check_uv
where uv >nul 2>nul
if errorlevel 1 (
    echo [bootstrap] uv not found - installing via winget...
    winget install --id astral-sh.uv -e --accept-source-agreements --accept-package-agreements
    set "PATH=%LOCALAPPDATA%\Microsoft\WinGet\Links;%PATH%"
)

rem nvapi-rs is a path dependency of nvoc-core, so `cargo run -p xtask` cannot
rem even parse the workspace manifest without it - yet the submodule bootstrap
rem is itself a step inside `cargo xtask setup`. Break the chicken-and-egg by
rem checking it out here, before the first cargo invocation.
if not exist nvapi-rs\Cargo.toml (
    echo [bootstrap] nvapi-rs submodule missing - initializing...
    git submodule update --init nvapi-rs
    if errorlevel 1 exit /b 1
)

cargo run --quiet -p xtask -- setup %FORWARD%
goto :eof

:forward
if defined FORWARD (set "FORWARD=%FORWARD% %~1") else set "FORWARD=%~1"
goto :eof

:msys2_bootstrap
rem MSYS2 winget package (verified: `winget search --id MSYS2.MSYS2 -e`).
set "MSYS2_ROOT=%SYSTEMDRIVE%\msys64"
if exist "%MSYS2_ROOT%\usr\bin\bash.exe" goto :msys2_pacman
echo [bootstrap] MSYS2 not found - installing via winget...
winget install --id MSYS2.MSYS2 -e --accept-source-agreements --accept-package-agreements
if errorlevel 1 (
    echo [bootstrap] winget install failed - install MSYS2 from https://www.msys2.org and re-run this script.
    exit /b 1
)

:msys2_pacman
rem First-run keyring init, then the standard double -Syu: an msys2-runtime
rem update replaces files a running shell holds open, so the first pass stages
rem it and the second pass finishes. Every step is idempotent on an up-to-date
rem tree, so re-running setup.cmd --msys2 is safe.
echo [bootstrap] MSYS2: keyring init + first pacman -Syu pass...
"%MSYS2_ROOT%\usr\bin\bash.exe" -lc "pacman-key --init 2>/dev/null; pacman-key --populate msys2; pacman -Syu --noconfirm"
echo [bootstrap] MSYS2: second pacman pass (completes a staged runtime update)...
"%MSYS2_ROOT%\usr\bin\bash.exe" -lc "pacman -Su --noconfirm"

rem Toolchain selection: llvm = clang64 env (gnullvm face, default, verified);
rem gcc = ucrt64 env (windows-gnu face; ucrt64 over mingw64 because UCRT is the
rem same runtime uv's MSVC CPython uses, which is what lets pynvoc link).
if "%MSYS2_FLAVOR%"=="gcc" (
    set "MSYS2_ENV=ucrt64"
    set "MSYS2_PKGS=mingw-w64-ucrt-x86_64-toolchain mingw-w64-ucrt-x86_64-rust"
) else (
    set "MSYS2_ENV=clang64"
    set "MSYS2_PKGS=mingw-w64-clang-x86_64-toolchain mingw-w64-clang-x86_64-rust"
)
echo [bootstrap] MSYS2: installing the %MSYS2_ENV% toolchain (%MSYS2_FLAVOR%)...
"%MSYS2_ROOT%\usr\bin\bash.exe" -lc "pacman -S --needed --noconfirm %MSYS2_PKGS%"
if errorlevel 1 (
    echo [bootstrap] pacman failed to install the %MSYS2_ENV% toolchain - see the output above.
    exit /b 1
)

echo [bootstrap] MSYS2 %MSYS2_ENV% ready (%MSYS2_FLAVOR% toolchain).
echo [bootstrap]   shell:      %MSYS2_ROOT%\msys2_shell.cmd -%MSYS2_ENV% -use-full-path
echo [bootstrap]   (-use-full-path keeps uv and the rustup cargo visible inside msys2)
echo [bootstrap]   the selected face is verified by xtask; the msvc face stays the default.
goto :eof
