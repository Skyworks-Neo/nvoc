use anstream::eprintln;
#[cfg(any(feature = "cuda", feature = "vulkan"))]
use anstream::println;
use clap::Parser;
#[cfg(feature = "cuda")]
use clap::{CommandFactory, FromArgMatches, parser::ValueSource};
#[cfg(feature = "cuda")]
use std::collections::HashMap;
#[cfg(feature = "cuda")]
use std::fs;
use style::stylize;
#[cfg(feature = "cuda")]
use style::stylize_title;

#[cfg(feature = "cuda")]
use cli_stressor_cuda_rs::parse_int_list;

#[cfg(feature = "cuda")]
use cli_stressor_cuda_rs::{
    Backend, DeviceInfo, KernelParamOverride, KernelType, PrecisionKind, PrecisionMixtureEntry,
    PrecisionSpec, StressResult, StressRunConfig, parse_kernel_mixture,
    parse_kernel_param_overrides, parse_kernel_type, parse_kernel_type_list, parse_precision_list,
    parse_stream_mode, run_stress_mixed,
};
#[cfg(feature = "cuda")]
use serde::Deserialize;

mod style;

#[cfg(feature = "cuda")]
mod cuda_backend;

#[cfg(feature = "cuda")]
use cuda_backend::{
    enumerate_cuda_devices, resolve_device_index_by_pci_bus, resolve_device_index_by_sorted_index,
    resolve_device_index_by_uuid,
};

// Stressor Vulkan engine (optional). Only compiled when the crate is built with
// --features "vulkan" in addition to "cuda".
#[cfg(feature = "vulkan")]
#[path = "vulkan_gfx_stressor.rs"]
mod vulkan_gfx_stressor;
#[cfg(all(feature = "cuda", feature = "vulkan"))]
use vulkan_gfx_stressor::VulkanDeviceSelection;
#[cfg(feature = "vulkan")]
use vulkan_gfx_stressor::VulkanGraphicsEngine;

#[cfg(feature = "vulkan")]
fn run_vulkan_for_duration(duration_s: f64) -> i32 {
    println!(
        "{}",
        stylize(
            &format!(
                "Vulkan-only mode: running Vulkan engine for {:.1}s",
                duration_s
            ),
            false
        )
    );

    let mut eng = VulkanGraphicsEngine::new();
    if let Err(e) = eng.start_stress_thread() {
        eprintln!(
            "{}",
            stylize(
                &format!("Failed to start VulkanGraphicsEngine: {}", e),
                true
            )
        );
        return 1;
    }

    let err_flag = eng.get_error_flag_arc();
    let started = std::time::Instant::now();
    while started.elapsed().as_secs_f64() < duration_s {
        if err_flag.load(std::sync::atomic::Ordering::SeqCst) {
            eprintln!(
                "{}",
                stylize("[FATAL] Vulkan engine reported an error; exiting", true)
            );
            let _ = eng.stop();
            return 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    if let Err(e) = eng.stop() {
        eprintln!(
            "{}",
            stylize(&format!("[FATAL] Vulkan engine stop failed: {}", e), true)
        );
        return 1;
    }
    println!("{}", stylize("Vulkan-only run finished.", false));
    0
}

#[derive(Parser, Debug)]
#[command(
    name = "cli-stressor-cuda-rs",
    about = "GPU core-domain stressor (Rust): mixed kernel-path stress + validation sidecar"
)]
struct Args {
    #[arg(
        long,
        help = "Optional TOML config file. Supports [kernel_params.<kernel>] for per-kernel precisions/precision_mixture/matrix_sizes/warmup_iters/burst_iters/transpose_prob/minor_mixture_rate"
    )]
    config: Option<String>,

    #[arg(long, default_value_t = 90.0)]
    duration: f64,

    #[arg(long, default_value = "2049,4096,4097,8192,8193,16384")]
    matrix_sizes: String,

    #[arg(long, default_value = "2048,4096")]
    fp64_matrix_sizes: String,

    #[arg(long, default_value = "fp16,bf16")]
    precisions: String,

    #[arg(long, default_value_t = 3)]
    warmup_iters: u32,

    #[arg(long, default_value_t = 6)]
    burst_iters: u32,

    #[arg(long, default_value_t = 10.0)]
    validate_interval: f64,

    #[arg(long, default_value_t = 1024)]
    validate_size: usize,

    #[arg(long, default_value_t = 0.5)]
    transpose_prob: f64,

    #[arg(long, default_value_t = 0.15)]
    minor_mixture_rate: f64,

    #[arg(long, default_value_t = 12345)]
    seed: u64,

    #[arg(
        long,
        default_value = "gemm,memcpy,memset,transpose,elementwise,reduction,atomic",
        help = "Enabled kernel paths (comma-separated): gemm,memcpy,memset,transpose,elementwise,reduction,atomic"
    )]
    kernel_types: String,

    #[arg(
        long,
        default_value = "",
        help = "Kernel mixture weights as type:weight pairs, e.g. gemm:0.5,memcpy:0.3,reduction:0.2 (empty = equal weights)"
    )]
    kernel_mixture: String,

    #[arg(
        long,
        default_value = "",
        help = "Per-kernel overrides, e.g. 'gemm:precisions=fp16|bf16,precision_mixture=fp16:0.7|bf16:0.3,matrix_sizes=2049|4096,warmup=4,burst=8;memcpy:matrix_sizes=8192|16384,burst=64'"
    )]
    kernel_params: String,

    #[arg(
        long,
        default_value = "single",
        help = "Submission stream mode: single|dual|triple"
    )]
    stream_mode: String,

    #[arg(long)]
    disable_fp8: bool,
    /// Enable the Vulkan graphics stressor thread (optional)
    #[arg(long, default_value_t = false)]
    enable_vulkan_stress: bool,

    /// Run Vulkan-only stress (skip CUDA workload)
    #[arg(long, default_value_t = false)]
    vulkan_only: bool,

    /// CUDA GPU index in PCI-bus-sorted order (0-based)
    #[arg(
        long,
        value_name = "INDEX",
        help = "CUDA GPU index in PCI-bus-sorted order (0-based, default: 0)"
    )]
    gpu_index: Option<u32>,

    /// CUDA GPU PCI bus address (e.g., 0001:01:00.0)
    #[arg(
        long,
        value_name = "DOMAIN:BUS:DEVICE.FUNCTION",
        help = "CUDA GPU PCI bus address (e.g., 0001:01:00.0)"
    )]
    pci_bus: Option<String>,

    /// CUDA GPU UUID (128-bit hex string)
    #[arg(
        long,
        value_name = "UUID",
        help = "CUDA GPU UUID (32 hex digits or space/dash separated)"
    )]
    gpu_uuid: Option<String>,

    /// List CUDA GPUs (PCI-sorted index, CUDA index, PCI bus, UUID) and exit
    #[arg(long, default_value_t = false)]
    list_gpus: bool,
}

