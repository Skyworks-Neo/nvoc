param(
    [int]$TargetPciBus = -1
)

function Toggle-DeviceStateByHardwareID {
    param (
        [string]$HardwareID
    )

    $device = Get-PnpDevice | Where-Object { $_.PNPDeviceID -like "*$HardwareID*" }

    if ($device -eq $null) {
        Write-Host "No device found with Hardware ID: $HardwareID."
        return
    }

    foreach ($dev in $device) {
        if ($dev.Status -eq "OK") {
            $dev | Disable-PnpDevice -Confirm:$false
            Write-Host "Device '$($dev.FriendlyName)' with Hardware ID: $HardwareID is now disabled."
        } elseif ($dev.Status -eq "Error") {
            $dev | Enable-PnpDevice -Confirm:$false
            Write-Host "Device '$($dev.FriendlyName)' with Hardware ID: $HardwareID is now enabled."
        } else {
            Write-Host "Device '$($dev.FriendlyName)' with Hardware ID: $HardwareID is in an unknown state ($($dev.Status))."
            $dev | Enable-PnpDevice -Confirm:$false -ErrorAction SilentlyContinue
        }
    }
}

if ($TargetPciBus -ge 0) {
    $nvidiaGpus = Get-PnpDevice | Where-Object {
        $_.Class -eq 'Display' -and $_.FriendlyName -like '*NVIDIA*' -and $_.Present
    }

    if ($nvidiaGpus -eq $null) {
        Write-Host "No NVIDIA GPU found."
        exit 1
    }

    $targetDev = $null
    foreach ($gpu in $nvidiaGpus) {
        try {
            $busNum = (Get-PnpDeviceProperty -InstanceId $gpu.InstanceId -KeyName 'DEVPKEY_Device_BusNumber').Data
            if ($busNum -eq $TargetPciBus) {
                $targetDev = $gpu
                break
            }
        } catch {
            # Some devices may not expose DEVPKEY_Device_BusNumber
        }
        # Fallback: parse location info string "PCI bus 1, device 0, function 0"
        try {
            $loc = (Get-PnpDeviceProperty -InstanceId $gpu.InstanceId -KeyName 'DEVPKEY_Device_LocationInfo').Data
            if ($loc -match 'bus\s+(\d+)') {
                if ([int]$Matches[1] -eq $TargetPciBus) {
                    $targetDev = $gpu
                    break
                }
            }
        } catch {}
    }

    if ($targetDev -eq $null) {
        Write-Host "No NVIDIA GPU found at PCI bus $TargetPciBus"
        Write-Host "Available NVIDIA GPUs:"
        foreach ($g in $nvidiaGpus) { Write-Host "  $($g.FriendlyName)" }
        exit 1
    }

    Write-Host "Targeting GPU at PCI bus $TargetPciBus : $($targetDev.FriendlyName)"
    Write-Host "PNPDeviceID: $($targetDev.InstanceId)"

    # Disable
    $targetDev | Disable-PnpDevice -Confirm:$false
    Write-Host "Device '$($targetDev.FriendlyName)' is now disabled."
    Start-Sleep -Seconds 5
    # Enable
    $targetDev | Enable-PnpDevice -Confirm:$false
    Write-Host "Device '$($targetDev.FriendlyName)' is now enabled."
} else {
    # Legacy: toggle all NVIDIA GPUs
    $gpus = Get-WmiObject Win32_VideoController | Where-Object { $_.Description -like "*NVIDIA*" }
    if ($gpus -eq $null) {
        Write-Host "No NVIDIA GPU found."
        exit 1
    }
    foreach ($gpu in $gpus) {
        Write-Host $gpu.Description
        Write-Host $gpu.PNPDeviceID
        Toggle-DeviceStateByHardwareID -HardwareID $gpu.PNPDeviceID
    }
}
