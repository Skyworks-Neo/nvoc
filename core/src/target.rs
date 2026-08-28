use super::Error;
use ::nvapi::hi::Gpu;
use nvml_wrapper::Nvml;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GpuId(pub u32);

impl GpuId {
    pub fn from_pci_bus(bus: u32) -> Self {
        Self(bus.saturating_mul(256))
    }

    pub fn pci_bus(self) -> u32 {
        self.0 / 256
    }

    pub fn from_pci_address(address: PciAddress) -> Self {
        Self::from_pci_bus(address.bus)
    }

    pub fn from_pci_str(raw: &str) -> Result<Self, Error> {
        Ok(Self::from_pci_address(PciAddress::from_str(raw)?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PciAddress {
    pub domain: u32,
    pub bus: u32,
    pub device: u32,
    pub function: u32,
}

impl fmt::Display for PciAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:04x}:{:02x}:{:02x}.{}",
            self.domain, self.bus, self.device, self.function
        )
    }
}

impl FromStr for PciAddress {
    type Err = Error;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let input = raw.trim();
        if let Some(start) = input.find('(')
            && let Some(end) = input[start + 1..].find(')')
        {
            return parse_nvapi_pci_address(&input[start + 1..start + 1 + end]);
        }

        parse_standard_pci_address(input)
    }
}

fn parse_standard_pci_address(raw: &str) -> Result<PciAddress, Error> {
    let (domain_raw, rest) = raw
        .split_once(':')
        .ok_or_else(|| Error::Custom(format!("invalid PCI address {:?}", raw)))?;
    let (bus_raw, rest) = rest
        .split_once(':')
        .ok_or_else(|| Error::Custom(format!("invalid PCI address {:?}", raw)))?;
    let (device_raw, function_raw) = rest
        .split_once('.')
        .ok_or_else(|| Error::Custom(format!("invalid PCI address {:?}", raw)))?;

    Ok(PciAddress {
        domain: parse_pci_component(domain_raw, 16, "domain", raw)?,
        bus: parse_pci_component(bus_raw, 16, "bus", raw)?,
        device: parse_pci_component(device_raw, 16, "device", raw)?,
        function: parse_pci_component(function_raw, 10, "function", raw)?,
    })
}

fn parse_nvapi_pci_address(raw: &str) -> Result<PciAddress, Error> {
    let parts = raw.split(':').collect::<Vec<_>>();
    if parts.len() < 2 {
        return Err(Error::Custom(format!(
            "invalid NVAPI PCI address {:?}",
            raw
        )));
    }

    Ok(PciAddress {
        domain: 0,
        bus: parse_decimal_prefix(parts[0], "bus", raw)?,
        device: parse_decimal_prefix(parts[1], "device", raw)?,
        function: 0,
    })
}

fn parse_pci_component(raw: &str, radix: u32, label: &str, full: &str) -> Result<u32, Error> {
    u32::from_str_radix(raw.trim(), radix)
        .map_err(|_| Error::Custom(format!("invalid PCI {} in {:?}", label, full)))
}

fn parse_decimal_prefix(raw: &str, label: &str, full: &str) -> Result<u32, Error> {
    let trimmed = raw.trim();
    let digits = trimmed
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return Err(Error::Custom(format!(
            "invalid PCI {} in {:?}",
            label, full
        )));
    }
    digits
        .parse::<u32>()
        .map_err(|_| Error::Custom(format!("invalid PCI {} in {:?}", label, full)))
}

pub(crate) fn gpu_id_from_nvapi_gpu(gpu: &Gpu) -> GpuId {
    GpuId(gpu.id() as u32)
}

pub fn pci_address_from_nvml_device(
    device: &nvml_wrapper::Device<'_>,
) -> Result<PciAddress, Error> {
    let pci = device
        .pci_info()
        .map_err(|e| Error::Custom(format!("NVML pci_info failed: {:?}", e)))?;
    Ok(PciAddress {
        domain: pci.domain,
        bus: pci.bus,
        device: pci.device,
        function: 0,
    })
}

