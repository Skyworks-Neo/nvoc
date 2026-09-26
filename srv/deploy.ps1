# srv/deploy.ps1 - build srv + cli, then (re)install the nvoc_service Windows
# service from the build output.
#
# Default install location is the build output directory itself - the same
# behavior as manually running install_service.exe, which registers the
# nvoc_service.exe next to itself (srv/src/bin/install_service.rs). Pass
# -InstallDir to register from a stable out-of-repo directory instead: a
# registration pointing into the build output dangles as soon as that
# directory is deleted/moved (cargo clean, workspace cleanup) - exactly how
# the old dangling "nvoc_service" registration happened.
#
# Usage (elevated PowerShell):
#   .\deploy.ps1                    # build + stop + install/start + health check
#   .\deploy.ps1 -InstallDir X      # stage exes into X and register from there
#   .\deploy.ps1 -NoStart           # stage and install but leave the service stopped
#   .\deploy.ps1 -SkipBuild         # reuse existing release binaries
param(
    [string]$InstallDir = "",       # empty = build output dir (main's original behavior)
    [switch]$NoStart,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$SvcName = "nvoc_service"
$ControlUrl = "http://127.0.0.1:14514"
$RepoRoot = Split-Path -Parent $PSScriptRoot            # <repo>\nvoc
$WorkspaceRoot = Split-Path -Parent $RepoRoot           # D:\08-skyworks\nvoc-srv
# Service SCM state codes (locale-independent): 1=STOPPED 2=START_PENDING
# 3=STOP_PENDING 4=RUNNING. sc.exe output is localized on non-English systems.
$SvcStopped = 1
$SvcRunning = 4

function Get-ServiceStateCode([string]$Name) {
    # Returns "absent" or the numeric SCM state.
    $out = & sc.exe query $Name 2>$null
    if ($LASTEXITCODE -eq 1060) { return "absent" }
    if ($LASTEXITCODE -ne 0) { throw "sc.exe query $Name failed with exit code $LASTEXITCODE" }
    $line = ($out | Select-String "STATE").ToString()
    return [int]($line -replace ".*:\s*(\d+).*", '$1')
}

function Wait-ServiceState([string]$Name, [int]$Want, [int]$TimeoutSec = 30) {
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 500
        if ((Get-ServiceStateCode $Name) -eq $Want) { return $true }
    }
    return $false
}

# --- environment: repo-documented local toolchain (rustup 1.95 install is
# damaged on this machine; use the stable 1.97.1 binaries directly). Dot-source
# workspace-env.ps1 (out of repo) for .deps paths and CARGO_INCREMENTAL=0.
$EnvFile = Join-Path $WorkspaceRoot "workspace-env.ps1"
if (Test-Path $EnvFile) { . $EnvFile }
$rustBin = "C:\Users\Chebeilsea\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin"
$env:RUSTC = "$rustBin\rustc.exe"
$env:RUSTDOC = "$rustBin\rustdoc.exe"
$env:PATH = "$rustBin;$env:PATH"
# workspace-env.ps1 points CARGO_TARGET_DIR at the workspace-local .deps\target;
# resolve the release dir from it instead of assuming repo-local target\.
$CargoTargetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $RepoRoot "target" }
$TargetDir = Join-Path $CargoTargetRoot "release"
# Default install location: the build output itself (main's original flow).
if (-not $InstallDir) { $InstallDir = $TargetDir }

if (-not $SkipBuild) {
    Write-Host "== cargo build (offline, release): nvoc-srv + nvoc-cli =="
    & "$rustBin\cargo.exe" build --offline --release -p nvoc-srv -p nvoc-cli
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}

# --- admin required for every service operation below.
net session > $null 2>&1
if ($LASTEXITCODE -ne 0) {
    throw "Administrator privileges required (service stop/install/start). Run from an elevated PowerShell."
}

