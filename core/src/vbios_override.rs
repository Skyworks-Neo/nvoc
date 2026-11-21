//! vBIOS 注册表 override（vix loader 车道）——Windows-only。
//!
//! 机制（Panchovix nvidia-vbios-vix-loader README + mVolt+ 0.50.13 RE 定案，
//! 2026-11）：GPU 的显示类驱动键
//! `HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e968-...}\NNNN` 下的
//! **`vbios` REG_BINARY 直接存 VBIOS 镜像字节链**（非文件路径）；驱动在
//! 初始化时读取（616.56+），默认模式需 Windows 完整重启生效，GSP 固件
//! 模式（EnableGpuFirmware=0x11）下设备禁用/启用即可加载。仅校验镜像
//! checksum 与单镜像 PCI ID，不验签名。nvlddmkm 侧同族值名：`vbiosSource`
//! / `romOverride` / `vbios000`。
//!
//! 枚举走 SetupDi（mVolt+ 同款：GetClassDevs→EnumDeviceInfo→
//! OpenDevRegKey(DIREG_DRV)），mVolt 用 LoadLibraryExA 动态解析 Reg*W 以
//! 躲静态导入表；这里正常链接（Win32_System_Registry）。

#[cfg(windows)]
pub use windows_impl::{
    DisplayAdapter, FragmentMergeReport, RomOverrideFragment, SessionState, VBIOS_VALUE_NAME,
    VbiosOverrideEntry, build_romoverride_fragment, clear_all_nvidia_overrides, clear_override,
    clear_romoverride_all, delete_vbios_value, device_problem, enumerate_display_adapters,
    merge_romoverride_chain, read_override, read_romoverride_fragments, read_session_state,
    trigger_reinit, write_override, write_romoverride_fragments,
};