#[cfg(feature = "cuda")]
#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    duration: Option<f64>,
    matrix_sizes: Option<Vec<usize>>,
    fp64_matrix_sizes: Option<Vec<usize>>,
    precisions: Option<Vec<String>>,
    warmup_iters: Option<u32>,
    burst_iters: Option<u32>,
    validate_interval: Option<f64>,
    validate_size: Option<usize>,
    transpose_prob: Option<f64>,
    minor_mixture_rate: Option<f64>,
    seed: Option<u64>,
    kernel_types: Option<Vec<String>>,
    kernel_mixture: Option<KernelMixtureConfig>,
    stream_mode: Option<String>,
    disable_fp8: Option<bool>,
    enable_vulkan_stress: Option<bool>,
    #[serde(alias = "vulkan-only")]
    vulkan_only: Option<bool>,
    kernel_params: Option<HashMap<String, FileKernelParam>>,
    gpu_index: Option<u32>,
    pci_bus: Option<String>,
    gpu_uuid: Option<String>,
    list_gpus: Option<bool>,
}

#[cfg(feature = "cuda")]
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum KernelMixtureConfig {
    Text(String),
    Map(HashMap<String, f64>),
}

#[cfg(feature = "cuda")]
#[derive(Debug, Default, Deserialize)]
struct FileKernelParam {
    precisions: Option<Vec<String>>,
    #[serde(alias = "precision_weights")]
    precision_weight: Option<Vec<f64>>,
    precision_mixture: Option<HashMap<String, f64>>,
    matrix_sizes: Option<Vec<usize>>,
    warmup_iters: Option<u32>,
    burst_iters: Option<u32>,
    transpose_prob: Option<f64>,
    minor_mixture_rate: Option<f64>,
}

#[cfg(feature = "cuda")]
fn precision_mixture_from_weights(
    precisions: &[PrecisionSpec],
    weights: &[f64],
    context: &str,
) -> Result<Vec<PrecisionMixtureEntry>, String> {
    if precisions.is_empty() {
        return Err(format!("{context} precisions cannot be empty"));
    }
    if precisions.len() != weights.len() {
        return Err(format!(
            "{context} precision_weight length ({}) must match precisions ({})",
            weights.len(),
            precisions.len()
        ));
    }
    let mut entries = Vec::with_capacity(precisions.len());
    for (spec, weight) in precisions.iter().zip(weights.iter()) {
        if !weight.is_finite() || *weight < 0.0 {
            return Err(format!(
                "{context} precision_weight must be finite and >= 0: {weight}"
            ));
        }
        entries.push(PrecisionMixtureEntry {
            spec: *spec,
            weight: *weight,
        });
    }
    Ok(entries)
}

