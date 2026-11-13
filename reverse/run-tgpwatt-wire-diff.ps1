# 对照机(30 系,毒路径可用)抓包:身份/版本 + GET/SET 的 ioctl 序列 + 毒写入
# 真正改动的映射节字节。必须提权 —— 否则写路径会在下发 ioctl 前以
# InvalidUserPrivilege 返回,抓到的只有 GET。
#
# 用法(管理员 PowerShell 7):
#   pwsh -File reverse\run-tgpwatt-wire-diff.ps1
# 产物在 %TEMP%\nvoc-esc-capture\,重点回传 SUMMARY.txt 与 changed-*.txt。
#Requires -RunAsAdministrator
chcp 65001 > $null

$root = Split-Path $PSScriptRoot -Parent
$dir = Join-Path $env:TEMP "nvoc-esc-capture"
Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$env:NVOC_ESC_CAPTURE = $dir

$log = Join-Path $dir "run.log"
Push-Location (Join-Path $root "nvapi-rs")
cargo test -p nvapi --test esc_capture tgpwatt_wire_diff -- --ignored --nocapture *> $log
$code = $LASTEXITCODE
Pop-Location

"exit=$code" | Add-Content $log
Write-Host "exit=$code"
Write-Host "capture -> $dir"
Write-Host "回传这个文件即可: $(Join-Path $dir 'SUMMARY.txt')"
