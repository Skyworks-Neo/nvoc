use super::Error;
use nvapi_hi::Gpu;
use nvml_wrapper::Nvml;
use std::str::FromStr;

/// GPU selection specification, independent of command dispatch.
pub struct GpuSelector(Option<Vec<String>>);

impl GpuSelector {
    /// Select all available GPUs.
    pub fn all() -> Self {
        Self(None)
    }

    /// Select GPUs by decimal or hex index / GPU id strings.
    pub fn from_specs(specs: impl IntoIterator<Item = String>) -> Self {
        Self(Some(specs.into_iter().collect()))
    }

    /// Build from clap values at the CLI boundary.
    pub fn from_clap(values: Option<clap::parser::ValuesRef<'_, String>>) -> Self {
        Self(values.map(|v| v.cloned().collect()))
    }

    fn specs(&self) -> Option<&[String]> {
        self.0.as_deref()
    }
}

pub fn single_gpu<'a>(gpus: &[&'a Gpu]) -> anyhow::Result<&'a Gpu, Error> {
    let mut gpus = gpus.iter();
    gpus.next()
        .ok_or_else(|| Error::from("no GPU selected"))
        .and_then(|g| match gpus.next() {
            None => Ok(*g),
            Some(..) => Err(Error::from("multiple GPUs selected")),
        })
}

fn parse_gpu_id(raw: &str) -> anyhow::Result<usize> {
    let raw = raw.trim();

    if let Some(rest) = raw.strip_prefix("pu=").or_else(|| raw.strip_prefix("pu ")) {
        anyhow::bail!(
            "invalid GPU id {:?} -- did you mean --gpu={}?",
            raw,
            rest.trim()
        );
    }

    if !raw.starts_with(|c: char| c.is_ascii_digit()) {
        anyhow::bail!(
            "invalid GPU id {:?}: expected a decimal or hex (0x...) number",
            raw
        );
    }

    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        usize::from_str_radix(hex, 16).map_err(|_| anyhow::anyhow!("invalid hex GPU id {:?}", raw))
    } else {
        usize::from_str(raw).map_err(|_| anyhow::anyhow!("invalid decimal GPU id {:?}", raw))
    }
}

pub fn select_gpus<'a>(
    gpus: &'a [Gpu],
    selector: &GpuSelector,
) -> anyhow::Result<Vec<&'a Gpu>, Error> {
    let selected = match selector.specs() {
        Some(specs) => {
            let inputs = specs
                .iter()
                .map(|s| parse_gpu_id(s.as_str()))
                .collect::<anyhow::Result<Vec<_>, _>>()
                .map_err(|e| Error::Custom(e.to_string()))?;

            let mut selected = Vec::new();
            for input in inputs {
                if input < 256 {
                    let gpu = gpus.get(input).ok_or_else(|| {
                        Error::Custom(format!(
                            "no GPU matches --gpu {}; use `nvoc list` to see available indices",
                            input
                        ))
                    })?;
                    selected.push(gpu);
                    continue;
                }

                if let Some(gpu) = gpus.iter().find(|gpu| gpu.id() == input) {
                    selected.push(gpu);
                    continue;
                }

                let legacy = input << 8;
                if let Some(gpu) = gpus.iter().find(|gpu| gpu.id() == legacy) {
                    selected.push(gpu);
                    continue;
                }

                return Err(Error::Custom(format!(
                    "no GPU matches --gpu {}; use `nvoc list` to see available indices",
                    input
                )));
            }
            selected
        }
        None => gpus.iter().collect(),
    };

    if selected.is_empty() {
        Err(Error::DeviceNotFound)
    } else {
        Ok(selected)
    }
}

pub fn get_sorted_gpus() -> nvapi_hi::Result<Vec<Gpu>> {
    let mut gpus = Gpu::enumerate()?;
    gpus.sort_by_key(|g| g.id());
    Ok(gpus)
}

pub fn get_sorted_gpu_ids_nvml(nvml: &Nvml) -> Result<Vec<u32>, Error> {
    let count = nvml
        .device_count()
        .map_err(|e| Error::Custom(format!("NVML device_count failed: {:?}", e)))?;

    let mut gpu_ids = Vec::new();
    for i in 0..count {
        let device = nvml
            .device_by_index(i)
            .map_err(|e| Error::Custom(format!("NVML device_by_index({}) failed: {:?}", i, e)))?;
        let pci = device
            .pci_info()
            .map_err(|e| Error::Custom(format!("NVML pci_info({}) failed: {:?}", i, e)))?;

        gpu_ids.push(pci.bus.saturating_mul(256));
    }

    gpu_ids.sort_unstable();
    gpu_ids.dedup();
    Ok(gpu_ids)
}

pub fn select_gpu_ids(gpu_ids: &[u32], selector: &GpuSelector) -> anyhow::Result<Vec<u32>, Error> {
    let selected = match selector.specs() {
        Some(specs) => {
            let inputs = specs
                .iter()
                .map(|s| parse_gpu_id(s.as_str()))
                .collect::<anyhow::Result<Vec<_>, _>>()
                .map_err(|e| Error::Custom(e.to_string()))?;

            let mut selected = Vec::new();
            for input in inputs {
                if input < 256 {
                    let id = gpu_ids.get(input).ok_or_else(|| {
                        Error::Custom(format!(
                            "no GPU matches --gpu {}; use `nvoc list` to see available indices",
                            input
                        ))
                    })?;
                    selected.push(*id);
                    continue;
                }

                if let Some(&id) = gpu_ids.iter().find(|&&id| id as usize == input) {
                    selected.push(id);
                    continue;
                }

                let legacy = (input as u32) << 8;
                if let Some(&id) = gpu_ids.iter().find(|&&id| id == legacy) {
                    selected.push(id);
                    continue;
                }

                return Err(Error::Custom(format!(
                    "no GPU matches --gpu {}; use `nvoc list` to see available indices",
                    input
                )));
            }
            selected
        }
        None => gpu_ids.to_vec(),
    };

    if selected.is_empty() {
        Err(Error::DeviceNotFound)
    } else {
        Ok(selected)
    }
}
