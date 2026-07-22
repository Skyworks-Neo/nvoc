//! CUDA device enumeration and selection (by UUID / PCI bus / sorted index),
//! plus compute-capability / memory queries.

use std::ffi::CStr;

use cudarc::driver::sys as cuda_sys;

use cli_stressor_cuda_rs::{BackendError, CudaDeviceEnumInfo, DeviceInfo, PciBusAddress};

/// Query the [`DeviceInfo`] (name / total memory / compute capability) for the
/// CUDA device at `device_index`. Used by the backend constructor.
pub(super) fn query_device_info_for_index(device_index: u32) -> Result<DeviceInfo, BackendError> {
    let mut device = 0;
    unsafe {
        let res = cuda_sys::cuDeviceGet(&mut device, device_index as i32);
        if res as u32 != 0 {
            return Err(BackendError::Other(format!(
                "cuDeviceGet failed for device {}: error code {}",
                device_index, res as u32
            )));
        }
    }

    let mut name_buf = [0i8; 128];
    unsafe {
        cuda_sys::cuDeviceGetName(name_buf.as_mut_ptr(), name_buf.len() as i32, device);
    }
    let name = unsafe { CStr::from_ptr(name_buf.as_ptr()) }
        .to_string_lossy()
        .trim_end_matches('\0')
        .to_string();

    let mut total_mem = 0usize;
    unsafe {
        cuda_sys::cuDeviceTotalMem_v2(&mut total_mem as *mut usize, device);
    }
    let total_mem_gb = if total_mem > 0 {
        Some(total_mem as f64 / 1024.0 / 1024.0 / 1024.0)
    } else {
        None
    };

    let mut major = 0i32;
    let mut minor = 0i32;
    unsafe {
        cuda_sys::cuDeviceGetAttribute(
            &mut major,
            cuda_sys::CUdevice_attribute::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR,
            device,
        );
        cuda_sys::cuDeviceGetAttribute(
            &mut minor,
            cuda_sys::CUdevice_attribute::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR,
            device,
        );
    }

    Ok(DeviceInfo {
        name,
        total_mem_gb,
        compute_capability: Some((major, minor)),
    })
}

pub fn enumerate_cuda_devices() -> Result<Vec<CudaDeviceEnumInfo>, BackendError> {
    unsafe {
        let res = cuda_sys::cuInit(0);
        if res as u32 != 0 {
            return Err(BackendError::Other(format!(
                "cuInit failed: error code {}",
                res as u32
            )));
        }
    }

    let mut device_count = 0i32;
    unsafe {
        let res = cuda_sys::cuDeviceGetCount(&mut device_count);
        if res as u32 != 0 {
            return Err(BackendError::Other(format!(
                "cuDeviceGetCount failed: error code {}",
                res as u32
            )));
        }
    }

    let mut devices = Vec::new();
    for idx in 0..device_count {
        let device_index = idx as u32;
        let mut device = 0i32;
        unsafe {
            cuda_sys::cuDeviceGet(&mut device, idx);
        }

        let mut name_buf = [0i8; 128];
        unsafe {
            cuda_sys::cuDeviceGetName(name_buf.as_mut_ptr(), name_buf.len() as i32, device);
        }
        let device_name = unsafe { CStr::from_ptr(name_buf.as_ptr()) }
            .to_string_lossy()
            .trim_end_matches('\0')
            .to_string();

        let uuid = fetch_device_uuid(device)?;
        let pci_bus = fetch_device_pci_bus(device)?;

        let mut total_mem = 0usize;
        unsafe {
            cuda_sys::cuDeviceTotalMem_v2(&mut total_mem as *mut usize, device);
        }
        let total_mem_gb = if total_mem > 0 {
            Some(total_mem as f64 / 1024.0 / 1024.0 / 1024.0)
        } else {
            None
        };

        let mut major = 0i32;
        let mut minor = 0i32;
        unsafe {
            cuda_sys::cuDeviceGetAttribute(
                &mut major,
                cuda_sys::CUdevice_attribute::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR,
                device,
            );
            cuda_sys::cuDeviceGetAttribute(
                &mut minor,
                cuda_sys::CUdevice_attribute::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR,
                device,
            );
        }
        let compute_capability = Some((major, minor));

        devices.push(CudaDeviceEnumInfo {
            device_index,
            device_name,
            uuid,
            pci_bus,
            compute_capability,
            total_mem_gb,
        });
    }

    Ok(devices)
}

pub fn resolve_device_index_by_uuid(target_uuid: [u8; 16]) -> Result<u32, String> {
    let devices =
        enumerate_cuda_devices().map_err(|e| format!("failed to enumerate devices: {e}"))?;
    for dev in &devices {
        if dev.uuid == target_uuid {
            println!(
                "[CUDA] Selected device index {} by UUID match: {}",
                dev.device_index, dev.device_name
            );
            return Ok(dev.device_index);
        }
    }
    Err(format!(
        "no CUDA device found with UUID {}",
        format_uuid_hex(&target_uuid)
    ))
}

pub fn resolve_device_index_by_pci_bus(target_pci: PciBusAddress) -> Result<u32, String> {
    let devices =
        enumerate_cuda_devices().map_err(|e| format!("failed to enumerate devices: {e}"))?;
    for dev in &devices {
        if let Some(pci) = dev.pci_bus
            && pci == target_pci
        {
            println!(
                "[CUDA] Selected device index {} by PCI match: {}",
                dev.device_index, dev.device_name
            );
            return Ok(dev.device_index);
        }
    }
    Err(format!(
        "no CUDA device found with PCI {}",
        format_pci_address(&target_pci)
    ))
}