#[cfg(windows)]
mod windows_impl {
    use crate::Error;
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_Status, CM_LOCATE_DEVNODE_NORMAL, CM_Locate_DevNodeW, CONFIGRET, CR_SUCCESS,
        DICS_DISABLE, DICS_ENABLE, DICS_FLAG_CONFIGSPECIFIC, DIF_PROPERTYCHANGE, DN_HAS_PROBLEM,
        HDEVINFO, SETUP_DI_GET_CLASS_DEVS_FLAGS, SETUP_DI_REGISTRY_PROPERTY,
        SP_CLASSINSTALL_HEADER, SP_DEVINFO_DATA, SP_PROPCHANGE_PARAMS, SetupDiChangeState,
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
        SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceRegistryPropertyW, SetupDiOpenDevRegKey,
        SetupDiSetClassInstallParamsW,
    };
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS};
    use windows_sys::Win32::System::Registry::{
        HKEY, KEY_READ, KEY_WRITE, REG_BINARY, REG_VALUE_TYPE, RegCloseKey, RegDeleteValueW,
        RegFlushKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };

    /// GUID_DEVCLASS_DISPLAY {4d36e968-e325-11ce-bfc1-08002be10318}（自持
    /// 常量，不依赖 windows-sys 的常量摆放）。
    const GUID_DEVCLASS_DISPLAY: windows_sys::core::GUID =
        windows_sys::core::GUID::from_u128(0x4d36e968_e325_11ce_bfc1_08002be10318);

    /// SPDRP_DEVICEDESCRIPTION = 0（setupapi 标准；windows-sys 未导出）。
    const SPDRP_DEVICEDESCRIPTION: SETUP_DI_REGISTRY_PROPERTY = 0;

    /// DIGCF_PRESENT（仅枚举在位设备）。
    const DIGCF_PRESENT: SETUP_DI_GET_CLASS_DEVS_FLAGS = 2;

    /// DICS_FLAG_GLOBAL / DIREG_DRV（打开硬件驱动键）。
    const DICS_FLAG_GLOBAL: u32 = 1;
    const DIREG_DRV: u32 = 2;

    /// vix loader 车道的注册表值名。
    pub const VBIOS_VALUE_NAME: &str = "vbios";

    /// 一块显示适配器（仅 NVIDIA 有意义，但枚举不过滤——让调用方选择）。
    #[derive(Debug, Clone)]
    pub struct DisplayAdapter {
        /// 设备实例路径，如
        /// `PCI\VEN_10DE&DEV_1F02&SUBSYS_...&BUS_01&DEV_00&FUNC_0\10de...`。
        pub instance_id: String,
        pub description: String,
        pub is_nvidia: bool,
        /// PCI 总线号（BUS_xx 段）。
        pub bus: u16,
        /// PCI 设备号（DEV_xx 段）。
        pub device: u16,
        /// PCI 功能号（FUNC_x 段）。
        pub func: u16,
    }

    fn last_error() -> String {
        format!(
            "Win32 error {}",
            std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
        )
    }

    fn last_err() -> u32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32
    }

    fn new_devinfo_data() -> SP_DEVINFO_DATA {
        // SAFETY: SP_DEVINFO_DATA 是纯数据结构（GUID + u32 + usize），全零
        // 是合法初值；cbSize 随后覆写。
        SP_DEVINFO_DATA {
            cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
            ..unsafe { std::mem::zeroed() }
        }
    }

    /// 枚举在位的显示适配器。只读实例路径与描述，不需要管理员。
    pub fn enumerate_display_adapters() -> Result<Vec<DisplayAdapter>, Error> {
        unsafe {
            let devinfo = SetupDiGetClassDevsW(
                &GUID_DEVCLASS_DISPLAY,
                std::ptr::null(),
                std::ptr::null_mut(),
                DIGCF_PRESENT,
            );
            if devinfo == 0 {
                return Err(Error::from(format!(
                    "SetupDiGetClassDevsW failed: {}",
                    last_error()
                )));
            }
            let mut out = Vec::new();
            let mut index: u32 = 0;
            loop {
                let mut dev = new_devinfo_data();
                if SetupDiEnumDeviceInfo(devinfo, index, &mut dev) == 0 {
                    if last_err() == ERROR_NO_MORE_ITEMS {
                        break;
                    }
                    SetupDiDestroyDeviceInfoList(devinfo);
                    return Err(Error::from(format!(
                        "SetupDiEnumDeviceInfo({index}) failed: {}",
                        last_error()
                    )));
                }
                let mut len: u32 = 0;
                SetupDiGetDeviceInstanceIdW(devinfo, &dev, std::ptr::null_mut(), 0, &mut len);
                if len > 0 {
                    let mut buf = vec![0u16; len as usize];
                    if SetupDiGetDeviceInstanceIdW(
                        devinfo,
                        &dev,
                        buf.as_mut_ptr(),
                        buf.len() as u32,
                        &mut len,
                    ) != 0
                    {
                        let instance_id = String::from_utf16_lossy(&buf[..len as usize]);
                        let lower = instance_id.to_ascii_lowercase();
                        let is_nvidia = lower.contains("ven_10de");
                        let (bus, device, func) = parse_bdf(&lower);
                        let description = device_description(devinfo, &dev);
                        out.push(DisplayAdapter {
                            instance_id,
                            description,
                            is_nvidia,
                            bus,
                            device,
                            func,
                        });
                    }
                }
                index += 1;
            }
            SetupDiDestroyDeviceInfoList(devinfo);
            Ok(out)
        }
    }

    fn device_description(devinfo: isize, dev: &SP_DEVINFO_DATA) -> String {
        unsafe {
            let mut buf = [0u16; 512];
            let mut len: u32 = 0;
            if SetupDiGetDeviceRegistryPropertyW(
                devinfo,
                dev,
                SPDRP_DEVICEDESCRIPTION,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut len,
            ) != 0
                && len > 2
            {
                return String::from_utf16_lossy(&buf[..(len / 2) as usize]);
            }
        }
        String::from("(unknown)")
    }

    /// `bus_xx&dev_xx&func_x` → (bus, device, func)。这三段只出现在
    /// PCIROOT 父节点的实例路径里；显示适配器自己的实例 id（
    /// `PCI\VEN_x&DEV_x&SUBSYS_...\<inst>`，DEV_ 是硬件设备 ID 不是总线
    /// 设备号）通常**没有**总线段——此时返回 (0,0,0)，匹配退化为单卡
    /// 自动 + 多卡列表。
    fn parse_bdf(lower: &str) -> (u16, u16, u16) {
        let Some(bp) = lower.find("bus_") else {
            return (0, 0, 0);
        };
        let tail = &lower[bp..];
        let parts = tail.split('&').map_while(|s| {
            let (k, v) = s.split_once('_')?;
            Some((k, v))
        });
        let get = |key: &str| -> u16 {
            parts
                .clone()
                .find(|(k, _)| *k == key)
                .and_then(|(_, v)| u16::from_str_radix(v, 16).ok())
                .unwrap_or(0)
        };
        let (bus, dev, func) = (get("bus"), get("dev"), get("func"));
        // 设备号 >31 / 功能号 >7 说明解析到的不是总线段（假命中）。
        if bus > 255 || dev > 31 || func > 7 {
            return (0, 0, 0);
        }
        (bus, dev, func)
    }

    /// 打开实例的驱动键（DIREG_DRV = `Class\{4d36e968-...}\NNNN`）。
    /// `access` = KEY_READ（状态读取，Users 可用）或 KEY_READ|KEY_WRITE
    /// （override 写通道，需要管理员）。
    fn open_driver_key(instance_id: &str, access: u32) -> Result<HKEY, Error> {
        unsafe {
            let devinfo = SetupDiGetClassDevsW(
                &GUID_DEVCLASS_DISPLAY,
                std::ptr::null(),
                std::ptr::null_mut(),
                DIGCF_PRESENT,
            );
            if devinfo == 0 {
                return Err(Error::from(format!(
                    "SetupDiGetClassDevsW failed: {}",
                    last_error()
                )));
            }
            let mut result: Result<HKEY, Error> = Err(Error::from(
                "vbios_override: adapter not found while opening its driver key",
            ));
            for index in 0u32.. {
                let mut dev = new_devinfo_data();
                if SetupDiEnumDeviceInfo(devinfo, index, &mut dev) == 0 {
                    break; // 枚举到头
                }
                let mut len: u32 = 0;
                SetupDiGetDeviceInstanceIdW(devinfo, &dev, std::ptr::null_mut(), 0, &mut len);
                if len == 0 {
                    continue;
                }
                let mut buf = vec![0u16; len as usize];
                if SetupDiGetDeviceInstanceIdW(
                    devinfo,
                    &dev,
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                    &mut len,
                ) != 0
                    && String::from_utf16_lossy(&buf[..len as usize])
                        .eq_ignore_ascii_case(instance_id)
                {
                    let hkey =
                        SetupDiOpenDevRegKey(devinfo, &dev, DICS_FLAG_GLOBAL, 0, DIREG_DRV, access);
                    if hkey.is_null() || hkey as isize == -1 {
                        result = Err(Error::from(format!(
                            "SetupDiOpenDevRegKey failed (admin required for writes): {}",
                            last_error()
                        )));
                    } else {
                        result = Ok(hkey);
                    }
                    break;
                }
            }
            SetupDiDestroyDeviceInfoList(devinfo);
            result
        }
    }

    fn wide_name(name: &str) -> Vec<u16> {
        name.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// `Session` 子键运行层状态（驱动自己写的输出面）。实查语义：
    /// - `vbiosSource`：当前 VBIOS 来源标记（2070/flash=07，
    ///   5060 Ti/override 生效=00——精确枚举待 reset 对照实验）；
    /// - `Session\vbios`：当前实际加载镜像的回显（55 AA 开头）；
    /// - 根部 `romOverride00` 是否存在：mVolt 车道（分块 romOverride00~NN）
    ///   是否在写——mVolt 的 override 开关就是这个值族的有无（用户实查）。
    #[derive(Debug, Clone)]
    pub struct SessionState {
        pub vbios_source: u32,
        pub session_vbios_len: usize,
        pub mvolt_romoverride_present: bool,
    }

    /// 读取 Session 状态。键不存在（老驱动）返回 None，不阻断其余读取。
    pub fn read_session_state(instance_id: &str) -> Option<SessionState> {
        unsafe {
            let hkey = open_driver_key(instance_id, KEY_READ).ok()?;
            // mVolt 车道：根部 romOverride00（读长度即可判定存在）。
            let mut mvolt = false;
            let rname = wide_name("romOverride00");
            let mut rlen: u32 = 0;
            if RegQueryValueExW(
                hkey,
                rname.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut rlen,
            ) == 0
            {
                mvolt = rlen > 0;
            }
            let session = wide_name("Session");
            let mut sub: HKEY = std::ptr::null_mut();
            if RegOpenKeyExW(hkey, session.as_ptr(), 0, KEY_READ, &mut sub) != 0 {
                RegCloseKey(hkey);
                return None;
            }
            RegCloseKey(hkey);
            let mut source: Option<u32> = None;
            let name = wide_name("vbiosSource");
            let mut v: u32 = 0;
            let mut len: u32 = 4;
            let mut ty: REG_VALUE_TYPE = 0;
            if RegQueryValueExW(
                sub,
                name.as_ptr(),
                std::ptr::null(),
                &mut ty,
                &mut v as *mut u32 as *mut u8,
                &mut len,
            ) == 0
            {
                source = Some(v);
            }
            let mut vlen = 0usize;
            let vname = wide_name(VBIOS_VALUE_NAME);
            let mut blen: u32 = 0;
            if RegQueryValueExW(
                sub,
                vname.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut blen,
            ) == 0
            {
                vlen = blen as usize;
            }
            RegCloseKey(sub);
            source.map(|s| SessionState {
                vbios_source: s,
                session_vbios_len: vlen,
                mvolt_romoverride_present: mvolt,
            })
        }
    }

    /// 一个 romOverride 值的解码视图。
    #[derive(Debug, Clone)]
    pub struct RomOverrideFragment {
        /// 值名（romOverride00..FF）。
        pub name: String,
        /// 版本指纹 A（BE u32，0=通配）。
        pub version_gate_a: u32,
        /// 版本指纹 B（0=通配）。
        pub version_gate_b: u8,
        /// 镜像内偏移。
        pub offset: u32,
        pub len: u8,
        pub data_hex: String,
        /// 完整原始值字节（合并链时原样重发用）。
        pub raw: Vec<u8>,
    }

    /// 构造 romOverride 片段字节（指纹可非零，供高级用法）。数据 ≤246B；
    /// 常规 power-limit 片段用全零通配指纹（驱动 `!v10 ||` 短路明证）。
    pub fn build_romoverride_fragment(
        version_gate_a_be: [u8; 4],
        version_gate_b: u8,
        offset: u32,
        data: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(10 + data.len());
        out.extend_from_slice(&version_gate_a_be);
        out.push(version_gate_b);
        out.extend_from_slice(&offset.to_le_bytes());
        out.push(data.len() as u8);
        out.extend_from_slice(data);
        out
    }

    fn romoverride_name(i: u8) -> Vec<u16> {
        wide_name(&format!("romOverride{i:02X}"))
    }

    /// 读全部 romOverride00..FF 片段（顺序到首个缺失值）。只读。
    pub fn read_romoverride_fragments(
        instance_id: &str,
    ) -> Result<Vec<RomOverrideFragment>, Error> {
        unsafe {
            let hkey = open_driver_key(instance_id, KEY_READ)?;
            let mut out = Vec::new();
            for i in 0u8..=0xFF {
                let name = romoverride_name(i);
                let mut len: u32 = 0;
                let r = RegQueryValueExW(
                    hkey,
                    name.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut len,
                );
                if r != 0 && r != ERROR_INSUFFICIENT_BUFFER {
                    break; // 断链
                }
                let mut buf = vec![0u8; len as usize];
                let r2 = RegQueryValueExW(
                    hkey,
                    name.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    buf.as_mut_ptr(),
                    &mut len,
                );
                if r2 != 0 || buf.len() < 10 {
                    break;
                }
                let ga = (u32::from(buf[0]) << 24)
                    | (u32::from(buf[1]) << 16)
                    | (u32::from(buf[2]) << 8)
                    | u32::from(buf[3]);
                let gb = buf[4];
                let off = u32::from_le_bytes([buf[5], buf[6], buf[7], buf[8]]);
                let dlen = buf[9] as usize;
                let data_hex: String = buf[10..(10 + dlen).min(buf.len())]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                out.push(RomOverrideFragment {
                    name: format!("romOverride{i:02X}"),
                    version_gate_a: ga,
                    version_gate_b: gb,
                    offset: off,
                    len: buf[9],
                    data_hex,
                    raw: buf[..(10 + dlen).min(buf.len())].to_vec(),
                });
            }
            RegCloseKey(hkey);
            Ok(out)
        }
    }

    /// 写片段链：先清 romOverride00..FF（防旧链残留/断链），再按序写
    /// romOverride00..{n-1}。**需要管理员**。
    pub fn write_romoverride_fragments(
        instance_id: &str,
        fragments: &[Vec<u8>],
    ) -> Result<usize, Error> {
        if fragments.len() > 0x100 {
            return Err(Error::from("romOverride: >256 fragments"));
        }
        for f in fragments {
            if f.len() > 256 || f.len() < 10 {
                return Err(Error::from("romOverride: fragment must be 10..=256 bytes"));
            }
        }
        unsafe {
            let hkey = open_driver_key(instance_id, KEY_READ | KEY_WRITE)?;
            for i in 0u8..=0xFF {
                let name = romoverride_name(i);
                RegDeleteValueW(hkey, name.as_ptr());
            }
            let mut written = 0usize;
            for (i, frag) in fragments.iter().enumerate() {
                let name = romoverride_name(i as u8);
                let r = RegSetValueExW(
                    hkey,
                    name.as_ptr(),
                    0,
                    REG_BINARY,
                    frag.as_ptr().cast(),
                    frag.len() as u32,
                );
                if r != 0 {
                    RegCloseKey(hkey);
                    return Err(Error::from(format!(
                        "RegSetValueExW(romOverride{i:02X}) failed: {r} (admin required?)",
                    )));
                }
                written += 1;
            }
            let flushed = RegFlushKey(hkey);
            RegCloseKey(hkey);
            if flushed != 0 {
                return Err(Error::from(format!("RegFlushKey failed: {flushed}")));
            }
            Ok(written)
        }
    }

    /// 清空 romOverride00..FF（direct patch 车道关闭）。返回删掉的值数。
    /// **需要管理员**。
    pub fn clear_romoverride_all(instance_id: &str) -> Result<usize, Error> {
        unsafe {
            let hkey = open_driver_key(instance_id, KEY_READ | KEY_WRITE)?;
            let mut n = 0usize;
            // 两位名（621 驱动消费面 00..FF）+ 三位名（mVolt 备用命名 000..999，
            // 跨版本残留清理——驱动不读三位，但留着会污染其它驱动代）。
            for i in 0u8..=0xFF {
                let name = romoverride_name(i);
                if RegDeleteValueW(hkey, name.as_ptr()) == 0 {
                    n += 1;
                }
            }
            for i in 0..1000u32 {
                let name = wide_name(&format!("romOverride{i:03}"));
                if RegDeleteValueW(hkey, name.as_ptr()) == 0 {
                    n += 1;
                }
            }
            RegCloseKey(hkey);
            Ok(n)
        }
    }

    /// 读取 override 值（None = 未设置）。驱动键对 Users 可读。
    pub fn read_override(instance_id: &str) -> Result<Option<Vec<u8>>, Error> {
        unsafe {
            let hkey = open_driver_key(instance_id, KEY_READ)?;
            let name = wide_name(VBIOS_VALUE_NAME);
            let mut ty: REG_VALUE_TYPE = 0;
            let mut len: u32 = 0;
            let r = RegQueryValueExW(
                hkey,
                name.as_ptr(),
                std::ptr::null(),
                &mut ty,
                std::ptr::null_mut(),
                &mut len,
            );
            if r != 0 && r != ERROR_INSUFFICIENT_BUFFER {
                RegCloseKey(hkey);
                return Ok(None); // 值不存在（ERROR_FILE_NOT_FOUND 等）
            }
            let mut buf = vec![0u8; len as usize];
            let r2 = RegQueryValueExW(
                hkey,
                name.as_ptr(),
                std::ptr::null(),
                &mut ty,
                buf.as_mut_ptr(),
                &mut len,
            );
            RegCloseKey(hkey);
            if r2 == 0 && ty == REG_BINARY {
                buf.truncate(len as usize);
                Ok(Some(buf))
            } else {
                Err(Error::from(format!(
                    "RegQueryValueExW(vbios) failed: {r2} (type {ty})",
                )))
            }
        }
    }

    /// 写入 override 镜像。**需要管理员**；写后 flush。
    pub fn write_override(instance_id: &str, image: &[u8]) -> Result<(), Error> {
        unsafe {
            let hkey = open_driver_key(instance_id, KEY_READ | KEY_WRITE)?;
            let name = wide_name(VBIOS_VALUE_NAME);
            let r = RegSetValueExW(
                hkey,
                name.as_ptr(),
                0,
                REG_BINARY,
                image.as_ptr().cast(),
                image.len() as u32,
            );
            let flushed = RegFlushKey(hkey);
            RegCloseKey(hkey);
            if r != 0 {
                return Err(Error::from(format!(
                    "RegSetValueExW(vbios) failed: {r} (admin required?)",
                )));
            }
            if flushed != 0 {
                return Err(Error::from(format!("RegFlushKey failed: {flushed}")));
            }
            Ok(())
        }
    }

    /// GPU 无关回执条目（纯 SetupDi 路径用——GPU 代码 43 / 驱动拒载时
    /// NVAPI 枚举不可用，vix loader 的 unload 同样不碰 GPU）。
    #[derive(Debug, Clone)]
    pub struct VbiosOverrideEntry {
        pub instance_id: String,
        pub description: String,
        /// 操作前值是否存在。
        pub had_value: bool,
        /// 是否真的删除了。
        pub cleared: bool,
    }

    /// GPU 无关清除：枚举全部 NVIDIA 显示适配器（DIGCF_PRESENT 仍含代码
    /// 43 的在位设备），对带 `vbios` 值的逐个删除。单管理员操作；返回
    /// 全部 NVIDIA 适配器的处置清单（含没值的——便于核对没漏）。
    pub fn clear_all_nvidia_overrides() -> Result<Vec<VbiosOverrideEntry>, Error> {
        let adapters = enumerate_display_adapters()?;
        let mut out = Vec::new();
        for a in adapters.into_iter().filter(|a| a.is_nvidia) {
            let had = matches!(read_override(&a.instance_id), Ok(Some(_)));
            let cleared = if had {
                clear_override(&a.instance_id)?
            } else {
                false
            };
            // mVolt direct-patch 链（romOverride00..FF）一并清——reset 语义=
            // 全恢复（vix unload 同样删 romOverride 族）。
            let _ = clear_romoverride_all(&a.instance_id);
            out.push(VbiosOverrideEntry {
                instance_id: a.instance_id,
                description: a.description,
                had_value: had,
                cleared,
            });
        }
        Ok(out)
    }

    /// 片段链合并报告。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct FragmentMergeReport {
        /// 最终链（保留片段在前按原序，新片段按给定序追加）。
        pub chain: Vec<Vec<u8>>,
        /// 原样保留的既有片段数。
        pub kept: usize,
        /// 被新片段覆盖的既有 offset 数。
        pub replaced: usize,
        /// 全新 offset 数。
        pub added: usize,
    }

    /// 按 offset 合并：既有链保留（同 offset 被新片段替换），新片段追加。
    /// 驱动按各片段 offset 独立应用，链内顺序不影响语义。
    pub fn merge_romoverride_chain(
        existing: &[RomOverrideFragment],
        new: &[(u32, Vec<u8>)],
    ) -> FragmentMergeReport {
        let new_offsets: std::collections::HashSet<u32> = new.iter().map(|(o, _)| *o).collect();
        let mut chain: Vec<Vec<u8>> = Vec::new();
        let mut kept = 0usize;
        let mut replaced = 0usize;
        for f in existing {
            if new_offsets.contains(&f.offset) {
                replaced += 1;
            } else {
                kept += 1;
                chain.push(f.raw.clone());
            }
        }
        let added = new.len();
        for (off, data) in new {
            chain.push(build_romoverride_fragment([0, 0, 0, 0], 0, *off, data));
        }
        FragmentMergeReport {
            chain,
            kept,
            replaced,
            added,
        }
    }

    /// 删除 override 值。返回是否真的删了（false = 值本来就不在，幂等）。
    /// **需要管理员**。
    pub fn clear_override(instance_id: &str) -> Result<bool, Error> {
        unsafe {
            let hkey = open_driver_key(instance_id, KEY_READ | KEY_WRITE)?;
            let name = wide_name(VBIOS_VALUE_NAME);
            let r = RegDeleteValueW(hkey, name.as_ptr());
            RegCloseKey(hkey);
            Ok(r == 0)
        }
    }

    /// 仅删除 `vbios` 值（不动 romOverride 链）——file 车道回退用。
    pub fn delete_vbios_value(instance_id: &str) -> Result<bool, Error> {
        unsafe {
            let hkey = open_driver_key(instance_id, KEY_READ | KEY_WRITE)?;
            let name = wide_name(VBIOS_VALUE_NAME);
            let r = RegDeleteValueW(hkey, name.as_ptr());
            RegFlushKey(hkey);
            RegCloseKey(hkey);
            Ok(r == 0)
        }
    }

    /// 定位实例 → (devinfo, devinfo data)。调用方负责
    /// SetupDiDestroyDeviceInfoList。
    fn open_adapter_devinfo(instance_id: &str) -> Result<(HDEVINFO, SP_DEVINFO_DATA), Error> {
        unsafe {
            let devinfo = SetupDiGetClassDevsW(
                &GUID_DEVCLASS_DISPLAY,
                std::ptr::null(),
                std::ptr::null_mut(),
                DIGCF_PRESENT,
            );
            if devinfo == 0 {
                return Err(Error::from("SetupDiGetClassDevsW failed"));
            }
            let want = wide_name(instance_id);
            let mut index = 0u32;
            loop {
                let mut dev = std::mem::zeroed::<SP_DEVINFO_DATA>();
                dev.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
                if SetupDiEnumDeviceInfo(devinfo, index, &mut dev) == 0 {
                    SetupDiDestroyDeviceInfoList(devinfo);
                    return Err(Error::from(format!(
                        "adapter instance {instance_id:?} not found (SetupDi)"
                    )));
                }
                let mut len = 0u32;
                SetupDiGetDeviceInstanceIdW(devinfo, &dev, std::ptr::null_mut(), 0, &mut len);
                let mut buf = vec![0u16; len as usize];
                if SetupDiGetDeviceInstanceIdW(devinfo, &dev, buf.as_mut_ptr(), len, &mut len) != 0
                    && buf[..len as usize]
                        .windows(1)
                        .take_while(|w| w[0] != 0)
                        .count()
                        == want.len().saturating_sub(1)
                {
                    // 宽字符比对（去掉结尾 NUL）。
                    let id: Vec<u16> = buf[..len as usize]
                        .iter()
                        .copied()
                        .take_while(|&c| c != 0)
                        .collect();
                    if id == want[..want.len() - 1] {
                        return Ok((devinfo, dev));
                    }
                }
                index += 1;
            }
        }
    }

    /// 设备管理器禁用/启用一轮（DICS_DISABLE → ~1.5s → DICS_ENABLE，
    /// CONFIGSPECIFIC 域）——驱动卸载/重载即重读 override。
    pub fn trigger_reinit(instance_id: &str) -> Result<(), Error> {
        let (devinfo, mut dev) = open_adapter_devinfo(instance_id)?;
        let r = unsafe {
            let mut propchange = std::mem::zeroed::<SP_PROPCHANGE_PARAMS>();
            propchange.ClassInstallHeader.cbSize =
                std::mem::size_of::<SP_CLASSINSTALL_HEADER>() as u32;
            propchange.ClassInstallHeader.InstallFunction = DIF_PROPERTYCHANGE;
            propchange.StateChange = DICS_DISABLE;
            propchange.Scope = DICS_FLAG_CONFIGSPECIFIC;
            propchange.HwProfile = 0;
            let set = SetupDiSetClassInstallParamsW(
                devinfo,
                &dev,
                &propchange.ClassInstallHeader,
                std::mem::size_of::<SP_PROPCHANGE_PARAMS>() as u32,
            );
            let dis = if set != 0 {
                SetupDiChangeState(devinfo, &mut dev)
            } else {
                0
            };
            if set != 0 && dis != 0 {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                propchange.StateChange = DICS_ENABLE;
                let set2 = SetupDiSetClassInstallParamsW(
                    devinfo,
                    &dev,
                    &propchange.ClassInstallHeader,
                    std::mem::size_of::<SP_PROPCHANGE_PARAMS>() as u32,
                );
                let en = if set2 != 0 {
                    SetupDiChangeState(devinfo, &mut dev)
                } else {
                    0
                };
                if set2 != 0 && en != 0 { Some(()) } else { None }
            } else {
                None
            }
        };
        unsafe {
            SetupDiDestroyDeviceInfoList(devinfo);
        }
        match r {
            Some(()) => Ok(()),
            None => Err(Error::from(
                "SetupDi property-change/disable-enable failed (admin required?)",
            )),
        }
    }

    /// 设备问题码：`Some(CM_PROB)` = DN_HAS_PROBLEM（code 43 = 0x31）；
    /// `None` = 健康（或设备不在位）。
    pub fn device_problem(instance_id: &str) -> Result<Option<u32>, Error> {
        let wide = wide_name(instance_id);
        let mut devinst = 0u32;
        let loc: CONFIGRET =
            unsafe { CM_Locate_DevNodeW(&mut devinst, wide.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) };
        if loc != CR_SUCCESS {
            return Ok(None);
        }
        let mut status = 0u32;
        let mut problem = 0u32;
        let r: CONFIGRET = unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, devinst, 0) };
        if r != CR_SUCCESS {
            return Ok(None);
        }
        if status & DN_HAS_PROBLEM != 0 {
            Ok(Some(problem))
        } else {
            Ok(None)
        }
    }
}