#[cfg(feature = "cuda")]
fn precision_mixture_from_map(
    map: &HashMap<String, f64>,
    context: &str,
) -> Result<Vec<PrecisionMixtureEntry>, String> {
    if map.is_empty() {
        return Err(format!("{context} precision_mixture cannot be empty"));
    }
    let mut entries = Vec::with_capacity(map.len());
    for (name, weight) in map {
        if !weight.is_finite() || *weight < 0.0 {
            return Err(format!(
                "{context} precision_mixture must be finite and >= 0: {weight}"
            ));
        }
        let specs = parse_precision_list(name)?;
        if specs.len() != 1 {
            return Err(format!(
                "{context} expected a single precision, got: {name}"
            ));
        }
        let spec = specs[0];
        entries.push(PrecisionMixtureEntry {
            spec,
            weight: *weight,
        });
    }
    Ok(entries)
}

#[cfg(feature = "cuda")]
fn load_file_config(path: &str) -> Result<FileConfig, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("failed to read config: {e}"))?;
    toml::from_str(&raw).map_err(|e| format!("invalid TOML: {e}"))
}

#[cfg(feature = "cuda")]
fn load_kernel_overrides_from_config(
    parsed: &FileConfig,
) -> Result<Vec<KernelParamOverride>, String> {
    let mut out = Vec::new();
    if let Some(kernel_params) = &parsed.kernel_params {
        for (name, item) in kernel_params {
            let kind = parse_kernel_type(name)?;
            if let Some(sizes) = &item.matrix_sizes
                && sizes.is_empty()
            {
                return Err(format!("kernel_params.{name}.matrix_sizes cannot be empty"));
            }
            if item.precision_mixture.is_some() && item.precision_weight.is_some() {
                return Err(format!(
                    "kernel_params.{name} cannot set both precision_mixture and precision_weight"
                ));
            }
            let precisions = if let Some(list) = &item.precisions {
                if list.is_empty() {
                    None
                } else {
                    let joined = list.join(",");
                    Some(parse_precision_list(&joined)?)
                }
            } else {
                None
            };
            let precision_mixture = if let Some(map) = &item.precision_mixture {
                Some(precision_mixture_from_map(
                    map,
                    &format!("kernel_params.{name}.precision_mixture"),
                )?)
            } else if let Some(weights) = &item.precision_weight {
                let specs = precisions.as_ref().ok_or_else(|| {
                    format!("kernel_params.{name}.precision_weight requires precisions")
                })?;
                Some(precision_mixture_from_weights(
                    specs,
                    weights,
                    &format!("kernel_params.{name}.precision_weight"),
                )?)
            } else {
                None
            };
            out.push(KernelParamOverride {
                kind,
                precisions,
                precision_mixture,
                matrix_sizes: item.matrix_sizes.clone(),
                warmup_iters: item.warmup_iters,
                burst_iters: item.burst_iters,
                transpose_prob: item.transpose_prob,
                minor_mixture_rate: item.minor_mixture_rate,
            });
        }
    }
    Ok(out)
}

#[cfg(feature = "cuda")]
fn merge_kernel_overrides(
    base: Vec<KernelParamOverride>,
    cli: Vec<KernelParamOverride>,
) -> Vec<KernelParamOverride> {
    let mut merged = base;
    for item in cli {
        if let Some(idx) = merged.iter().position(|v| v.kind == item.kind) {
            if item.precisions.is_some() {
                merged[idx].precisions = item.precisions;
            }
            if item.precision_mixture.is_some() {
                merged[idx].precision_mixture = item.precision_mixture;
            }
            if item.matrix_sizes.is_some() {
                merged[idx].matrix_sizes = item.matrix_sizes;
            }
            if item.warmup_iters.is_some() {
                merged[idx].warmup_iters = item.warmup_iters;
            }
            if item.burst_iters.is_some() {
                merged[idx].burst_iters = item.burst_iters;
            }
            if item.transpose_prob.is_some() {
                merged[idx].transpose_prob = item.transpose_prob;
            }
            if item.minor_mixture_rate.is_some() {
                merged[idx].minor_mixture_rate = item.minor_mixture_rate;
            }
        } else {
            merged.push(item);
        }
    }
    merged
}

