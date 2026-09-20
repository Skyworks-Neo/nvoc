# srv/verify_deploy.ps1 - standalone post-deploy acceptance checks for the
# nvoc srv control plane. Complements deploy.ps1's inline health check with a
# full PASS/FAIL report, so "the service is up" is not mistaken for "the
# service is the right build and still behaves correctly".
#
# Verifies against the live install (no elevation required):
#   1. registration : service exists, RUNNING, binPath where deploy.ps1
#                     registered it (the build output dir by default, or the
#                     -InstallDir location; a binPath inside the repo is
#                     flagged as the dangling-registration pattern)
#   2. identity     : /version returns version + git_hash; the service log's
#                     latest startup line agrees with it
#   3. config plane : /config shape
#   4. guards       : CSRF (405 without POST + X-Requested-With) and range
#                     validation (400) on both mutation endpoints, plus one
#                     reversible temp_limit write that is restored afterwards
#                     (test value stays above idle temperature, so the VFP
#                     control loop is never actually triggered)
#
# Usage:
#   .\verify_deploy.ps1                          # verify the live install
#   .\verify_deploy.ps1 -ExpectedGitHash <hash>  # hard-assert embedded hash
#   .\verify_deploy.ps1 -InstallDir X            # same -InstallDir as deploy used
#
# Exit code: 0 = all checks passed, 1 = at least one FAIL.
param(
    [string]$ServiceName = "nvoc_service",
    [string]$BaseUrl = "http://127.0.0.1:14514",
    [string]$InstallDir = "",       # empty = build output dir, matching deploy.ps1's default
    [string]$ExpectedGitHash = ""
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
$LogPath = Join-Path $env:PROGRAMDATA "nvoc\logs\nvoc_service-output.log"
# Resolve the default expected binPath exactly like deploy.ps1 does:
# workspace-env.ps1's CARGO_TARGET_DIR when present, else repo target\.
$EnvFile = Join-Path (Split-Path -Parent $RepoRoot) "workspace-env.ps1"
if (Test-Path $EnvFile) { . $EnvFile }
$CargoTargetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $RepoRoot "target" }
if (-not $InstallDir) { $InstallDir = Join-Path $CargoTargetRoot "release" }
$Pass = 0; $Fail = 0; $Warn = 0

function Check([bool]$Ok, [string]$Label, [string]$FailHint = "") {
    if ($Ok) {
        $script:Pass++
        Write-Host "  [PASS] $Label"
    } else {
        $script:Fail++
        Write-Host "  [FAIL] $Label" -ForegroundColor Red
        if ($FailHint) { Write-Host "         $FailHint" -ForegroundColor DarkGray }
    }
}

function Warn([string]$Label, [string]$Detail = "") {
    $script:Warn++
    Write-Host "  [WARN] $Label" -ForegroundColor Yellow
    if ($Detail) { Write-Host "         $Detail" -ForegroundColor DarkGray }
}

# HTTP helper returning @{ Status; Body } with Status=0 on transport failure.
# Invoke-WebRequest throws on 4xx/5xx in Windows PowerShell 5.1, so use
# HttpClient directly to get clean status codes for negative tests.
Add-Type -AssemblyName System.Net.Http
$Client = New-Object System.Net.Http.HttpClient
$Client.Timeout = [TimeSpan]::FromSeconds(5)

function Invoke-Httpraw([string]$Method, [string]$Uri, [hashtable]$Headers = @{}) {
    $req = New-Object System.Net.Http.HttpRequestMessage(
        (New-Object System.Net.Http.HttpMethod($Method)), $Uri)
    foreach ($k in $Headers.Keys) {
        [void]$req.Headers.TryAddWithoutValidation($k, [string]$Headers[$k])
    }
    try {
        $resp = $Client.SendAsync($req).GetAwaiter().GetResult()
        $body = $resp.Content.ReadAsStringAsync().GetAwaiter().GetResult()
        return @{ Status = [int]$resp.StatusCode; Body = $body }
    } catch {
        # Unwrap AggregateException so transport errors read plainly.
        $msg = if ($_.Exception.InnerException) { $_.Exception.InnerException.Message } else { $_.Exception.Message }
        return @{ Status = 0; Body = $msg }
    }
}

