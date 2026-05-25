param(
    [long]$StartMs,
    [long]$EndMs
)

$startDateTime = [DateTimeOffset]::FromUnixTimeMilliseconds($StartMs).LocalDateTime
$endDateTime   = [DateTimeOffset]::FromUnixTimeMilliseconds($EndMs).LocalDateTime

$ids  = @(153, 13, 4101, 10110, 10111)
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
