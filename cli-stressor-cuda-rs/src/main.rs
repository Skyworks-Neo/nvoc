use clap::Parser;
#[cfg(feature = "cuda")]
use clap::{CommandFactory, FromArgMatches, parser::ValueSource};
#[cfg(feature = "cuda")]
use std::collections::HashMap;
#[cfg(feature = "cuda")]
use std::fs;

#[cfg(feature = "cuda")]
use cli_stressor_cuda_rs::parse_int_list;

#[cfg(feature = "cuda")]
use cli_stressor_cuda_rs::{
    Backend, DeviceInfo, KernelParamOverride, KernelType, PrecisionKind, PrecisionMixtureEntry,
    PrecisionSpec, StressResult, StressRunConfig, parse_kernel_mixture, parse_kernel_param_overrides,
    parse_kernel_type, parse_kernel_type_list, parse_precision_list, parse_stream_mode,
    run_stress_mixed,
};
#[cfg(feature = "cuda")]
use serde::Deserialize;

#[cfg(feature = "cuda")]
mod cuda_backend;

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
    kernel_params: Option<HashMap<String, FileKernelParam>>,
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
            return Err(format!("{context} precision_weight must be finite and >= 0: {weight}"));
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
            return Err(format!("{context} precision_mixture must be finite and >= 0: {weight}"));
        }
        let spec = parse_precision_list(name)?
            .first()
            .copied()
            .ok_or_else(|| format!("{context} invalid precision: {name}"))?;
        entries.push(PrecisionMixtureEntry { spec, weight: *weight });
    }
    Ok(entries)
}

#[cfg(feature = "cuda")]
fn precision_mixture_cli_from_weights(
    precisions: &[String],
    weights: &[f64],
    context: &str,
) -> Result<String, String> {
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
    let mut parts = Vec::with_capacity(precisions.len());
    for (name, weight) in precisions.iter().zip(weights.iter()) {
        if !weight.is_finite() || *weight < 0.0 {
            return Err(format!("{context} precision_weight must be finite and >= 0: {weight}"));
        }
        parts.push(format!("{name}:{weight}"));
    }
    Ok(parts.join("|"))
}

#[cfg(feature = "cuda")]
fn precision_mixture_cli_from_map(map: &HashMap<String, f64>, context: &str) -> Result<String, String> {
    if map.is_empty() {
        return Err(format!("{context} precision_mixture cannot be empty"));
    }
    let mut parts = Vec::with_capacity(map.len());
    for (name, weight) in map {
        if !weight.is_finite() || *weight < 0.0 {
            return Err(format!("{context} precision_mixture must be finite and >= 0: {weight}"));
        }
        parts.push(format!("{name}:{weight}"));
    }
    Ok(parts.join("|"))
}