pub fn gpu_id_from_nvml_device(device: &nvml_wrapper::Device<'_>) -> Result<GpuId, Error> {
    Ok(GpuId::from_pci_address(pci_address_from_nvml_device(
        device,
    )?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendSet {
    Nvapi,
    Nvml,
    Both,
}

#[derive(Clone, Copy)]
pub struct GpuTarget<'a> {
    pub id: GpuId,
    pub index: usize,
    nvapi: Option<&'a Gpu>,
    nvml: Option<&'a Nvml>,
    /// Why `nvml` is `None` when the backend set requested NVML (`BackendSet::Both`):
    /// carries the original `Nvml::init()` failure reason so a later `nvml()` call
    /// can surface the root cause ("NVML init failed: ...") instead of the bare
    /// "has no NVML backend" message. `None` when NVML was never attempted
    /// (`BackendSet::Nvapi`) or when NVML initialized successfully.
    nvml_error: Option<&'a str>,
}

impl fmt::Debug for GpuTarget<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GpuTarget")
            .field("id", &self.id)
            .field("index", &self.index)
            .field("nvapi", &self.nvapi.is_some())
            .field("nvml", &self.nvml.is_some())
            .finish()
    }
}

impl<'a> GpuTarget<'a> {
    pub fn without_backends(id: GpuId, index: usize) -> Self {
        Self {
            id,
            index,
            nvapi: None,
            nvml: None,
            nvml_error: None,
        }
    }

    pub fn has_nvapi(&self) -> bool {
        self.nvapi.is_some()
    }

    pub fn has_nvml(&self) -> bool {
        self.nvml.is_some()
    }

    /// 原生 GC6 唤醒：force_gc6_exit（NVAPI 0x55590CB2，单参数、无 struct），
    /// 一次性把 GCOFF 掉电的 dGPU 拉回 D0（610+ 移动驱动空闲 5-20s 即进 GC6）。
    /// 唤醒非持久 —— 空闲后仍会重新掉电，因此只对紧随其后的操作有效。
    /// 桌面端（无 GC6）驱动返回 NoImplementation(-104) 等错误，由调用方决定
    /// 按 best-effort 忽略还是上报。
    ///
    /// NVAPI 写操作通常无需手动调用：core::operation::run 已按
    /// GpuType::needs_gc6_wake() 自动预唤醒。本方法供读操作序列、长驻进程
    /// 的显式唤醒等场景使用（auto-optimizer 的原生唤醒即走此入口）。
    pub fn force_wake(&self) -> Result<(), Error> {
        self.nvapi()?.force_gc6_exit().map_err(Error::from)
    }

    pub(crate) fn nvapi(&self) -> Result<&'a Gpu, Error> {
        self.nvapi
            .ok_or_else(|| Error::Custom(format!("GPU {} has no NvAPI backend", self.id.0)))
    }

    pub fn nvml(&self) -> Result<&'a Nvml, Error> {
        self.nvml.ok_or_else(|| {
            // Preserve the root cause when NVML was requested but unavailable
            // (degraded `BackendSet::Both`): the original `Nvml::init()` error
            // is far more actionable than a bare "has no NVML backend".
            match self.nvml_error {
                Some(reason) => {
                    Error::Custom(format!("GPU {} has no NVML backend ({reason})", self.id.0))
                }
                None => Error::Custom(format!("GPU {} has no NVML backend", self.id.0)),
            }
        })
    }
}

pub struct TargetInventory {
    nvml: Option<Nvml>,
    nvapi_gpus: Vec<Gpu>,
    nvml_ids: Vec<u32>,
    /// Root-cause string stashed when `BackendSet::Both` degraded to NVAPI-only
    /// because `Nvml::init()` failed. Surfaced by [`GpuTarget::nvml`] so callers
    /// that genuinely need NVML see "NVML init failed: ..." rather than a bare
    /// "has no NVML backend". `None` for `BackendSet::Nvapi` (never attempted),
    /// `BackendSet::Nvml` (init failure is a hard error, never stored), or a
    /// healthy `BackendSet::Both`.
    nvml_error: Option<String>,
}

