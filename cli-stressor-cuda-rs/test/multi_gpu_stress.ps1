#requires -Version 7.0
<#
.SYNOPSIS
    Multi-GPU correctness + throughput/power reference test for cli-stressor-cuda-rs.

.DESCRIPTION
    Runs a single-card baseline on the first selected GPU, then runs every
    selected GPU concurrently (one stressor process pinned per card), and
    reports:
      * per-card TFLOPS, validation failures, and process exit code
      * concurrent-vs-solo per-card throughput (a CPU-oversubscription check:
        the parallel host-side input generation must not starve concurrent
        instances)
      * peak CPU load and total GPU power drawn during the concurrent run

    PASS criteria (the script exits non-zero if any fail):
      1. every instance exits 0 with val_fail == 0 (no silent corruption), and
      2. concurrent per-card throughput stays at or above -MinConcurrentRatio
         of the solo baseline (default 0.85), i.e. no oversubscription collapse.

    This is a manual hardware reference test, not a CI unit test: it needs real
    NVIDIA GPUs and a CUDA-feature build of the stressor.

.PARAMETER Exe
    Path to cli-stressor-cuda-rs(.exe). Defaults to the release build next to
    this repo (target/release/cli-stressor-cuda-rs[.exe]).

.PARAMETER Gpus
    GPU indices (PCI-bus-sorted, as shown by `--list-gpus`) to test.
    Defaults to every CUDA device the binary enumerates.

.PARAMETER DllDir
    Optional directory prepended to PATH so the CUDA 12.x runtime DLLs
    (nvrtc64_120_0 / cublas64_12 / cublasLt64_12 / cudart64_12) are found.
    Not needed if those DLLs already sit next to the exe or on PATH.

.PARAMETER Duration
    Per-run stress duration in seconds (default 30).

.EXAMPLE
    pwsh ./multi_gpu_stress.ps1                         # all cards, defaults
    pwsh ./multi_gpu_stress.ps1 -Gpus 0,1,2,3 -Duration 60
    pwsh ./multi_gpu_stress.ps1 -DllDir ..\..\cuda-runtime
#>
[CmdletBinding()]
param(
    [string]$Exe,
    [int[]]$Gpus,
    [string]$DllDir,
    [double]$Duration = 30,
    [string]$Precisions = 'fp32',
    [string]$MatrixSizes = '4096',
    [int]$BurstIters = 16,
    [string]$StreamMode = 'single',
    [double]$ValidateInterval = 6,
    [double]$MinConcurrentRatio = 0.85
)

$ErrorActionPreference = 'Stop'