$Csrf = @{ "X-Requested-With" = "XMLHttpRequest" }

# Current HEAD hash, when available: lets us flag a stale deployment without
# failing it (the deployed binary may legitimately predate newer commits).
$headHash = ""
try {
    $gitOut = & git -C $RepoRoot rev-parse --short=12 HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $gitOut) { $headHash = ([string]$gitOut).Trim() }
} catch { }

Write-Host "== verify_deploy: $BaseUrl (service '$ServiceName') =="

# --- 1. service registration ---
$svc = Get-CimInstance Win32_Service -Filter "Name='$ServiceName'"
if (-not $svc) {
    Check $false "service '$ServiceName' is registered" "not found; run srv\deploy.ps1 first"
    Write-Host ""
    Write-Host "== verify summary: $Pass passed, $Fail failed, $Warn warned =="
    exit 1
}
Check ($svc.State -eq "Running") "service state is Running (state: $($svc.State))" `
    "start with: sc.exe start $ServiceName"

$expectedBin = Join-Path $InstallDir "nvoc_service.exe"
$binPath = $svc.PathName.Trim('"')
Check ($binPath -ieq $expectedBin) "binPath = $binPath" "expected $expectedBin (pass the same -InstallDir used at deploy time)"
Check (-not $binPath.StartsWith($RepoRoot, [StringComparison]::OrdinalIgnoreCase)) `
    "binPath is outside the repo" `
    "dangling-registration regression: binPath must not live under $RepoRoot"

# --- 2. build identity (/version) ---
$ver = Invoke-Httpraw "GET" "$BaseUrl/version"
$verJson = $null
if ($ver.Status -eq 200) {
    try { $verJson = $ver.Body | ConvertFrom-Json } catch { $verJson = $null }
    Check ($null -ne $verJson) "/version is valid JSON" "body: $($ver.Body)"
    if ($verJson) {
        Check (-not [string]::IsNullOrWhiteSpace([string]$verJson.version)) `
            "version embedded: $($verJson.version)"
        $h = [string]$verJson.git_hash
        Check ($h -match '^[0-9a-f]{7,40}$') "git_hash embedded: $h" `
            "git_hash missing or 'unknown' - binary built outside a git checkout?"
        if ($ExpectedGitHash) {
            Check ($h -ieq $ExpectedGitHash) "git_hash matches -ExpectedGitHash" `
                "expected $ExpectedGitHash, got $h"
        } elseif ($headHash -and ($headHash -ine $h)) {
            Warn "deployed git_hash $h != current HEAD $headHash" `
                "binary predates the latest commit(s); re-run srv\deploy.ps1 to refresh"
        }
    }
} else {
    $hint = if ($ver.Status -eq 0) { "unreachable: $($ver.Body)" } else { "HTTP $($ver.Status): $($ver.Body)" }
    Check $false "/version responds" $hint
}

# --- 3. startup log agrees with /version ---
# The identity line is written once per service start, so on a long-running
# install it can sit far above any tail window or even rotate out of the
# active log entirely. Scan the active file fully, then rotated siblings as a
# fallback; a missing line is a WARN, a disagreeing line is a FAIL.
function Find-StartupLine {
    $dir = Split-Path -Parent $LogPath
    $name = Split-Path -Leaf $LogPath
    if (-not (Test-Path $dir)) { return $null }
    $files = @(Get-ChildItem $dir -Filter "$name*" -File -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending)
    foreach ($f in $files) {
        $hits = @(Select-String -Path $f.FullName -Pattern 'nvoc_service\s+(\S+)\s+\((\S+)\)\s+starting')
        if ($hits.Count -gt 0) { return $hits[-1] }
    }
    return $null
}

$startLine = Find-StartupLine
if ($null -ne $startLine) {
    $m = $startLine.Matches[0]
    $logVer = $m.Groups[1].Value
    $logHash = $m.Groups[2].Value
    if ($verJson) {
        Check (($logVer -eq [string]$verJson.version) -and ($logHash -eq [string]$verJson.git_hash)) `
            "startup log line agrees with /version ($logVer / $logHash)" `
            "log says $logVer ($logHash), /version says $($verJson.version) ($($verJson.git_hash))"
    }
} else {
    Warn "startup identity line not found in service log" `
        "likely rotated out; /version identity is still verified and the line re-appears at next restart"
}