pub fn resolve_device_index_by_sorted_index(sorted_index: u32) -> Result<u32, String> {
    let mut devices =
        enumerate_cuda_devices().map_err(|e| format!("failed to enumerate devices: {e}"))?;

    // Sort by PCI bus address
    devices.sort_by(|a, b| match (a.pci_bus, b.pci_bus) {
        (Some(pci_a), Some(pci_b)) => (pci_a.domain, pci_a.bus, pci_a.device, pci_a.function)
            .cmp(&(pci_b.domain, pci_b.bus, pci_b.device, pci_b.function)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    if sorted_index >= devices.len() as u32 {
        return Err(format!(
            "sorted index {} out of range (available: 0-{})",
            sorted_index,
            devices.len() - 1
        ));
    }

    let dev = &devices[sorted_index as usize];
    println!(
        "[CUDA] Selected device index {} (sorted position {}) by index: {}",
        dev.device_index, sorted_index, dev.device_name
    );
    Ok(dev.device_index)
}

fn format_uuid_hex(uuid: &[u8; 16]) -> String {
    uuid.iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join("")
}

fn format_pci_address(pci: &PciBusAddress) -> String {
    format!(
        "{:04X}:{:02X}:{:02X}.{}",
        pci.domain, pci.bus, pci.device, pci.function
    )
}

fn fetch_device_uuid(device: i32) -> Result<[u8; 16], BackendError> {
    let mut raw_uuid = std::mem::MaybeUninit::<cuda_sys::CUuuid>::zeroed();
    unsafe {
        let res = cuda_sys::cuDeviceGetUuid_v2(raw_uuid.as_mut_ptr(), device);
        if res as u32 != 0 {
            // Return zero UUID if not supported instead of failing
            return Ok([0u8; 16]);
        }
        let raw_uuid = raw_uuid.assume_init();
        let uuid: [u8; 16] = std::mem::transmute(raw_uuid);
        Ok(uuid)
    }
}

fn fetch_device_pci_bus(device: i32) -> Result<Option<PciBusAddress>, BackendError> {
    let mut buf = [0i8; 32];
    unsafe {
        let res = cuda_sys::cuDeviceGetPCIBusId(buf.as_mut_ptr(), buf.len() as i32, device);
        if res as u32 != 0 {
            return Ok(None);
        }
    }

    let pci_bus_id = unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .trim()
        .to_string();
    if pci_bus_id.is_empty() {
        return Ok(None);
    }

    parse_cuda_pci_bus_id(&pci_bus_id)
        .map(Some)
        .map_err(BackendError::Other)
}

#[cfg(feature = "vulkan")]
pub(super) fn query_cuda_device_uuid(device_index: u32) -> Result<[u8; 16], BackendError> {
    let mut device = 0;
    unsafe {
        let res = cuda_sys::cuDeviceGet(&mut device, device_index as i32);
        if res as u32 != 0 {
            return Err(BackendError::Other(format!(
                "cuDeviceGet failed: error code {}",
                res as u32
            )));
        }
    }

    let mut raw_uuid = std::mem::MaybeUninit::<cuda_sys::CUuuid>::zeroed();
    unsafe {
        let res = cuda_sys::cuDeviceGetUuid_v2(raw_uuid.as_mut_ptr(), device);
        if res as u32 != 0 {
            return Err(BackendError::Other(format!(
                "cuDeviceGetUuid_v2 failed: error code {}",
                res as u32
            )));
        }
        let raw_uuid = raw_uuid.assume_init();
        let uuid: [u8; 16] = std::mem::transmute(raw_uuid);
        Ok(uuid)
    }
}

#[cfg(feature = "vulkan")]
pub(super) fn query_cuda_device_pci_bus_address(
    device_index: u32,
) -> Result<Option<PciBusAddress>, BackendError> {
    let mut device = 0;
    unsafe {
        let res = cuda_sys::cuDeviceGet(&mut device, device_index as i32);
        if res as u32 != 0 {
            return Err(BackendError::Other(format!(
                "cuDeviceGet failed: error code {}",
                res as u32
            )));
        }
    }

    fetch_device_pci_bus(device)
}

fn parse_cuda_pci_bus_id(raw: &str) -> Result<PciBusAddress, String> {
    let raw = raw.trim();
    let (domain_raw, rest) = raw
        .split_once(':')
        .ok_or_else(|| format!("invalid CUDA PCI bus id: {raw}"))?;
    let (bus_raw, rest) = rest
        .split_once(':')
        .ok_or_else(|| format!("invalid CUDA PCI bus id: {raw}"))?;
    let (device_raw, function_raw) = rest
        .split_once('.')
        .ok_or_else(|| format!("invalid CUDA PCI bus id: {raw}"))?;

    let domain = u32::from_str_radix(domain_raw, 16)
        .map_err(|_| format!("invalid PCI domain in bus id: {domain_raw}"))?;
    let bus = u32::from_str_radix(bus_raw, 16)
        .map_err(|_| format!("invalid PCI bus in bus id: {bus_raw}"))?;
    let device = u32::from_str_radix(device_raw, 16)
        .map_err(|_| format!("invalid PCI device in bus id: {device_raw}"))?;
    let function = u32::from_str_radix(function_raw, 16)
        .map_err(|_| format!("invalid PCI function in bus id: {function_raw}"))?;

    Ok(PciBusAddress {
        domain,
        bus,
        device,
        function,
    })
}