#[cfg(feature = "cuda")]
fn parse_args_with_cli_sources() -> (Args, std::collections::HashSet<&'static str>) {
    let cmd = Args::command();
    let matches = cmd.get_matches();
    let args = Args::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let mut cli_set = std::collections::HashSet::new();
    for id in [
        "config",
        "duration",
        "matrix_sizes",
        "fp64_matrix_sizes",
        "precisions",
        "warmup_iters",
        "burst_iters",
        "validate_interval",
        "validate_size",
        "transpose_prob",
        "minor_mixture_rate",
        "seed",
        "kernel_types",
        "kernel_mixture",
        "kernel_params",
        "enable_vulkan_stress",
        "vulkan_only",
        "stream_mode",
        "disable_fp8",
        "gpu_index",
        "pci_bus",
        "gpu_uuid",
        "list_gpus",
    ] {
        if matches.value_source(id) == Some(ValueSource::CommandLine) {
            cli_set.insert(id);
        }
    }
    (args, cli_set)
}

#[cfg(feature = "cuda")]
fn apply_file_config_to_args(
    args: &mut Args,
    cli_set: &std::collections::HashSet<&'static str>,
    parsed: Option<&FileConfig>,
) -> Result<(), String> {
    let Some(parsed) = parsed else {
        return Ok(());
    };

    if let (true, Some(v)) = (!cli_set.contains("duration"), parsed.duration) {
        args.duration = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("matrix_sizes"), &parsed.matrix_sizes) {
        args.matrix_sizes = v
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(",");
    }
    if let (true, Some(v)) = (
        !cli_set.contains("fp64_matrix_sizes"),
        &parsed.fp64_matrix_sizes,
    ) {
        args.fp64_matrix_sizes = v
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(",");
    }
    if let (true, Some(v)) = (!cli_set.contains("precisions"), &parsed.precisions) {
        args.precisions = v.join(",");
    }
    if let (true, Some(v)) = (!cli_set.contains("warmup_iters"), parsed.warmup_iters) {
        args.warmup_iters = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("burst_iters"), parsed.burst_iters) {
        args.burst_iters = v;
    }
    if let (true, Some(v)) = (
        !cli_set.contains("validate_interval"),
        parsed.validate_interval,
    ) {
        args.validate_interval = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("validate_size"), parsed.validate_size) {
        args.validate_size = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("transpose_prob"), parsed.transpose_prob) {
        args.transpose_prob = v;
    }
    if let (true, Some(v)) = (
        !cli_set.contains("minor_mixture_rate"),
        parsed.minor_mixture_rate,
    ) {
        args.minor_mixture_rate = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("seed"), parsed.seed) {
        args.seed = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("kernel_types"), &parsed.kernel_types) {
        args.kernel_types = v.join(",");
    }
    if let (true, Some(v)) = (!cli_set.contains("kernel_mixture"), &parsed.kernel_mixture) {
        args.kernel_mixture = match v {
            KernelMixtureConfig::Text(s) => s.clone(),
            KernelMixtureConfig::Map(m) => {
                let mut parts = Vec::with_capacity(m.len());
                for (k, w) in m {
                    parts.push(format!("{k}:{w}"));
                }
                parts.join(",")
            }
        };
    }
    if let (true, Some(v)) = (!cli_set.contains("stream_mode"), &parsed.stream_mode) {
        args.stream_mode = v.clone();
    }
    if let (true, Some(v)) = (!cli_set.contains("disable_fp8"), parsed.disable_fp8) {
        args.disable_fp8 = v;
    }
    if let (true, Some(v)) = (
        !cli_set.contains("enable_vulkan_stress"),
        parsed.enable_vulkan_stress,
    ) {
        args.enable_vulkan_stress = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("vulkan_only"), parsed.vulkan_only) {
        args.vulkan_only = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("gpu_index"), parsed.gpu_index) {
        args.gpu_index = Some(v);
    }
    if let (true, Some(v)) = (!cli_set.contains("pci_bus"), &parsed.pci_bus) {
        args.pci_bus = Some(v.clone());
    }
    if let (true, Some(v)) = (!cli_set.contains("gpu_uuid"), &parsed.gpu_uuid) {
        args.gpu_uuid = Some(v.clone());
    }
    if let (true, Some(v)) = (!cli_set.contains("list_gpus"), parsed.list_gpus) {
        args.list_gpus = v;
    }
    Ok(())
}

