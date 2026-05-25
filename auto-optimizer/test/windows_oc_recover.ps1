# Self-elevating PowerShell script to toggle the enabled state of a device
# Function to toggle the device state by Hardware ID
# TODO: Multi-GPU Choosing
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
# Device is enabled, so disable it
$dev | Disable-PnpDevice -Confirm:$false
Write-Host "Device '$($dev.FriendlyName)' with Hardware ID: $HardwareID is now disabled."
} elseif ($dev.Status -eq "Error") {
# Device is disabled, so enable it
$dev | Enable-PnpDevice -Confirm:$false
Write-Host "Device '$($dev.FriendlyName)' with Hardware ID: $HardwareID is now enabled."
} else {
Write-Host "Device '$($dev.FriendlyName)' with Hardware ID: $HardwareID is in an unknown state."
}
}
}

# Toggle the devices using their Hardware IDs
#Toggle-DeviceStateByHardwareID -HardwareID "USB\VID_1532&PID_0208&MI_01&JS_02"
#Toggle-DeviceStateByHardwareID -HardwareID "HID\VID_1532&PID_0208&REV_0200&MI_02"

foreach($gpu in Get-WmiObject Win32_VideoController | Where-Object { $_.Description -like "*NVIDIA*" })
{
  Write-Host $gpu.Description
  Write-Host $gpu.PNPDeviceID
  Toggle-DeviceStateByHardwareID -HardwareID $gpu.PNPDeviceID
}