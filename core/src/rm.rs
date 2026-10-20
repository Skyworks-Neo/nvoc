//! Linux RM info lane (`/dev/nvidiactl`) — LACT-parity driver-handle summary.
//!
//! LACT's `DriverHandle` opens the NVIDIA character devices, allocates
//! client/device/subdevice objects through `NV_ESC_RM_ALLOC` and runs
//! `NV_ESC_RM_CONTROL` (`NVOS54`) against the subdevice to read the FB/GR
//! facts NVAPI does not expose: RAM type & vendor, bus width, VRAM size,
//! L2 cache size, ROP cluster info and the global SM order. This module is
//! the read-only equivalent for nvoc's Linux path. Non-Linux builds carry
//! the data shape and report `platform_supported() == false`.

use crate::error::Error;

/// The `/dev/nvidiactl` FB/GR summary for one GPU.
#[derive(Debug, Clone, Default)]
pub struct RmGpuInfo {
    /// FB_INFO RAM_TYPE decoded (`GDDR6`, `GDDR6X`, `HBM2`, ...).
    pub ram_type: Option<String>,
    /// FB_INFO MEMORYINFO_VENDOR_ID decoded (`Samsung`, `Micron`, ...).
    pub ram_vendor: Option<String>,
    /// FB_INFO BUS_WIDTH (bits).
    pub bus_width: Option<u32>,
    /// FB_INFO RAM_SIZE (KiB).
    pub ram_size_kb: Option<u32>,
    /// FB_INFO L2CACHE_SIZE (bytes).
    pub l2_cache_size: Option<u32>,
    /// GR_GET_ROP_INFO trio.
    pub rop_unit_count: Option<u32>,
    pub rop_operations_factor: Option<u32>,
    pub rop_operations_count: Option<u32>,
    /// GR_GET_GLOBAL_SM_ORDER `numSm`.
    pub sm_count: Option<u32>,
}

/// Whether the RM lane can run on this platform (Linux with the NVIDIA
/// character devices present).
pub fn platform_supported() -> bool {
    imp::platform_supported()
}

