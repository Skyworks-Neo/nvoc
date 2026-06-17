#requires -Version 7.0
<#
.SYNOPSIS
    Prototype: scan multiple GPUs *concurrently* by launching one auto-optimizer
    autoscan process per card, each pinned to its card and with isolated output.

.DESCRIPTION
    The autoscan binary already (a) pins the stressor to one card via
    `--cuda-device` (=> CUDA_VISIBLE_DEVICES) and (b) filters Windows GPU
    FECS/TDR events by the *target* GPU. So running one process per card in
    parallel gives concurrent scanning WITH per-card crash isolation and event
    monitoring, without touching the (sequential) in-process scan loop.

    Each card gets its own `ws\c<idx>\` for --log/--output so the processes do
    not clobber each other. After the scans, each card is reset to stock.

    SAFE BY DEFAULT: without -Execute this only prints the per-card commands
    (dry run). -Execute performs real OVERCLOCK WRITES on the GPUs and requires
    Administrator. Keep concurrency small (power!) and keep -TimeoutLoops low.

.EXAMPLE
    pwsh ./concurrent_autoscan.ps1 -Cards 0,1                 # dry run
    pwsh ./concurrent_autoscan.ps1 -Cards 0,1 -Execute        # real (admin)
#>
[CmdletBinding()]
param(
    # Comma-separated card indices (string, so it survives -File/RunAs arg passing)
    [string]$Cards = '0,1',
    [int]$TimeoutLoops = 1,
    [switch]$Ultrafast,
    [switch]$Execute,
    [bool]$Reset = $true,
    [string]$Repo = 'D:\nvoc-ws\nvoc',
    [string]$DllDir = 'D:\nvoc-ws\nvoc\cuda-runtime',
    [string]$Log = 'D:\nvoc-ws\nvoc\concurrent_autoscan.log',
    # Override the stressor wrapper (default matches autoscan-vfp's own default).
    # Use .\test\test_cuda_pascal.bat for Pascal/Maxwell cards (removes BF16/TF32).
    [string]$TestExe = '.\test\test_cuda_windows.bat'
)
$ErrorActionPreference = 'Stop'
$exe = Join-Path $Repo 'target\release\nvoc-auto-optimizer.exe'
if (-not (Test-Path $exe)) { throw "auto-optimizer not built at $exe" }
Set-Location (Join-Path $Repo 'auto-optimizer')
$env:PATH = "$DllDir;$env:PATH"
$CardList = @($Cards.Split(',') | ForEach-Object { [int]$_.Trim() })

# PCI-bus-sorted index -> { GpuId hex (for --gpu), CUDA ordinal (for --cuda-device) }
# from `nvoc-auto-optimizer list` on this host.
$map = @{
    0 = @{ id = '0x0400'; cuda = 0 }; 1 = @{ id = '0x0500'; cuda = 1 }
    2 = @{ id = '0x0800'; cuda = 2 }; 3 = @{ id = '0x0900'; cuda = 3 }
    4 = @{ id = '0x8600'; cuda = 4 }; 5 = @{ id = '0x8700'; cuda = 5 }
    6 = @{ id = '0x8A00'; cuda = 6 }; 7 = @{ id = '0x8B00'; cuda = 7 }
}

"=== concurrent autoscan  Execute=$Execute  cards=$($CardList -join ',')  t=$TimeoutLoops  ultrafast=$Ultrafast ===" | Tee-Object $Log
$procs = @{}
foreach ($c in $CardList) {
    if (-not $map.ContainsKey($c)) { throw "unknown card index $c" }
    $id = $map[$c].id; $cd = $map[$c].cuda
    $ws = "ws\c$c"; New-Item -ItemType Directory -Force $ws | Out-Null
    $a = @('autoscan-vfp', '--gpu', $id, '--cuda-device', "$cd",
        '-t', "$TimeoutLoops", '-l', "$ws\vfp.jsonl", '-o', "$ws\vfp-tem.csv",
        '-w', $TestExe, '-i', "$ws\vfp-init.csv")
    if ($Ultrafast) { $a += '--ultrafast' }
    "[card $c  gpu=$id  cuda=$cd]  $exe $($a -join ' ')" | Tee-Object $Log -Append
    if ($Execute) {
        # Seed the per-card reference curve the autoscan reads via -i.
        & $exe export-vfp --gpu $id -q "$ws\vfp-init.csv" 2>&1 | Tee-Object $Log -Append
        $procs[$c] = Start-Process -FilePath $exe -PassThru -WindowStyle Hidden `
            -RedirectStandardOutput "$ws\scan.out" -RedirectStandardError "$ws\scan.err" -ArgumentList $a
    }
}

if (-not $Execute) {
    "(dry run — nothing launched; re-run with -Execute as Administrator to scan for real)" | Tee-Object $Log -Append
    return
}

"launched $($procs.Count) concurrent autoscan(s); waiting for all to finish..." | Tee-Object $Log -Append
foreach ($c in $CardList) { $procs[$c].WaitForExit() }
foreach ($c in $CardList) { "[card $c] autoscan exit=$($procs[$c].ExitCode)" | Tee-Object $Log -Append }

if ($Reset) {
    foreach ($c in $CardList) {
        $id = $map[$c].id
        "[card $c] reset-vfp --gpu $id (restore VFP deltas to stock)" | Tee-Object $Log -Append
        & $exe reset-vfp --gpu $id 2>&1 | Tee-Object $Log -Append
    }
}
foreach ($c in $CardList) {
    $id = $map[$c].id
    "[card $c] post-state:" | Tee-Object $Log -Append
    & (Join-Path $Repo 'target\release\nvoc-cli.exe') get-settings --gpu $id 2>&1 |
        Select-String 'Offset|VFP|Lock' | Tee-Object $Log -Append
}
"=== DONE ===" | Tee-Object $Log -Append