# --- stop the service before replacing its exe (Windows locks the image).
$state = Get-ServiceStateCode $SvcName
if ($state -ne "absent" -and $state -ne $SvcStopped) {
    Write-Host "== stopping $SvcName (state $state) =="
    & sc.exe stop $SvcName | Out-Null
    if (-not (Wait-ServiceState $SvcName $SvcStopped 30)) {
        throw "$SvcName did not reach STOPPED within 30 s"
    }
}

# --- stage binaries into the install dir (no-op when it is the build output).
if ($InstallDir -ine $TargetDir) {
    Write-Host "== staging into $InstallDir =="
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $Exes = @(
        "nvoc_service.exe", "install_service.exe", "uninstall_service.exe",
        "notify_service.exe", "pause_continue.exe", "service_config.exe",
        "service_failure_actions.exe", "nvoc-cli.exe"
    )
    foreach ($exe in $Exes) {
        $src = Join-Path $TargetDir $exe
        if (-not (Test-Path $src)) { throw "missing build output: $src (run without -SkipBuild)" }
        Copy-Item -Force -Path $src -Destination (Join-Path $InstallDir $exe)
    }
} else {
    Write-Host "== install dir is the build output; no staging copy =="
    foreach ($exe in @("nvoc_service.exe", "install_service.exe")) {
        if (-not (Test-Path (Join-Path $TargetDir $exe))) {
            throw "missing build output: $(Join-Path $TargetDir $exe) (run without -SkipBuild)"
        }
    }
}

# --- install, or re-register when an existing registration points elsewhere.
$state = Get-ServiceStateCode $SvcName
if ($state -eq "absent") {
    Write-Host "== installing service from $InstallDir =="
    & (Join-Path $InstallDir "install_service.exe")
    if ($LASTEXITCODE -ne 0) { throw "install_service.exe failed" }
} else {
    $binPath = (Get-CimInstance Win32_Service -Filter "Name='$SvcName'").PathName.Trim('"')
    $wantBin = Join-Path $InstallDir "nvoc_service.exe"
    if ($binPath -ine $wantBin) {
        Write-Host "== re-registering service: $binPath -> $wantBin =="
        # The service is already stopped at this point (see above); drop the
        # old registration and recreate it from the current install location.
        & sc.exe delete $SvcName | Out-Null
        Start-Sleep -Milliseconds 500
        & (Join-Path $InstallDir "install_service.exe")
        if ($LASTEXITCODE -ne 0) { throw "install_service.exe failed (re-register)" }
    }
}

# --- start and verify.
if (-not $NoStart) {
    Write-Host "== starting $SvcName =="
    & sc.exe start $SvcName | Out-Null
    if (-not (Wait-ServiceState $SvcName $SvcRunning 30)) {
        $log = Join-Path $env:PROGRAMDATA "nvoc\logs\nvoc_service-output.log"
        throw "$SvcName did not reach RUNNING within 30 s; check $log"
    }

    Write-Host "== health check =="
    $version = $null
    foreach ($i in 1..10) {
        try {
            $version = Invoke-RestMethod -Uri "$ControlUrl/version" -TimeoutSec 2
            break
        } catch {
            Start-Sleep -Milliseconds 500
        }
    }
    if ($version) {
        $config = Invoke-RestMethod -Uri "$ControlUrl/config" -TimeoutSec 2
        Write-Host ("  /version : {0} ({1})" -f $version.version, $version.git_hash)
        Write-Host ("  /config  : vfp_lock_point={0} temp_limit={1}" -f $config.vfp_lock_point, $config.temp_limit)
    } else {
        $log = Join-Path $env:PROGRAMDATA "nvoc\logs\nvoc_service-output.log"
        Write-Warning "no response from $ControlUrl/version; check $log"
    }
}

# --- summary.
$state = Get-ServiceStateCode $SvcName
Write-Host ""
Write-Host "== deploy summary =="
Write-Host ("  install dir : {0}" -f $InstallDir)
Write-Host ("  service     : {0} (state {1})" -f $SvcName, $state)
Write-Host ("  build       : nvoc-srv {0}" -f (Get-Item (Join-Path $InstallDir "nvoc_service.exe")).VersionInfo.ProductVersion)