/// Open the RM handle stack for device `minor_number` (from NVML
/// `minor_number()`) and collect the FB/GR summary.
pub fn collect(minor_number: u32) -> Result<RmGpuInfo, Error> {
    imp::collect(minor_number)
}

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::c_void;
    use std::fs::{File, OpenOptions};
    use std::os::fd::AsRawFd;
    use std::os::raw::c_int;
    use std::ptr;

    use crate::error::Error;

    use super::RmGpuInfo;

    pub fn platform_supported() -> bool {
        true
    }

    // --- constants (nv-ioctl-numbers.h / nv_escape.h / class headers) -------

    const NV_IOCTL_MAGIC: u64 = 0x46; // 'F'
    const NV_ESC_REGISTER_FD: u64 = 201; // NV_IOCTL_BASE + 1
    const NV_ESC_RM_ALLOC: u64 = 0x2B;
    const NV_ESC_RM_CONTROL: u64 = 0x2A;

    const NV01_DEVICE_0: u32 = 0x80;
    const NV20_SUBDEVICE_0: u32 = 0x2080;

    const NV2080_CTRL_CMD_FB_GET_INFO: u32 = 0x2080_1301;
    const NV2080_CTRL_CMD_GR_GET_ROP_INFO: u32 = 0x2080_1213;
    const NV2080_CTRL_CMD_GR_GET_GLOBAL_SM_ORDER: u32 = 0x2080_121B;

    const FB_INFO_INDEX_RAM_SIZE: u32 = 0x7;
    const FB_INFO_INDEX_BUS_WIDTH: u32 = 0xB;
    const FB_INFO_INDEX_RAM_TYPE: u32 = 0xD;
    const FB_INFO_INDEX_L2CACHE_SIZE: u32 = 0x1B;
    const FB_INFO_INDEX_MEMORYINFO_VENDOR_ID: u32 = 0x1C;

    // --- ioctl plumbing ------------------------------------------------------

    /// Linux uapi `_IOWR(TYPE, NR, SIZE)`: direction 3 (read|write).
    fn iowr(nr: u64, size: usize) -> u64 {
        (3u64 << 30) | ((size as u64 & 0x3FFF) << 16) | (NV_IOCTL_MAGIC << 8) | nr
    }

    unsafe extern "C" {
        fn ioctl(fd: c_int, request: std::os::raw::c_ulong, ...) -> c_int;
    }

    fn raw_ioctl(fd: c_int, nr: u64, request_size: usize, arg: *mut c_void) -> Result<(), Error> {
        // SAFETY: each call passes the request struct the RM ABI expects for
        // `nr` at the size baked into the request code (see the size asserts
        // on the Nvos* types below).
        let rc = unsafe { ioctl(fd, iowr(nr, request_size) as std::os::raw::c_ulong, arg) };
        if rc < 0 {
            return Err(Error::Custom(format!(
                "ioctl {nr:#x} failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    // --- RM ABI structs (nvos.h / cl0080.h / cl2080.h / ctrl2080*) ----------

    #[repr(C)]
    #[derive(Default)]
    struct Nvos21Parameters {
        h_root: u32,
        h_object_parent: u32,
        h_object_new: u32,
        h_class: i32,
        p_alloc_parms: u64,
        params_size: u32,
        status: i32,
    }
    const _: () = assert!(std::mem::size_of::<Nvos21Parameters>() == 32);

    #[repr(C)]
    #[derive(Default)]
    struct Nvos64Parameters {
        h_root: u32,
        h_object_parent: u32,
        h_object_new: u32,
        h_class: i32,
        p_alloc_parms: u64,
        p_rights_requested: u64,
        params_size: u32,
        flags: u32,
        status: i32,
    }
    const _: () = assert!(std::mem::size_of::<Nvos64Parameters>() == 48);

    #[repr(C)]
    #[derive(Default)]
    struct Nvos54Parameters {
        h_client: u32,
        h_object: u32,
        cmd: i32,
        flags: u32,
        params: u64,
        params_size: u32,
        status: i32,
    }
    const _: () = assert!(std::mem::size_of::<Nvos54Parameters>() == 32);

    #[repr(C)]
    #[derive(Default)]
    struct Nv0080AllocParameters {
        device_id: u32,
        h_client_share: u32,
        h_target_client: u32,
        h_target_device: u32,
        flags: i32,
        va_space_size: u64,
        va_start_internal: u64,
        va_limit_internal: u64,
        va_mode: i32,
    }
    const _: () = assert!(std::mem::size_of::<Nv0080AllocParameters>() == 56);

    #[repr(C)]
    #[derive(Default)]
    struct Nv2080AllocParameters {
        sub_device_id: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Nv2080FbInfo {
        index: u32,
        data: u32,
    }

    #[repr(C)]
    struct Nv2080FbGetInfoParams {
        fb_info_list_size: u32,
        fb_info_list: u64,
    }
    const _: () = assert!(std::mem::size_of::<Nv2080FbGetInfoParams>() == 16);

    #[repr(C)]
    #[derive(Default)]
    struct Nv2080GrRopInfoParams {
        rop_unit_count: u32,
        rop_operations_factor: u32,
        rop_operations_count: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct GrGlobalSmId {
        gpc_id: u16,
        local_tpc_id: u16,
        local_sm_id: u16,
        global_tpc_id: u16,
        virtual_gpc_id: u16,
        migratable_tpc_id: u16,
        ugpu_id: u16,
        physical_cpc_id: u16,
        virtual_tpc_id: u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Nv0080GrRouteInfo {
        flags: u32,
        _pad: u32,
        route: u64,
    }

    #[repr(C)]
    struct Nv2080GrGlobalSmOrderParams {
        global_sm_id: [GrGlobalSmId; 512],
        num_sm: u16,
        num_tpc: u16,
        _align: [u8; 4],
        gr_route_info: Nv0080GrRouteInfo,
    }
    const _: () = assert!(std::mem::size_of::<Nv2080GrGlobalSmOrderParams>() == 9240);

    // --- handle stack ---------------------------------------------------------

    struct RmHandle {
        nvidiactl: File,
        _device: File,
        client_handle: u32,
        subdevice_handle: u32,
    }

    impl RmHandle {
        fn open(minor_number: u32) -> Result<Self, Error> {
            let nvidiactl = OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/nvidiactl")
                .map_err(|e| Error::Custom(format!("could not open /dev/nvidiactl: {e}")))?;

            // Client allocation: all-zero NVOS21 request, RM returns the new
            // client handle in hObjectNew (LACT DriverHandle::open parity).
            let mut client = Nvos21Parameters::default();
            raw_ioctl(
                nvidiactl.as_raw_fd(),
                NV_ESC_RM_ALLOC,
                std::mem::size_of::<Nvos21Parameters>(),
                ptr::from_mut(&mut client).cast(),
            )?;
            if client.status != 0 {
                return Err(Error::Custom(format!(
                    "RM client alloc failed with status {:#x}",
                    client.status
                )));
            }
            let client_handle = client.h_object_new;

            let device = OpenOptions::new()
                .read(true)
                .write(true)
                .open(format!("/dev/nvidia{minor_number}"))
                .map_err(|e| {
                    Error::Custom(format!("could not open /dev/nvidia{minor_number}: {e}"))
                })?;

            // Bind the device fd to the client (NV_ESC_REGISTER_FD, fd by value).
            let mut device_fd = device.as_raw_fd();
            raw_ioctl(
                nvidiactl.as_raw_fd(),
                NV_ESC_REGISTER_FD,
                std::mem::size_of::<c_int>(),
                ptr::from_mut(&mut device_fd).cast(),
            )?;

            let mut device_params = Nv0080AllocParameters {
                device_id: minor_number,
                ..Nv0080AllocParameters::default()
            };
            let device_handle = alloc_object(
                nvidiactl.as_raw_fd(),
                client_handle,
                client_handle,
                NV01_DEVICE_0,
                ptr::from_mut(&mut device_params).cast(),
                std::mem::size_of::<Nv0080AllocParameters>(),
            )?;

            let mut subdevice_params = Nv2080AllocParameters::default();
            let subdevice_handle = alloc_object(
                nvidiactl.as_raw_fd(),
                client_handle,
                device_handle,
                NV20_SUBDEVICE_0,
                ptr::from_mut(&mut subdevice_params).cast(),
                std::mem::size_of::<Nv2080AllocParameters>(),
            )?;

            Ok(Self {
                nvidiactl,
                _device: device,
                client_handle,
                subdevice_handle,
            })
        }

        fn rm_control<T>(&self, cmd: u32, params: &mut T) -> Result<(), Error> {
            let mut request = Nvos54Parameters {
                h_client: self.client_handle,
                h_object: self.subdevice_handle,
                cmd: cmd as i32,
                flags: 0,
                params: ptr::from_mut(params).addr() as u64,
                params_size: std::mem::size_of::<T>() as u32,
                status: 0,
            };
            raw_ioctl(
                self.nvidiactl.as_raw_fd(),
                NV_ESC_RM_CONTROL,
                std::mem::size_of::<Nvos54Parameters>(),
                ptr::from_mut(&mut request).cast(),
            )?;
            if request.status != 0 {
                return Err(Error::Custom(format!(
                    "RM control {cmd:#x} failed with status {:#x}",
                    request.status
                )));
            }
            Ok(())
        }

        fn fb_info(&self, indices: &[u32]) -> Result<Vec<Nv2080FbInfo>, Error> {
            let mut infos: Vec<Nv2080FbInfo> = indices
                .iter()
                .map(|&index| Nv2080FbInfo { index, data: 0 })
                .collect();
            let mut params = Nv2080FbGetInfoParams {
                fb_info_list_size: infos.len() as u32,
                fb_info_list: ptr::from_mut(&mut infos).addr() as u64,
            };
            self.rm_control(NV2080_CTRL_CMD_FB_GET_INFO, &mut params)?;
            Ok(infos)
        }

        fn collect_info(&self) -> Result<RmGpuInfo, Error> {
            let mut info = RmGpuInfo::default();

            let fb = self.fb_info(&[
                FB_INFO_INDEX_RAM_SIZE,
                FB_INFO_INDEX_BUS_WIDTH,
                FB_INFO_INDEX_RAM_TYPE,
                FB_INFO_INDEX_L2CACHE_SIZE,
                FB_INFO_INDEX_MEMORYINFO_VENDOR_ID,
            ])?;
            info.ram_size_kb = fb.first().map(|i| i.data).filter(|&d| d != 0);
            info.bus_width = fb.get(1).map(|i| i.data).filter(|&d| d != 0);
            info.ram_type = fb.get(2).and_then(|i| decode_ram_type(i.data));
            info.l2_cache_size = fb.get(3).map(|i| i.data).filter(|&d| d != 0);
            info.ram_vendor = fb.get(4).and_then(|i| decode_ram_vendor(i.data));

            let mut rop = Nv2080GrRopInfoParams::default();
            if self
                .rm_control(NV2080_CTRL_CMD_GR_GET_ROP_INFO, &mut rop)
                .is_ok()
            {
                info.rop_unit_count = Some(rop.rop_unit_count).filter(|&v| v != 0);
                info.rop_operations_factor = Some(rop.rop_operations_factor);
                info.rop_operations_count = Some(rop.rop_operations_count).filter(|&v| v != 0);
            }

            // ~9 KB params blob — keep it heap-side.
            let mut sm_order = Box::new(Nv2080GrGlobalSmOrderParams {
                global_sm_id: [GrGlobalSmId::default(); 512],
                num_sm: 0,
                num_tpc: 0,
                _align: [0; 4],
                gr_route_info: Nv0080GrRouteInfo::default(),
            });
            if self
                .rm_control(NV2080_CTRL_CMD_GR_GET_GLOBAL_SM_ORDER, &mut *sm_order)
                .is_ok()
            {
                info.sm_count = Some(u32::from(sm_order.num_sm)).filter(|&v| v != 0);
            }

            Ok(info)
        }
    }

    fn alloc_object(
        fd: c_int,
        root: u32,
        parent: u32,
        class: u32,
        params: *mut c_void,
        params_size: usize,
    ) -> Result<u32, Error> {
        let mut request = Nvos64Parameters {
            h_root: root,
            h_object_parent: parent,
            h_object_new: 0,
            h_class: class as i32,
            p_alloc_parms: params.addr() as u64,
            p_rights_requested: 0,
            params_size: params_size as u32,
            flags: 0,
            status: 0,
        };
        raw_ioctl(
            fd,
            NV_ESC_RM_ALLOC,
            std::mem::size_of::<Nvos64Parameters>(),
            ptr::from_mut(&mut request).cast(),
        )?;
        if request.status != 0 {
            return Err(Error::Custom(format!(
                "RM alloc of class {class:#x} failed with status {:#x}",
                request.status
            )));
        }
        Ok(request.h_object_new)
    }

    fn decode_ram_type(value: u32) -> Option<String> {
        let name = match value {
            0x1 => "SDRAM",
            0x2 => "DDR1",
            0x3 => "DDR2",
            0x4 => "GDDR2",
            0x5 => "GDDR3",
            0x6 => "GDDR4",
            0x7 => "DDR3",
            0x8 => "GDDR5",
            0x9 => "LPDDR2",
            0xC => "SDDR4",
            0xD => "LPDDR4",
            0xE => "HBM1",
            0xF => "HBM2",
            0x10 => "GDDR5X",
            0x11 => "GDDR6",
            0x12 => "GDDR6X",
            0x13 => "LPDDR5",
            0x14 => "HBM3",
            0x15 => "GDDR7",
            0x16 => "HBM4",
            _ => return None,
        };
        Some(name.to_owned())
    }

    fn decode_ram_vendor(value: u32) -> Option<String> {
        let name = match value {
            0x1 => "Samsung",
            0x2 => "Qimonda",
            0x3 => "Elpida",
            0x4 => "Etron",
            0x5 => "Nanya",
            0x6 => "SK Hynix",
            0x7 => "Mosel",
            0x8 => "Winbond",
            0x9 => "ESMT",
            0xF => "Micron",
            _ => return None,
        };
        Some(name.to_owned())
    }

    pub fn collect(minor_number: u32) -> Result<RmGpuInfo, Error> {
        RmHandle::open(minor_number)?.collect_info()
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use crate::error::Error;

    use super::RmGpuInfo;

    pub fn platform_supported() -> bool {
        false
    }

    pub fn collect(_minor_number: u32) -> Result<RmGpuInfo, Error> {
        Err(Error::Custom(
            "the /dev/nvidiactl RM info lane requires Linux".to_owned(),
        ))
    }
}