#[cfg(feature = "cuda")]
fn load_kernel_overrides_from_config(path: &str) -> Result<Vec<KernelParamOverride>, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("failed to read config: {e}"))?;
    let parsed: FileConfig = toml::from_str(&raw).map_err(|e| format!("invalid TOML: {e}"))?;
    let mut out = Vec::new();
    if let Some(kernel_params) = parsed.kernel_params {
        for (name, item) in kernel_params {
            let kind = parse_kernel_type(&name)?;
            if let Some(sizes) = &item.matrix_sizes
                && sizes.is_empty()
            {
                return Err(format!("kernel_params.{name}.matrix_sizes cannot be empty"));
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
                matrix_sizes: item.matrix_sizes,
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
    let mut cmd = Args::command();
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
        "stream_mode",
        "disable_fp8",
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
) -> Result<(), String> {
    let Some(path) = &args.config else {
        return Ok(());
    };
    let raw = fs::read_to_string(path).map_err(|e| format!("failed to read config: {e}"))?;
    let parsed: FileConfig = toml::from_str(&raw).map_err(|e| format!("invalid TOML: {e}"))?;

    if !cli_set.contains("duration") {
        if let Some(v) = parsed.duration {
            args.duration = v;
        }
    }
    if !cli_set.contains("matrix_sizes") {
        if let Some(v) = parsed.matrix_sizes {
            args.matrix_sizes = v
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(",");
        }
    }
    if !cli_set.contains("fp64_matrix_sizes") {
        if let Some(v) = parsed.fp64_matrix_sizes {
            args.fp64_matrix_sizes = v
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(",");
        }
    }
    if !cli_set.contains("precisions") {
        if let Some(v) = parsed.precisions {
            args.precisions = v.join(",");
        }
    }
    if !cli_set.contains("warmup_iters") {
        if let Some(v) = parsed.warmup_iters {
            args.warmup_iters = v;
        }
    }
    if !cli_set.contains("burst_iters") {
        if let Some(v) = parsed.burst_iters {
            args.burst_iters = v;
        }
    }
    if !cli_set.contains("validate_interval") {
        if let Some(v) = parsed.validate_interval {
            args.validate_interval = v;
        }
    }
    if !cli_set.contains("validate_size") {
        if let Some(v) = parsed.validate_size {
            args.validate_size = v;
        }
    }
    if !cli_set.contains("transpose_prob") {
        if let Some(v) = parsed.transpose_prob {
            args.transpose_prob = v;
        }
    }
    if !cli_set.contains("minor_mixture_rate") {
        if let Some(v) = parsed.minor_mixture_rate {
            args.minor_mixture_rate = v;
        }
    }
    if !cli_set.contains("seed") {
        if let Some(v) = parsed.seed {
            args.seed = v;
        }
    }
    if !cli_set.contains("kernel_types") {
        if let Some(v) = parsed.kernel_types {
            args.kernel_types = v.join(",");
        }
    }
    if !cli_set.contains("kernel_mixture") {
        if let Some(v) = parsed.kernel_mixture {
            args.kernel_mixture = match v {
                KernelMixtureConfig::Text(s) => s,
                KernelMixtureConfig::Map(m) => {
                    let mut parts = Vec::with_capacity(m.len());
                    for (k, w) in m {
                        parts.push(format!("{k}:{w}"));
                    }
                    parts.join(",")
                }
            };
        }
    }
    if !cli_set.contains("stream_mode") {
        if let Some(v) = parsed.stream_mode {
            args.stream_mode = v;
        }
    }
    if !cli_set.contains("disable_fp8") {
        if let Some(v) = parsed.disable_fp8 {
            args.disable_fp8 = v;
        }
    }
    if !cli_set.contains("kernel_params") {
        if let Some(kernel_params) = parsed.kernel_params {
            let mut entries = Vec::new();
            for (name, item) in kernel_params {
                let mut kvs = Vec::new();
                if let Some(v) = &item.precisions {
                    kvs.push(format!("precisions={}", v.join("|")));
                }
                if let Some(v) = &item.precision_mixture {
                    let encoded = precision_mixture_cli_from_map(
                        v,
                        &format!("kernel_params.{name}.precision_mixture"),
                    )?;
                    kvs.push(format!("precision_mixture={encoded}"));
                }
                if let Some(weights) = &item.precision_weight {
                    let precisions = item
                        .precisions
                        .as_ref()
                        .ok_or_else(|| {
                            format!("kernel_params.{name}.precision_weight requires precisions")
                        })?;
                    let encoded = precision_mixture_cli_from_weights(
                        precisions,
                        weights,
                        &format!("kernel_params.{name}.precision_weight"),
                    )?;
                    kvs.push(format!("precision_mixture={encoded}"));
                }
                if let Some(v) = &item.matrix_sizes {
                    kvs.push(format!(
                        "matrix_sizes={}",
                        v.iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>()
                            .join("|")
                    ));
                }
                if let Some(v) = item.warmup_iters {
                    kvs.push(format!("warmup_iters={v}"));
                }
                if let Some(v) = item.burst_iters {
                    kvs.push(format!("burst_iters={v}"));
                }
                if let Some(v) = item.transpose_prob {
                    kvs.push(format!("transpose_prob={v}"));
                }
                if let Some(v) = item.minor_mixture_rate {
                    kvs.push(format!("minor_mixture_rate={v}"));
                }
                entries.push(format!("{name}:{}", kvs.join(",")));
            }
            args.kernel_params = entries.join(";");
        }
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
            "Atomic path disabled: current GPU is below SM80 (detected {:?})",
            info.compute_capability
        );
    }
}

#[cfg(feature = "cuda")]
fn print_device_info(info: &DeviceInfo) {
    println!("Testing Device: {}", info.name);
    if let Some((major, minor)) = info.compute_capability {
        println!("Compute Capability: SM{}.{}", major, minor);
    }
    if let Some(mem) = info.total_mem_gb {
        println!("Video Memory: {:.1} GB", mem);
    }
}

#[cfg(feature = "cuda")]
fn print_summary(results: &[StressResult], info: &DeviceInfo) {
    println!("\n{}", "=".repeat(72));
    println!("Phase 1 core stability summary");
    println!("Testing Device: {}", info.name);
    if let Some(mem) = info.total_mem_gb {
        println!("Video Memory: {:.1} GB", mem);
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
        );
        if let Some(err) = &r.first_error {
            println!("{:12}      first_error: {}", "", err);
            if let Some(at) = r.first_error_at_s {
                println!("{:12}      at: {:.1}s", "", at);
            }
        }
    }

    println!("{}", "=".repeat(72));
    println!(" Result:");
    if overall_ok {
        println!(
            "- No obvious computation errors or validation failures were observed in the current test window."
        );
    } else {
        println!("- At least one precision mode reported an error or validation failure.");
        std::process::exit(1);
    }
    println!("{}", "=".repeat(72));
}