#[cfg(all(windows, test))]
mod fragment_merge_tests {
    use super::windows_impl::{
        RomOverrideFragment, build_romoverride_fragment, merge_romoverride_chain,
    };

    fn frag(offset: u32, data: &[u8]) -> RomOverrideFragment {
        RomOverrideFragment {
            name: String::new(),
            version_gate_a: 0,
            version_gate_b: 0,
            offset,
            len: data.len() as u8,
            data_hex: data.iter().map(|b| format!("{b:02x}")).collect(),
            raw: build_romoverride_fragment([0, 0, 0, 0], 0, offset, data),
        }
    }

    #[test]
    fn merge_keeps_replaces_adds() {
        // 既有链：mVolt 三片段（0x57B2E/0x57B30/0x57B94）
        let existing = vec![
            frag(0x57B2E, &[0x98, 0x92, 0x98, 0x00]),
            frag(0x57B30, &[0x98, 0x92, 0x98, 0x00]),
            frag(0x57B94, &[0x98, 0x92, 0x98, 0x00]),
        ];
        // 新：只改 board power（0x57B30）——OCP 两片段必须保留
        let r = merge_romoverride_chain(&existing, &[(0x57B30, vec![0xB0, 0x8E, 0x98, 0x00])]);
        assert_eq!(r.kept, 2);
        assert_eq!(r.replaced, 1);
        assert_eq!(r.added, 1);
        assert_eq!(r.chain.len(), 3);
        // 保留片段字节原样、按原序在前（0x57B2E、0x57B94），替换片段追加在后
        assert_eq!(r.chain[0], existing[0].raw);
        assert_eq!(r.chain[1], existing[2].raw);
        assert_eq!(r.chain[2][5..9], 0x57B30u32.to_le_bytes());
        assert_eq!(&r.chain[2][10..], &[0xB0, 0x8E, 0x98, 0x00]);
        // 全新 offset → 追加
        let r2 = merge_romoverride_chain(&existing, &[(0x60000, vec![1, 2, 3, 4])]);
        assert_eq!(
            (r2.kept, r2.replaced, r2.added, r2.chain.len()),
            (3, 0, 1, 4)
        );
    }
}
