#! /bin/bash
# Expect: this_script {nv_pci_like_0000:01:00.0}

set -e

if [[ -z "$1" ]]; then
  echo "Usage: sudo this_script.sh NV_GPU_PCI_ID (like 0000:01:00.0)" 1>&2
  exit 127
fi

if [[  "$(id -u)" != "0" ]]; then
  echo "Run with sudo" 1>&2
  exit 127
fi

gpu_pci="$1"

echo "Unbinding GPU at $gpu_pci from NVIDIA driver"

echo "$gpu_pci" > /sys/bus/pci/drivers/nvidia/unbind

sleep 2

echo "Resetting GPU"

echo 1 > /sys/bus/pci/devices/$gpu_pci/reset

sleep 2

echo "Binding GPU to NVIDIA driver"

echo "$gpu_pci" > /sys/bus/pci/drivers/nvidia/bind
