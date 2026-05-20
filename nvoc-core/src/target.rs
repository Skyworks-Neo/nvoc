use super::Error;
use nvapi_hi::Gpu;
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

pub fn gpu_id_from_nvapi_gpu(gpu: &Gpu) -> GpuId {
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
    pub nvapi: Option<&'a Gpu>,
    pub nvml: Option<&'a Nvml>,
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
    pub fn nvapi(&self) -> Result<&'a Gpu, Error> {
        self.nvapi
            .ok_or_else(|| Error::Custom(format!("GPU {} has no NvAPI backend", self.id.0)))
    }

    pub fn nvml(&self) -> Result<&'a Nvml, Error> {
        self.nvml
            .ok_or_else(|| Error::Custom(format!("GPU {} has no NVML backend", self.id.0)))
    }
}

pub struct TargetInventory {
    nvml: Option<Nvml>,
    nvapi_gpus: Vec<Gpu>,
    nvml_ids: Vec<u32>,
}

impl TargetInventory {
    pub fn discover(backends: BackendSet) -> Result<Self, Error> {
        let nvapi_gpus = match backends {
            BackendSet::Nvapi | BackendSet::Both => super::gpu::get_sorted_gpus()?,
            BackendSet::Nvml => Vec::new(),
        };

        let nvml = match backends {
            BackendSet::Nvml | BackendSet::Both => Some(
                Nvml::init().map_err(|e| Error::Custom(format!("NVML init failed: {:?}", e)))?,
            ),
            BackendSet::Nvapi => None,
        };

        let nvml_ids = match &nvml {
            Some(nvml) => super::gpu::get_sorted_gpu_ids_nvml(nvml)?,
            None => Vec::new(),
        };

        Ok(Self {
            nvml,
            nvapi_gpus,
            nvml_ids,
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

    pub fn target_by_nvapi_gpu(&self, gpu: &Gpu) -> Result<GpuTarget<'_>, Error> {
        self.target_by_id(gpu_id_from_nvapi_gpu(gpu))
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