#[cfg(feature = "cuda")]
fn resolve_gpu_device_index(args: &Args) -> Result<u32, String> {
    // Count how many selection methods are provided
    let has_gpu_index = args.gpu_index.is_some();
    let has_pci_bus = args.pci_bus.is_some();
    let has_gpu_uuid = args.gpu_uuid.is_some();

    let count = [has_gpu_index, has_pci_bus, has_gpu_uuid]
        .iter()
        .filter(|x| **x)
        .count();

    if count > 1 {
        return Err(
            "only one of --gpu-index, --pci-bus, or --gpu-uuid can be specified".to_string(),
        );
    }

    if let Some(index) = args.gpu_index {
        // gpu-index is interpreted as index in PCI-bus-sorted CUDA device list.
        println!(
            "{}",
            stylize(
                &format!("[GPU] Using PCI-sorted CUDA index: {}", index),
                false
            )
        );
        return resolve_device_index_by_sorted_index(index);
    }

    if let Some(ref pci_str) = args.pci_bus {
        println!(
            "{}",
            stylize(&format!("[GPU] Using PCI bus ID: {}", pci_str), false)
        );
        let pci = parse_pci_bus_string(pci_str)?;
        return resolve_device_index_by_pci_bus(pci);
    }

    if let Some(ref uuid_str) = args.gpu_uuid {
        println!(
            "{}",
            stylize(&format!("[GPU] Using UUID: {}", uuid_str), false)
        );
        let uuid = parse_uuid_string(uuid_str)?;
        return resolve_device_index_by_uuid(uuid);
    }

    // Default: use index 0 in PCI-sorted order.
    println!(
        "{}",
        stylize("[GPU] Using default PCI-sorted CUDA index: 0", false)
    );
    resolve_device_index_by_sorted_index(0)
}

#[cfg(feature = "cuda")]
fn parse_pci_bus_string(s: &str) -> Result<cli_stressor_cuda_rs::PciBusAddress, String> {
    let s = s.trim();
    let (domain_raw, rest) = s
        .split_once(':')
        .ok_or_else(|| format!("invalid PCI bus format: {s}"))?;
    let (bus_raw, rest) = rest
        .split_once(':')
        .ok_or_else(|| format!("invalid PCI bus format: {s}"))?;
    let (device_raw, function_raw) = rest
        .split_once('.')
        .ok_or_else(|| format!("invalid PCI bus format: {s}"))?;

    let domain = u32::from_str_radix(domain_raw, 16)
        .map_err(|_| format!("invalid PCI domain: {domain_raw}"))?;
    let bus =
        u32::from_str_radix(bus_raw, 16).map_err(|_| format!("invalid PCI bus: {bus_raw}"))?;
    let device = u32::from_str_radix(device_raw, 16)
        .map_err(|_| format!("invalid PCI device: {device_raw}"))?;
    let function = u32::from_str_radix(function_raw, 16)
        .map_err(|_| format!("invalid PCI function: {function_raw}"))?;

    Ok(cli_stressor_cuda_rs::PciBusAddress {
        domain,
        bus,
        device,
        function,
    })
}

#[cfg(feature = "cuda")]
fn parse_uuid_string(s: &str) -> Result<[u8; 16], String> {
    let s = s.trim();

    // Remove spaces and dashes, then parse as continuous hex pairs
    let hex_str: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();

    if hex_str.len() != 32 {
        return Err(format!("UUID must be 32 hex digits, got {}", hex_str.len()));
    }

    let mut uuid = [0u8; 16];
    for i in 0..16 {
        let hex_pair = &hex_str[i * 2..(i + 1) * 2];
        uuid[i] = u8::from_str_radix(hex_pair, 16)
            .map_err(|_| format!("invalid UUID hex pair: {hex_pair}"))?;
    }

    Ok(uuid)
}

#[cfg(feature = "cuda")]
fn format_uuid_hex(uuid: &[u8; 16]) -> String {
    uuid.iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(feature = "cuda")]
fn print_cuda_gpu_list() -> Result<(), String> {
    let mut devices =
        enumerate_cuda_devices().map_err(|e| format!("failed to enumerate devices: {e}"))?;
    devices.sort_by(|a, b| match (a.pci_bus, b.pci_bus) {
        (Some(pa), Some(pb)) => (pa.domain, pa.bus, pa.device, pa.function).cmp(&(
            pb.domain,
            pb.bus,
            pb.device,
            pb.function,
        )),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.device_index.cmp(&b.device_index),
    });

    println!("{}", stylize_title("CUDA GPUs (sorted by PCI bus):"));
    println!(
        "{}",
        stylize(
            &format!(
                "{:>5} {:>6} {:<14} {:<32} {}",
                "s_idx", "cuda", "pci_bus", "uuid", "name"
            ),
            false
        )
    );
    for (sorted_idx, dev) in devices.iter().enumerate() {
        let pci = dev
            .pci_bus
            .map(|p| {
                format!(
                    "{:04X}:{:02X}:{:02X}.{}",
                    p.domain, p.bus, p.device, p.function
                )
            })
            .unwrap_or_else(|| "<none>".to_string());
        println!(
            "{}",
            stylize(
                &format!(
                    "{:>5} {:>6} {:<14} {:<32} {}",
                    sorted_idx,
                    dev.device_index,
                    pci,
                    format_uuid_hex(&dev.uuid),
                    dev.device_name
                ),
                false
            )
        );
    }
    Ok(())
}