# --- resolve the stressor binary -------------------------------------------
if (-not $Exe) {
    $root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)  # repo root
    $cand = @(
        (Join-Path $root 'target/release/cli-stressor-cuda-rs.exe'),
        (Join-Path $root 'target/release/cli-stressor-cuda-rs')
    )
    $Exe = $cand | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $Exe -or -not (Test-Path $Exe)) {
    throw "stressor binary not found. Build it first: cargo build --release -p cli-stressor-cuda-rs --features cuda  (or pass -Exe)"
}
if ($DllDir) { $env:PATH = "$DllDir;$env:PATH" }

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("nvoc-mgpu-" + [System.IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Force $work | Out-Null

$commonArgs = @(
    '--duration', "$Duration",
    '--precisions', $Precisions,
    '--matrix-sizes', $MatrixSizes,
    '--stream-mode', $StreamMode,
    '--burst-iters', "$BurstIters",
    '--minor-mixture-rate', '0',
    '--kernel-types', 'gemm',
    '--validate-interval', "$ValidateInterval"
)

function Get-Metric([string]$log, [string]$pattern) {
    $m = Select-String -Path $log -Pattern $pattern | Select-Object -First 1
    if ($m) { $m.Matches[0].Groups[1].Value } else { $null }
}
function Get-Tflops([string]$log) { $v = Get-Metric $log '([\d.]+)\s+TFLOPS'; if ($v) { [double]$v } else { 0 } }
function Get-ValFail([string]$log) { $v = Get-Metric $log 'val_fail=\s*(\d+)'; if ($null -ne $v) { [int]$v } else { -1 } }

function Invoke-Stressor([int]$gpu) {
    $out = Join-Path $work "gpu$gpu.out"
    $err = Join-Path $work "gpu$gpu.err"
    Start-Process -FilePath $Exe -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput $out -RedirectStandardError $err `
        -ArgumentList (@('--gpu-index', "$gpu") + $commonArgs)
}

# --- discover GPUs ----------------------------------------------------------
if (-not $Gpus -or $Gpus.Count -eq 0) {
    $list = & $Exe --list-gpus 2>&1
    $Gpus = $list | ForEach-Object {
        if ($_ -match '^\s*(\d+)\s+\d+\s+\d{4}:') { [int]$Matches[1] }
    } | Sort-Object -Unique
    if (-not $Gpus) { throw "could not enumerate GPUs from --list-gpus output" }
}
Write-Host ("Stressor : {0}" -f $Exe)
Write-Host ("GPUs     : {0}" -f ($Gpus -join ', '))
Write-Host ("Config   : --precisions {0} --matrix-sizes {1} --stream-mode {2} --burst-iters {3} --duration {4}s" -f $Precisions, $MatrixSizes, $StreamMode, $BurstIters, $Duration)
Write-Host ""

# --- phase 1: solo baseline on the first GPU --------------------------------
$first = $Gpus[0]
Write-Host "== Phase 1: solo baseline on GPU $first =="
$p = Invoke-Stressor $first
$p.WaitForExit()
$soloLog = Join-Path $work "gpu$first.out"
$soloTf = Get-Tflops $soloLog
$soloVf = Get-ValFail $soloLog
Write-Host ("  GPU{0}: {1} TFLOPS  val_fail={2}  exit={3}" -f $first, $soloTf, $soloVf, $p.ExitCode)
Write-Host ""

# --- phase 2: all selected GPUs concurrently --------------------------------
Write-Host ("== Phase 2: {0} card(s) concurrent ==" -f $Gpus.Count)
$procs = @{}
foreach ($g in $Gpus) { $procs[$g] = Invoke-Stressor $g }

# sample CPU + GPU power roughly mid-run
Start-Sleep -Seconds ([Math]::Max(6, [int]($Duration / 2)))
$totalPower = 0.0
try {
    $smi = nvidia-smi --query-gpu=index,power.draw --format=csv,noheader,nounits 2>$null
    foreach ($line in ($smi -split "`n")) {
        if ($line -match '^\s*(\d+)\s*,\s*([\d.]+)') {
            if ([int]$Matches[1] -in $Gpus) { $totalPower += [double]$Matches[2] }
        }
    }
} catch { $totalPower = $null }
$cpu = $null
try { $cpu = (Get-CimInstance Win32_Processor | Measure-Object -Property LoadPercentage -Average).Average } catch {}

foreach ($g in $Gpus) { $procs[$g].WaitForExit() }

# --- collect + verdict ------------------------------------------------------
Write-Host ""
Write-Host "Per-card results (concurrent):"
$rows = @()
$allCorrect = $true
$noCollapse = $true
$totalTf = 0.0
foreach ($g in $Gpus) {
    $log = Join-Path $work "gpu$g.out"
    $tf = Get-Tflops $log
    $vf = Get-ValFail $log
    $ec = $procs[$g].ExitCode
    $totalTf += $tf
    if ($ec -ne 0 -or $vf -ne 0) { $allCorrect = $false }
    Write-Host ("  GPU{0}: {1,7} TFLOPS  val_fail={2}  exit={3}" -f $g, $tf, $vf, $ec)
    $rows += [pscustomobject]@{ Gpu = $g; Tflops = $tf; ValFail = $vf; Exit = $ec }
}

# The first GPU ran both phases; $soloTf was captured before the concurrent
# run overwrote its log, so compare the captured solo value to its concurrent
# throughput (from $rows) to detect oversubscription.
$firstConcurrent = ($rows | Where-Object { $_.Gpu -eq $first }).Tflops
$ratio = if ($soloTf -gt 0) { [math]::Round($firstConcurrent / $soloTf, 3) } else { 0 }
if ($soloTf -gt 0 -and $ratio -lt $MinConcurrentRatio) { $noCollapse = $false }

Write-Host ""
Write-Host "Summary:"
Write-Host ("  total throughput   : {0} TFLOPS across {1} card(s)" -f [math]::Round($totalTf, 1), $Gpus.Count)
Write-Host ("  GPU{0} solo->concurrent: {1} -> {2} TFLOPS ({3}%)" -f $first, $soloTf, $firstConcurrent, [math]::Round($ratio * 100))
if ($null -ne $cpu) { Write-Host ("  CPU load (mid-run) : {0}%" -f $cpu) }
if ($null -ne $totalPower) { Write-Host ("  total GPU power     : {0} W (single mid-run sample)" -f [math]::Round($totalPower)) }

Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue

Write-Host ""
if ($allCorrect -and $noCollapse) {
    Write-Host "RESULT: PASS (all instances val_fail=0/exit=0; no throughput collapse)" -ForegroundColor Green
    exit 0
} else {
    if (-not $allCorrect) { Write-Host "RESULT: FAIL - an instance reported val_fail!=0 or a non-zero exit." -ForegroundColor Red }
    if (-not $noCollapse) { Write-Host ("RESULT: FAIL - concurrent throughput {0}% < {1}% of solo (possible CPU oversubscription)." -f [math]::Round($ratio * 100), [math]::Round($MinConcurrentRatio * 100)) -ForegroundColor Red }
    exit 1
}