impl TargetInventory {
    pub fn discover(backends: BackendSet) -> Result<Self, Error> {
        let nvapi_gpus = match backends {
            BackendSet::Nvapi | BackendSet::Both => super::gpu::get_sorted_gpus()?,
            BackendSet::Nvml => Vec::new(),
        };

        // NVML availability policy:
        //  - `Nvml` (explicit `--nvml` / NVML-only commands): a missing NVML is
        //    a real error — the user asked for NVML, so surface it and stop.
        //  - `Both` (the auto/default path, and every `--nvapi` "augment"
        //    command): NVML is best-effort. NVAPI already enumerated the GPUs
        //    above and GPU identity is PCI-bus-based, so when `Nvml::init()`
        //    fails we degrade to an NVAPI-only inventory (nvml: None) instead
        //    of failing the whole discovery — commands that augment with NVML
        //    data guard on `target.nvml()`/`has_nvml()` and simply omit those
        //    fields. This is what unblocks `get-info`, `get-status`, GUI
        //    discovery, etc. on machines with no NVML binding.
        //  - `Nvapi`: never attempt NVML.
        let (nvml, nvml_error) = match backends {
            BackendSet::Nvml => (
                Some(
                    Nvml::init()
                        .map_err(|e| Error::Custom(format!("NVML init failed: {:?}", e)))?,
                ),
                None,
            ),
            BackendSet::Both => match Nvml::init() {
                Ok(nvml) => (Some(nvml), None),
                Err(e) => {
                    let reason = format!("NVML init failed: {:?}", e);
                    eprintln!("warning: {reason}; continuing with NVAPI-only backends");
                    (None, Some(reason))
                }
            },
            BackendSet::Nvapi => (None, None),
        };

        let nvml_ids = match &nvml {
            Some(nvml) => super::gpu::get_sorted_gpu_ids_nvml(nvml)?,
            None => Vec::new(),
        };

        Ok(Self {
            nvml,
            nvapi_gpus,
            nvml_ids,
            nvml_error,
        })
    }

    pub fn targets(&self) -> Vec<GpuTarget<'_>> {
        let mut ids = self
            .nvapi_gpus
            .iter()
            .map(|gpu| gpu_id_from_nvapi_gpu(gpu).0)
            .chain(self.nvml_ids.iter().copied())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();

        ids.into_iter()
            .enumerate()
            .map(|(index, id)| GpuTarget {
                id: GpuId(id),
                index,
                nvapi: self
                    .nvapi_gpus
                    .iter()
                    .find(|gpu| gpu_id_from_nvapi_gpu(gpu).0 == id),
                nvml: self.nvml.as_ref(),
                nvml_error: self.nvml_error.as_deref(),
            })
            .collect()
    }

    pub fn target_by_id(&self, id: GpuId) -> Result<GpuTarget<'_>, Error> {
        self.targets()
            .into_iter()
            .find(|target| target.id == id)
            .ok_or_else(|| Error::Custom(format!("GPU {} not found", id.0)))
    }

    pub fn target_by_pci_str(&self, raw: &str) -> Result<GpuTarget<'_>, Error> {
        self.target_by_id(GpuId::from_pci_str(raw)?)
    }
}

pub fn discover_targets(backends: BackendSet) -> Result<TargetInventory, Error> {
    TargetInventory::discover(backends)
}

pub fn select_targets<'a>(
    targets: &'a [GpuTarget<'a>],
    selector: &super::gpu::GpuSelector,
) -> Result<Vec<GpuTarget<'a>>, Error> {
    let ids = targets.iter().map(|target| target.id.0).collect::<Vec<_>>();
    let selected_ids = super::gpu::select_gpu_ids(&ids, selector)?;
    Ok(selected_ids
        .into_iter()
        .filter_map(|id| targets.iter().find(|target| target.id.0 == id).copied())
        .collect())
}