#[cfg(feature = "cuda")]
fn main() {
    let (mut args, cli_set) = parse_args_with_cli_sources();
    if let Err(err) = apply_file_config_to_args(&mut args, &cli_set) {
        eprintln!("Invalid config file: {}", err);
        std::process::exit(2);
    }

    let matrix_sizes = match parse_int_list(&args.matrix_sizes) {
        Ok(values) => values,
        Err(err) => {
            eprintln!("Invalid matrix sizes argument: {}", err);
            std::process::exit(2);
        }
    };
    let fp64_matrix_sizes = match parse_int_list(&args.fp64_matrix_sizes) {
        Ok(values) => values,
        Err(err) => {
            eprintln!("Invalid fp64 matrix sizes argument: {}", err);
            std::process::exit(2);
        }
    };

    let mut backend = match cuda_backend::CudaBackend::new() {
        Ok(backend) => backend,
        Err(err) => {
            eprintln!("CUDA init failed: {}", err);
            std::process::exit(1);
        }
    };

    let info = backend.device_info();
    print_device_info(&info);

    let precisions = match parse_precision_list(&args.precisions) {
        Ok(values) => values,
        Err(err) => {
            eprintln!("Invalid argument: {}", err);
            std::process::exit(2);
        }
    };

    let include_fp8 = !args.disable_fp8;
    let mut filtered = Vec::new();
    for spec in precisions {
        if spec.kind == PrecisionKind::FP8E4M3FN && !include_fp8 {
            println!("FP8 E4M3FN disabled by flag, skipping");
            continue;
        }
        filtered.push(spec);
    }
    if filtered.is_empty() {
        eprintln!("No runnable precision modes available");
        std::process::exit(1);
    }

    let kernel_types_all = match parse_kernel_type_list(&args.kernel_types) {
        Ok(values) => values,
        Err(err) => {
            eprintln!("Invalid kernel types argument: {}", err);
            std::process::exit(2);
        }
    };
    let mut kernel_types = kernel_types_all.clone();
    filter_atomic_for_sm(&mut kernel_types, &info);
    if kernel_types.is_empty() {
        eprintln!("No runnable kernel types after capability filtering");
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
            eprintln!("Invalid kernel mixture argument: {}", err);
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
            eprintln!("Invalid stream mode argument: {}", err);
            std::process::exit(2);
        }
    };
    let config_overrides = match &args.config {
        Some(path) => match load_kernel_overrides_from_config(path) {
            Ok(values) => values,
            Err(err) => {
                eprintln!("Invalid config file: {}", err);
                std::process::exit(2);
            }
        },
        None => Vec::new(),
    };
    let cli_overrides = match parse_kernel_param_overrides(&args.kernel_params) {
        Ok(values) => values,
        Err(err) => {
            eprintln!("Invalid kernel params argument: {}", err);
            std::process::exit(2);
        }
    };
    let kernel_param_overrides = merge_kernel_overrides(config_overrides, cli_overrides);

    let mut overall_passed = true;

    println!("\n{}", "-".repeat(72));
    println!("Starting mixed-kernel stress");
    println!(
        "  Precisions: {:?}",
        filtered.iter().map(|spec| spec.name).collect::<Vec<_>>()
    );
    println!("  Duration: {:.1} s", args.duration);
    println!("  Warmup iterations: {}", args.warmup_iters);
    println!("  Burst iterations: {}", args.burst_iters);
    println!("  Validation interval: {:.1} s", args.validate_interval);
    println!("  Validation size: {}", args.validate_size);
    println!("  Minor mixture rate: {:.2}", args.minor_mixture_rate);
    println!("  Kernel types: {:?}", kernel_types);
    println!("  Kernel mixture: {:?}", kernel_mixture);
    println!("  Kernel param overrides: {:?}", kernel_param_overrides);
    println!(
        "  Stream mode: {:?} ({} streams)",
        stream_mode,
        stream_mode.stream_count()
    );

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

    print_summary(&results, &info);

    if !overall_passed {
        std::process::exit(1);
    }
}

#[cfg(not(feature = "cuda"))]
fn main() {
    let _ = Args::parse();
    eprintln!("CUDA support is disabled. Rebuild with --features cuda.");
    std::process::exit(1);
}
