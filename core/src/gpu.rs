use super::Error;
use super::target::gpu_id_from_nvml_device;
use ::nvapi::hi::Gpu;
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

    fn specs(&self) -> Option<&[String]> {
        self.0.as_deref()
    }
}

fn parse_gpu_id(raw: &str) -> Result<usize, Error> {
    let raw = raw.trim();

    if let Some(rest) = raw.strip_prefix("pu=").or_else(|| raw.strip_prefix("pu ")) {
        return Err(Error::Custom(format!(
            "invalid GPU id {:?} -- did you mean --gpu={}?",
            raw,
            rest.trim()
        )));
    }

    if !raw.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(Error::Custom(format!(
            "invalid GPU id {:?}: expected a decimal or hex (0x...) number",
            raw
        )));
    }

    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        usize::from_str_radix(hex, 16)
            .map_err(|_| Error::Custom(format!("invalid hex GPU id {:?}", raw)))
    } else {
        usize::from_str(raw).map_err(|_| Error::Custom(format!("invalid decimal GPU id {:?}", raw)))
    }
}

/// Explicitly initialize NVAPI exactly once before first use. nvapi-rs relies
/// on the driver's implicit initialization, which fails on some old/legacy
/// drivers where tools that call NvAPI_Initialize up front (MSI Afterburner,
/// the ref tool plugin) still work — so we call it explicitly, like they do.
/// Failure is non-fatal: enumeration proceeds via the implicit path, which is
/// enough on every modern driver.
fn ensure_nvapi_initialized() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        if let Err(e) = ::nvapi::hi::initialize() {
            eprintln!("warning: NvAPI_Initialize failed ({e:?}); continuing via implicit init");
        }
    });
}

pub fn get_sorted_gpus() -> ::nvapi::hi::Result<Vec<Gpu>> {
    ensure_nvapi_initialized();
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
        gpu_ids.push(gpu_id_from_nvml_device(&device)?.0);
    }

    gpu_ids.sort_unstable();
    gpu_ids.dedup();
    Ok(gpu_ids)
}

pub fn select_gpu_ids(gpu_ids: &[u32], selector: &GpuSelector) -> Result<Vec<u32>, Error> {
    let selected = match selector.specs() {
        Some(specs) => {
            let inputs = specs
                .iter()
                .map(|s| parse_gpu_id(s.as_str()))
                .collect::<Result<Vec<_>, _>>()?;

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