# --- 4. config plane ---
$cfg = Invoke-Httpraw "GET" "$BaseUrl/config"
$cfgJson = $null
if ($cfg.Status -eq 200) {
    try { $cfgJson = $cfg.Body | ConvertFrom-Json } catch { $cfgJson = $null }
    Check ($null -ne $cfgJson -and $null -ne $cfgJson.vfp_lock_point -and $null -ne $cfgJson.temp_limit) `
        "/config shape (vfp_lock_point=$($cfgJson.vfp_lock_point) temp_limit=$($cfgJson.temp_limit))"
} else {
    Check $false "/config responds" "HTTP $($cfg.Status): $($cfg.Body)"
}

# --- 5. guards on both mutation endpoints ---
# Only 405/400 paths are exercised here; no OC write is ever sent.
$r = Invoke-Httpraw "GET" "$BaseUrl/set_temp_limit_soft_vfp?limit=64"
Check ($r.Status -eq 405) "temp_limit: GET rejected (HTTP $($r.Status))"
$r = Invoke-Httpraw "POST" "$BaseUrl/set_temp_limit_soft_vfp?limit=64"
Check ($r.Status -eq 405) "temp_limit: POST without X-Requested-With rejected (HTTP $($r.Status))"
$r = Invoke-Httpraw "POST" "$BaseUrl/set_temp_limit_soft_vfp?limit=999" $Csrf
Check ($r.Status -eq 400) "temp_limit: out-of-range value rejected (HTTP $($r.Status))"
$r = Invoke-Httpraw "POST" "$BaseUrl/set_temp_limit_soft_vfp" $Csrf
Check ($r.Status -eq 400) "temp_limit: missing parameter rejected (HTTP $($r.Status))"
$r = Invoke-Httpraw "GET" "$BaseUrl/oc_global?oc=1000"
Check ($r.Status -eq 405) "oc_global: GET rejected (HTTP $($r.Status))"
$r = Invoke-Httpraw "POST" "$BaseUrl/oc_global?oc=99999999" $Csrf
Check ($r.Status -eq 400) "oc_global: out-of-range value rejected (HTTP $($r.Status))"
$r = Invoke-Httpraw "POST" "$BaseUrl/oc_global" $Csrf
Check ($r.Status -eq 400) "oc_global: missing parameter rejected (HTTP $($r.Status))"

# --- 6. reversible write: temp_limit set -> reflected -> restored ---
$orig = $null
if ($cfgJson) { $orig = [int]$cfgJson.temp_limit }
if ($null -ne $orig) {
    $test = 65
    if ($orig -eq 65) { $test = 64 }
    try {
        $r = Invoke-Httpraw "POST" "$BaseUrl/set_temp_limit_soft_vfp?limit=$test" $Csrf
        $ok = ($r.Status -eq 200)
        if ($ok) {
            $chk = Invoke-Httpraw "GET" "$BaseUrl/config"
            try { $chkJson = $chk.Body | ConvertFrom-Json } catch { $chkJson = $null }
            $ok = ($chk.Status -eq 200) -and ($null -ne $chkJson) -and ([int]$chkJson.temp_limit -eq $test)
        }
        Check $ok "temp_limit write path: $orig -> $test reflected in /config"
    } finally {
        $r = Invoke-Httpraw "POST" "$BaseUrl/set_temp_limit_soft_vfp?limit=$orig" $Csrf
        if ($r.Status -eq 200) {
            $script:Pass++
            Write-Host "  [PASS] temp_limit restored to $orig"
        } else {
            Check $false "temp_limit restored to $orig" `
                "restore POST returned HTTP $($r.Status); set it back manually"
        }
    }
} else {
    Warn "temp_limit write path not exercised" "no /config baseline available"
}

# --- summary ---
Write-Host ""
Write-Host "== verify summary: $Pass passed, $Fail failed, $Warn warned =="
if ($Fail -gt 0) { exit 1 }
exit 0