#[cfg(feature = "cuda")]
fn filter_atomic_for_sm(kernel_types: &mut Vec<KernelType>, info: &DeviceInfo) {
    let sm_major = info.compute_capability.map(|(major, _)| major);
    if sm_major.unwrap_or(0) >= 8 {
        return;
    }
    if kernel_types.contains(&KernelType::Atomic) {
        kernel_types.retain(|k| *k != KernelType::Atomic);
        println!(
            "{}",
            stylize(
                &format!(
                    "Atomic path disabled: current GPU is below SM80 (detected {:?})",
                    info.compute_capability
                ),
                false
            )
        );
    }
}

#[cfg(feature = "cuda")]
fn print_device_info(info: &DeviceInfo) {
    println!(
        "{}",
        stylize_title(&format!("Testing Device: {}", info.name))
    );
    if let Some((major, minor)) = info.compute_capability {
        println!(
            "{}",
            stylize(&format!("Compute Capability: SM{}.{}", major, minor), false)
        );
    }
    if let Some(mem) = info.total_mem_gb {
        println!(
            "{}",
            stylize(&format!("Video Memory: {:.1} GB", mem), false)
        );
    }
}

#[cfg(feature = "cuda")]
fn print_summary(results: &[StressResult], info: &DeviceInfo) {
    println!("\n{}", "=".repeat(72));
    println!("{}", stylize_title("Phase 1 core stability summary"));
    println!(
        "{}",
        stylize_title(&format!("Testing Device: {}", info.name))
    );
    if let Some(mem) = info.total_mem_gb {
        println!(
            "{}",
            stylize(&format!("Video Memory: {:.1} GB", mem), false)
        );
    }

    let mut overall_ok = true;
    for r in results {
        let status = if !r.supported {
            "SKIP"
        } else if r.first_error.is_none() && r.validation_failures == 0 {
            "OK"
        } else {
            "FAIL"
        };
        if status == "FAIL" {
            overall_ok = false;
        }
        let eff = if r.elapsed_s > 0.0 {
            r.compute_s / r.elapsed_s * 100.0
        } else {
            0.0
        };
        println!(
            "{}",
            stylize(
                &format!(
                    "{:<12} {:<4} | iters={:8} | wall={:7.1}s | compute={:6.1}s | eff={:4.0}% | {:8.2} TFLOPS | val_fail={:3} | max_abs={:.3e} | max_rel={:.3e}",
                    r.precision,
                    status,
                    r.iterations,
                    r.elapsed_s,
                    r.compute_s,
                    eff,
                    r.tflops,
                    r.validation_failures,
                    r.max_abs_error,
                    r.max_rel_error
                ),
                false
            )
        );
        if let Some(err) = &r.first_error {
            println!(
                "{}",
                stylize(&format!("{:12}      first_error: {}", "", err), false)
            );
            if let Some(at) = r.first_error_at_s {
                println!(
                    "{}",
                    stylize(&format!("{:12}      at: {:.1}s", "", at), false)
                );
            }
        }
    }

    println!("{}", "=".repeat(72));
    println!("{}", stylize(" Result:", false));
    if overall_ok {
        println!(
            "{}",
            stylize(
                "- No obvious computation errors or validation failures were observed in the current test window.",
                false
            )
        );
    } else {
        println!(
            "{}",
            stylize(
                "- At least one precision mode reported an error or validation failure.",
                true
            )
        );
        std::process::exit(1);
    }
    println!("{}", "=".repeat(72));
}

