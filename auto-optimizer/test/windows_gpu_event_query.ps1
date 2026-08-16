param(
    [long]$StartMs,
    [long]$EndMs
)

# 强制重定向下输出为 UTF-8（默认是 OEM 码页 = 中文系统 GBK，事件文本含中文
# 会被 Rust 侧的 UTF-8 解码打碎）。脚本执行前 PowerShell 自身的报错仍可能是
# GBK，由调用侧 decode_console_output 的 OEM 回退兜底。
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8

$startDateTime = [DateTimeOffset]::FromUnixTimeMilliseconds($StartMs).LocalDateTime
$endDateTime   = [DateTimeOffset]::FromUnixTimeMilliseconds($EndMs).LocalDateTime

$ids  = @(153, 13, 14, 4101, 10110, 10111)
$logs = @('System', 'Microsoft-Windows-DriverFrameworks-UserMode/Operational')

foreach ($id in $ids) {
    foreach ($log in $logs) {
        try {
            Get-WinEvent -FilterHashtable @{
                LogName   = $log
                Id        = $id
                StartTime = $startDateTime
                EndTime   = $endDateTime
            } -ErrorAction Stop | ForEach-Object {
                $xml = $_.ToXml()
                # $_.Message needs the provider's message DLL; when missing it
                # returns a generic fallback that lacks the real error text.
                # We match against the raw XML instead.
                $gpuBusId = ''
                if ($xml -match 'GPUID:\s*(\d+)') { $gpuBusId = $Matches[1] }
                $fecs = 0; $tdr = 0
                if ($xml -match 'FECS')                { $fecs = 1 }
                if ($xml -match 'Restarting TDR|Reset TDR') { $tdr = 1 }
                # Truncated message for logging – prefer real text, fallback to XML
                try   { $msg = $_.Message } catch { $msg = '' }
                if ($msg.Length -eq 0) { $msg = $xml }
                $shortMsg = $msg.Substring(0, [Math]::Min(256, $msg.Length))
                Write-Output ($_.Id.ToString() + '|' + $gpuBusId + '|' +
                              $fecs.ToString() + '|' + $tdr.ToString() + '|' + $shortMsg)
            }
        } catch {}
    }
}