#[cfg(feature = "cuda")]
fn main() {
    let (mut args, cli_set) = parse_args_with_cli_sources();
    let file_config = match &args.config {
        Some(path) => match load_file_config(path) {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                eprintln!(
                    "{}",
                    stylize(&format!("Invalid config file: {}", err), true)
                );
                std::process::exit(2);
            }
        },
        None => None,
    };
    if let Err(err) = apply_file_config_to_args(&mut args, &cli_set, file_config.as_ref()) {
        eprintln!(
            "{}",
            stylize(&format!("Invalid config file: {}", err), true)
        );
        std::process::exit(2);
    }

    if args.list_gpus {
        match print_cuda_gpu_list() {
            Ok(()) => std::process::exit(0),
            Err(err) => {
                eprintln!(
                    "{}",
                    stylize(&format!("Failed to list CUDA GPUs: {}", err), true)
                );
                std::process::exit(1);
            }
        }
    }

    // In Vulkan-only mode, skip all CUDA initialization and mixed-kernel parsing/output.
    if args.vulkan_only {
        #[cfg(feature = "vulkan")]
        {
            std::process::exit(run_vulkan_for_duration(args.duration));
        }

        #[cfg(not(feature = "vulkan"))]
        {
            eprintln!(
                "{}",
                stylize(
                    "--vulkan-only requires building with --features vulkan",
                    true
                )
            );
            std::process::exit(2);
        }
    }

    let matrix_sizes = match parse_int_list(&args.matrix_sizes) {
        Ok(values) => values,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("Invalid matrix sizes argument: {}", err), true)
            );
            std::process::exit(2);
        }
    };
    let fp64_matrix_sizes = match parse_int_list(&args.fp64_matrix_sizes) {
        Ok(values) => values,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(
                    &format!("Invalid fp64 matrix sizes argument: {}", err),
                    true
                )
            );
            std::process::exit(2);
        }
    };

    let gpu_device_index = match resolve_gpu_device_index(&args) {
        Ok(idx) => idx,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("Failed to resolve GPU device: {}", err), true)
            );
            std::process::exit(2);
        }
    };

    let mut backend = match cuda_backend::CudaBackend::new_with_device(gpu_device_index) {
        Ok(backend) => backend,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(
                    &format!("CUDA init failed (gpu_index={}): {}", gpu_device_index, err),
                    true
                )
            );
            std::process::exit(1);
        }
    };

    #[cfg(feature = "vulkan")]
    let cuda_device_identity = match backend.device_identity() {
        Ok(identity) => Some(identity),
        Err(err) => {
            if args.enable_vulkan_stress {
                eprintln!(
                    "{}",
                    stylize(
                        &format!(
                            "Failed to read CUDA device identity for Vulkan alignment immediately after CUDA init: {}",
                            err
                        ),
                        true
                    )
                );
                std::process::exit(1);
            }
            None
        }
    };

    let info = backend.device_info();
    print_device_info(&info);

    let precisions = match parse_precision_list(&args.precisions) {
        Ok(values) => values,
        Err(err) => {
            eprintln!("{}", stylize(&format!("Invalid argument: {}", err), true));
            std::process::exit(2);
        }
    };

    let include_fp8 = !args.disable_fp8;
    let mut filtered = Vec::new();
    for spec in precisions {
        if spec.kind == PrecisionKind::FP8E4M3FN && !include_fp8 {
            println!(
                "{}",
                stylize("FP8 E4M3FN disabled by flag, skipping", false)
            );
            continue;
        }
        filtered.push(spec);
    }
    if filtered.is_empty() {
        eprintln!("{}", stylize("No runnable precision modes available", true));
        std::process::exit(1);
    }

    let kernel_types_all = match parse_kernel_type_list(&args.kernel_types) {
        Ok(values) => values,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("Invalid kernel types argument: {}", err), true)
            );
            std::process::exit(2);
        }
    };
    let mut kernel_types = kernel_types_all.clone();
    filter_atomic_for_sm(&mut kernel_types, &info);
    if kernel_types.is_empty() {
        eprintln!(
            "{}",
            stylize("No runnable kernel types after capability filtering", true)
        );
        std::process::exit(1);
    }

    let kernel_mixture_base = if args.kernel_mixture.trim().is_empty() {
        parse_kernel_mixture(&args.kernel_mixture, &kernel_types)
    } else {
        parse_kernel_mixture(&args.kernel_mixture, &kernel_types_all)
    };
    let mut kernel_mixture = match kernel_mixture_base {
        Ok(values) => values,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("Invalid kernel mixture argument: {}", err), true)
            );
            std::process::exit(2);
        }
    };
    if kernel_types != kernel_types_all {
        kernel_mixture.retain(|entry| kernel_types.contains(&entry.kind));
        if kernel_mixture.is_empty() {
            kernel_mixture = kernel_types
                .iter()
                .map(|kind| cli_stressor_cuda_rs::KernelMixtureEntry {
                    kind: *kind,
                    weight: 1.0,
                })
                .collect();
        }
    }
    let stream_mode = match parse_stream_mode(&args.stream_mode) {
        Ok(mode) => mode,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("Invalid stream mode argument: {}", err), true)
            );
            std::process::exit(2);
        }
    };
    let config_overrides = match &file_config {
        Some(parsed) => match load_kernel_overrides_from_config(parsed) {
            Ok(values) => values,
            Err(err) => {
                eprintln!(
                    "{}",
                    stylize(&format!("Invalid config file: {}", err), true)
                );
                std::process::exit(2);
            }
        },
        None => Vec::new(),
    };
    let cli_overrides = match parse_kernel_param_overrides(&args.kernel_params) {
        Ok(values) => values,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("Invalid kernel params argument: {}", err), true)
            );
            std::process::exit(2);
        }
    };
    let kernel_param_overrides = merge_kernel_overrides(config_overrides, cli_overrides);

    let mut overall_passed = true;

    println!("\n{}", "-".repeat(72));
    println!("{}", stylize_title("Starting mixed-kernel stress"));
    println!(
        "{}",
        stylize(
            &format!(
                "  Precisions: {:?}",
                filtered.iter().map(|spec| spec.name).collect::<Vec<_>>()
            ),
            false
        )
    );
    println!(
        "{}",
        stylize(&format!("  Duration: {:.1} s", args.duration), false)
    );
    println!(
        "{}",
        stylize(
            &format!("  Warmup iterations: {}", args.warmup_iters),
            false
        )
    );
    println!(
        "{}",
        stylize(&format!("  Burst iterations: {}", args.burst_iters), false)
    );
    println!(
        "{}",
        stylize(
            &format!("  Validation interval: {:.1} s", args.validate_interval),
            false
        )
    );
    println!(
        "{}",
        stylize(&format!("  Validation size: {}", args.validate_size), false)
    );
    println!(
        "{}",
        stylize(
            &format!("  Minor mixture rate: {:.2}", args.minor_mixture_rate),
            false
        )
    );
    println!(
        "{}",
        stylize(&format!("  Kernel types: {:?}", kernel_types), false)
    );
    println!(
        "{}",
        stylize(&format!("  Kernel mixture: {:?}", kernel_mixture), false)
    );
    // println!("  Kernel param overrides: {:?}", kernel_param_overrides);
    println!(
        "{}",
        stylize(
            &format!(
                "  Stream mode: {:?} ({} streams)",
                stream_mode,
                stream_mode.stream_count()
            ),
            false
        )
    );

    // Optionally start the Vulkan graphics engine (if built with --features "vulkan").
    #[cfg(feature = "vulkan")]
    let mut vulkan_engine: Option<VulkanGraphicsEngine> = None;

    #[cfg(feature = "vulkan")]
    {
        if args.enable_vulkan_stress
            && let Some(identity) = cuda_device_identity
        {
            let selection = VulkanDeviceSelection {
                cuda_uuid: identity.uuid,
                cuda_pci_bus: identity.pci_bus,
            };
            let mut eng = VulkanGraphicsEngine::with_selection(selection);
            match eng.start_stress_thread() {
                Ok(_) => {
                    vulkan_engine = Some(eng);
                }
                Err(e) => {
                    eprintln!(
                        "{}",
                        stylize(
                            &format!("Failed to start VulkanGraphicsEngine: {}", e),
                            true
                        )
                    );
                }
            }
        }
    }

    let results = run_stress_mixed(
        &mut backend,
        &filtered,
        StressRunConfig {
            matrix_sizes: &matrix_sizes,
            fp64_matrix_sizes: &fp64_matrix_sizes,
            duration_s: args.duration,
            warmup_iters: args.warmup_iters,
            burst_iters: args.burst_iters,
            validate_interval_s: args.validate_interval,
            validate_size: args.validate_size,
            transpose_prob: args.transpose_prob,
            base_seed: args.seed,
            minor_mixture_rate: args.minor_mixture_rate,
            kernel_mixture: &kernel_mixture,
            stream_mode,
            kernel_param_overrides: &kernel_param_overrides,
        },
    );

    for res in &results {
        if res.first_error.is_some() && res.supported {
            overall_passed = false;
        }
    }

    #[cfg(feature = "vulkan")]
    if let Some(eng) = &vulkan_engine
        && eng
            .get_error_flag_arc()
            .load(std::sync::atomic::Ordering::SeqCst)
    {
        eprintln!(
            "{}",
            stylize("[FATAL] Vulkan engine reported an error", true)
        );
        overall_passed = false;
    }

    print_summary(&results, &info);

    // Stop Vulkan engine if it was started.
    #[cfg(feature = "vulkan")]
    if let Some(mut eng) = vulkan_engine
        && let Err(e) = eng.stop()
    {
        eprintln!(
            "{}",
            stylize(&format!("[FATAL] Vulkan engine stop failed: {}", e), true)
        );
        overall_passed = false;
    }

    if !overall_passed {
        std::process::exit(1);
    }
}

#[cfg(all(not(feature = "cuda"), feature = "vulkan"))]
fn main() {
    let args = Args::parse();
    if args.vulkan_only || args.enable_vulkan_stress {
        std::process::exit(run_vulkan_for_duration(args.duration));
    }

    eprintln!(
        "CUDA support is disabled. Use --vulkan-only (or --enable-vulkan-stress) when building with --features vulkan, or rebuild with --features cuda."
    );
    std::process::exit(1);
}

#[cfg(all(not(feature = "cuda"), not(feature = "vulkan")))]
fn main() {
    let _ = Args::parse();
    eprintln!(
        "{}",
        stylize(
            "CUDA support is disabled. Rebuild with --features cuda.",
            true
        )
    );
    std::process::exit(1);
}
