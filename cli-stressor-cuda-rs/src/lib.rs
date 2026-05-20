use rand::rngs::StdRng;
use rand::seq::IndexedRandom;
use rand::{Rng, SeedableRng};
use rand_distr::StandardNormal;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrecisionKind {
    FP64,
    FP32,
    TF32,
    FP16,
    BF16,
    FP8E4M3FN,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum KernelType {
    Gemm,
    Memcpy,
    Memset,
    Transpose,
    Elementwise,
    Reduction,
    Atomic,
}

impl KernelType {
    pub fn as_str(&self) -> &'static str {
        match self {
            KernelType::Gemm => "GEMM",
            KernelType::Memcpy => "MEMCPY",
            KernelType::Memset => "MEMSET",
            KernelType::Transpose => "TRANSPOSE",
            KernelType::Elementwise => "ELEMENTWISE",
            KernelType::Reduction => "REDUCTION",
            KernelType::Atomic => "ATOMIC",
        }
    }
}

pub fn parse_kernel_type(raw: &str) -> Result<KernelType, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "gemm" => Ok(KernelType::Gemm),
        "memcpy" | "copy" | "clone" => Ok(KernelType::Memcpy),
        "memset" | "fill" => Ok(KernelType::Memset),
        "transpose" => Ok(KernelType::Transpose),
        "elementwise" | "elem" | "add" => Ok(KernelType::Elementwise),
        "reduction" | "reduce" | "sum" => Ok(KernelType::Reduction),
        "atomic" => Ok(KernelType::Atomic),
        other => Err(format!("unsupported kernel type: {other}")),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamMode {
    Single,
    Dual,
    Triple,
}

impl StreamMode {
    pub fn stream_count(&self) -> usize {
        match self {
            StreamMode::Single => 1,
            StreamMode::Dual => 2,
            StreamMode::Triple => 3,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct KernelMixtureEntry {
    pub kind: KernelType,
    pub weight: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct PrecisionMixtureEntry {
    pub spec: PrecisionSpec,
    pub weight: f64,
}

#[derive(Clone, Debug)]
pub struct KernelParamOverride {
    pub kind: KernelType,
    pub precisions: Option<Vec<PrecisionSpec>>,
    pub precision_mixture: Option<Vec<PrecisionMixtureEntry>>,
    pub matrix_sizes: Option<Vec<usize>>,
    pub warmup_iters: Option<u32>,
    pub burst_iters: Option<u32>,
    pub transpose_prob: Option<f64>,
    pub minor_mixture_rate: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
pub struct PrecisionSpec {
    pub name: &'static str,
    pub kind: PrecisionKind,
    pub tf32_enabled: Option<bool>,
}

#[derive(Debug, Default, Clone)]
pub struct StressResult {
    pub precision: String,
    pub supported: bool,
    pub iterations: u64,
    pub total_flops: u128,
    pub elapsed_s: f64,
    pub compute_s: f64,
    pub tflops: f64,
    pub validations: u32,
    pub validation_failures: u32,
    pub max_abs_error: f32,
    pub max_rel_error: f32,
    pub first_error: Option<String>,
    pub first_error_at_s: Option<f64>,
}

#[derive(Clone, Copy, Debug)]
pub struct StressRunConfig<'a> {
    pub matrix_sizes: &'a [usize],
    pub fp64_matrix_sizes: &'a [usize],
    pub duration_s: f64,
    pub warmup_iters: u32,
    pub burst_iters: u32,
    pub validate_interval_s: f64,
    pub validate_size: usize,
    pub transpose_prob: f64,
    pub base_seed: u64,
    pub minor_mixture_rate: f64,
    pub kernel_mixture: &'a [KernelMixtureEntry],
    pub stream_mode: StreamMode,
    pub kernel_param_overrides: &'a [KernelParamOverride],
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub total_mem_gb: Option<f64>,
    pub compute_capability: Option<(i32, i32)>,
}

#[derive(Debug, Clone)]
pub struct HostMatrix {
    pub size: usize,
    pub data: Vec<f32>,
}

#[derive(thiserror::Error, Debug)]
pub enum BackendError {
    #[error("cuda backend is disabled")]
    Disabled,
    #[error("backend error: {0}")]
    Other(String),
}

pub trait Backend {
    type Matrix;
    type Output;

    fn device_info(&self) -> DeviceInfo;
    fn supports_precision(&self, spec: &PrecisionSpec) -> Result<(), String>;
    fn set_tf32(&mut self, enabled: Option<bool>) -> Result<(), BackendError>;
    fn upload_matrix(
        &self,
        host: &HostMatrix,
        spec: &PrecisionSpec,
    ) -> Result<Self::Matrix, BackendError>;
    fn gemm(
        &mut self,
        a: &Self::Matrix,
        b: &Self::Matrix,
        transpose_a: bool,
        transpose_b: bool,
    ) -> Result<Self::Output, BackendError>;
    fn output_to_f32(&self, output: &Self::Output) -> Result<Vec<f32>, BackendError>;
    fn run_kernel_path(
        &mut self,
        spec: &PrecisionSpec,
        kind: KernelType,
        size: usize,
        warmup_iters: u32,
        burst_iters: u32,
        transpose_prob: f64,
        seed: u64,
        stream_mode: StreamMode,
    ) -> Result<f64, BackendError>;
    fn synchronize(&self) -> Result<(), BackendError>;
    fn empty_cache(&self) -> Result<(), BackendError>;
}

pub fn parse_int_list(raw: &str) -> Result<Vec<usize>, String> {
    let mut values = Vec::new();
    for item in raw.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value = trimmed
            .parse::<usize>()
            .map_err(|_| format!("invalid integer: {trimmed}"))?;
        values.push(value);
    }
    if values.is_empty() {
        return Err("matrix sizes cannot be empty".to_string());
    }
    Ok(values)
}

pub fn parse_precision_list(raw: &str) -> Result<Vec<PrecisionSpec>, String> {
    let mapping = [
        (
            "fp64",
            PrecisionSpec {
                name: "FP64",
                kind: PrecisionKind::FP64,
                tf32_enabled: None,
            },
        ),
        (
            "fp32",
            PrecisionSpec {
                name: "FP32",
                kind: PrecisionKind::FP32,
                tf32_enabled: Some(false),
            },
        ),
        (
            "tf32",
            PrecisionSpec {
                name: "TF32",
                kind: PrecisionKind::TF32,
                tf32_enabled: Some(true),
            },
        ),
        (
            "fp16",
            PrecisionSpec {
                name: "FP16",
                kind: PrecisionKind::FP16,
                tf32_enabled: None,
            },
        ),
        (
            "bf16",
            PrecisionSpec {
                name: "BF16",
                kind: PrecisionKind::BF16,
                tf32_enabled: None,
            },
        ),
        (
            "fp8",
            PrecisionSpec {
                name: "FP8 E4M3FN",
                kind: PrecisionKind::FP8E4M3FN,
                tf32_enabled: None,
            },
        ),
    ];

    let mut selected = Vec::new();
    for item in raw.split(',') {
        let key = item.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        let spec = mapping
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, spec)| *spec)
            .ok_or_else(|| format!("unsupported precision: {item}"))?;
        selected.push(spec);
    }

    if selected.is_empty() {
        return Err("precision list cannot be empty".to_string());
    }
    Ok(selected)
}

pub fn parse_kernel_type_list(raw: &str) -> Result<Vec<KernelType>, String> {
    let mut selected = Vec::new();
    for item in raw.split(',') {
        let key = item.trim().to_ascii_lowercase();
        if key.is_empty() {
            continue;
        }
        let kind = parse_kernel_type(&key)?;
        if !selected.contains(&kind) {
            selected.push(kind);
        }
    }
    if selected.is_empty() {
        return Err("kernel type list cannot be empty".to_string());
    }
    Ok(selected)
}

pub fn parse_kernel_param_overrides(raw: &str) -> Result<Vec<KernelParamOverride>, String> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in raw.split(';') {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (kind_raw, params_raw) = trimmed
            .split_once(':')
            .ok_or_else(|| format!("invalid kernel params entry: {trimmed}"))?;
        let kind = parse_kernel_type(kind_raw)?;
        let mut item = KernelParamOverride {
            kind,
            precisions: None,
            precision_mixture: None,
            matrix_sizes: None,
            warmup_iters: None,
            burst_iters: None,
            transpose_prob: None,
            minor_mixture_rate: None,
        };
        for kv in params_raw.split(',') {
            let kv = kv.trim();
            if kv.is_empty() {
                continue;
            }
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| format!("invalid key=value in kernel params: {kv}"))?;
            let key = k.trim().to_ascii_lowercase();
            let value = v.trim();
            match key.as_str() {
                "precisions" | "precision" => {
                    let normalized = value.replace('|', ",");
                    item.precisions = Some(parse_precision_list(&normalized)?);
                }
                "precision_mixture" | "precision_mix" => {
                    let normalized = value.replace('|', ",");
                    item.precision_mixture = Some(parse_precision_mixture(&normalized)?);
                }
                "matrix_sizes" | "sizes" => {
                    let normalized = value.replace('|', ",");
                    item.matrix_sizes = Some(parse_int_list(&normalized)?);
                }
                "warmup_iters" | "warmup" => {
                    item.warmup_iters = Some(
                        value
                            .parse::<u32>()
                            .map_err(|_| format!("invalid warmup_iters: {value}"))?,
                    );
                }
                "burst_iters" | "burst" => {
                    item.burst_iters = Some(
                        value
                            .parse::<u32>()
                            .map_err(|_| format!("invalid burst_iters: {value}"))?,
                    );
                }
                "transpose_prob" | "transpose" => {
                    item.transpose_prob = Some(
                        value
                            .parse::<f64>()
                            .map_err(|_| format!("invalid transpose_prob: {value}"))?,
                    );
                }
                "minor_mixture_rate" | "minor" => {
                    item.minor_mixture_rate = Some(
                        value
                            .parse::<f64>()
                            .map_err(|_| format!("invalid minor_mixture_rate: {value}"))?,
                    );
                }
                _ => return Err(format!("unsupported kernel param key: {k}")),
            }
        }
        out.push(item);
    }
    Ok(out)
}

pub fn parse_stream_mode(raw: &str) -> Result<StreamMode, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "single" | "1" => Ok(StreamMode::Single),
        "dual" | "2" => Ok(StreamMode::Dual),
        "triple" | "3" => Ok(StreamMode::Triple),
        other => Err(format!(
            "unsupported stream mode: {other}, expected single|dual|triple"
        )),
    }
}

pub fn parse_kernel_mixture(
    raw: &str,
    kernel_types: &[KernelType],
) -> Result<Vec<KernelMixtureEntry>, String> {
    if kernel_types.is_empty() {
        return Err("kernel types cannot be empty".to_string());
    }
    if raw.trim().is_empty() {
        return Ok(kernel_types
            .iter()
            .map(|kind| KernelMixtureEntry {
                kind: *kind,
                weight: 1.0,
            })
            .collect());
    }

    let mut entries = Vec::new();
    for item in raw.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (name, weight_raw) = trimmed.split_once(':').ok_or_else(|| {
            format!("invalid kernel mixture item: {trimmed}, expected type:weight")
        })?;
        let kind = parse_kernel_type_list(name)?
            .first()
            .copied()
            .ok_or_else(|| format!("invalid kernel type in mixture: {name}"))?;
        if !kernel_types.contains(&kind) {
            return Err(format!(
                "kernel type {} is not included in --kernel-types",
                kind.as_str()
            ));
        }
        let weight = weight_raw
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("invalid mixture weight: {}", weight_raw.trim()))?;
        if !weight.is_finite() || weight < 0.0 {
            return Err(format!("mixture weight must be finite and >= 0: {weight}"));
        }
        entries.push(KernelMixtureEntry { kind, weight });
    }

    if entries.is_empty() {
        return Err("kernel mixture cannot be empty".to_string());
    }

    for kind in kernel_types {
        if !entries.iter().any(|entry| entry.kind == *kind) {
            entries.push(KernelMixtureEntry {
                kind: *kind,
                weight: 0.0,
            });
        }
    }
    Ok(entries)
}

pub fn parse_precision_mixture(raw: &str) -> Result<Vec<PrecisionMixtureEntry>, String> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for item in raw.split(',') {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (name, weight_raw) = trimmed.split_once(':').ok_or_else(|| {
            format!("invalid precision mixture item: {trimmed}, expected precision:weight")
        })?;
        let spec = parse_precision_list(name)?
            .first()
            .copied()
            .ok_or_else(|| format!("invalid precision in mixture: {name}"))?;
        let weight = weight_raw
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("invalid precision mixture weight: {}", weight_raw.trim()))?;
        if !weight.is_finite() || weight < 0.0 {
            return Err(format!(
                "precision mixture weight must be finite and >= 0: {weight}"
            ));
        }
        entries.push(PrecisionMixtureEntry { spec, weight });
    }
    if entries.is_empty() {
        return Err("precision mixture cannot be empty".to_string());
    }
    Ok(entries)
}

pub fn choose_tolerance(precision_name: &str) -> (f32, f32) {
    match precision_name {
        "FP64" => (1e-5, 1e-5),
        "FP32" => (1e-2, 1e-2),
        "TF32" => (2e-1, 2e-1),
        "FP16" => (2e-1, 2e-1),
        "BF16" => (5e-1, 5e-1),
        "FP8 E4M3FN" => (1.5, 1.5),
        _ => (1e-2, 1e-2),
    }
}

pub fn per_element_allclose(diff: &[f32], reference: &[f32], atol: f32, rtol: f32) -> bool {
    diff.iter()
        .zip(reference.iter())
        .all(|(d, r)| *d <= atol + rtol * r.abs())
}

pub fn make_random_host_matrix(size: usize, seed: u64) -> HostMatrix {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut data = Vec::with_capacity(size * size);
    for _ in 0..size * size {
        let sample: f32 = rng.sample(StandardNormal);
        data.push(sample);
    }
    HostMatrix { size, data }
}

pub fn cpu_reference_f32(a: &HostMatrix, b: &HostMatrix) -> Vec<f32> {
    let size = a.size;
    let a_f64: Vec<f64> = a.data.iter().map(|&v| v as f64).collect();
    let b_f64: Vec<f64> = b.data.iter().map(|&v| v as f64).collect();
    let mut c_f64 = vec![0.0f64; size * size];

    unsafe {
        matrixmultiply::dgemm(
            size,
            size,
            size,
            1.0,
            a_f64.as_ptr(),
            1,
            size as isize,
            b_f64.as_ptr(),
            1,
            size as isize,
            0.0,
            c_f64.as_mut_ptr(),
            1,
            size as isize,
        );
    }

    c_f64.into_iter().map(|v| v as f32).collect()
}

pub fn validate_precision<B: Backend>(
    backend: &mut B,
    spec: &PrecisionSpec,
    validate_size: usize,
    seed: u64,
) -> Result<(bool, f32, f32, Option<String>), BackendError> {
    backend.set_tf32(spec.tf32_enabled)?;

    let a_host = make_random_host_matrix(validate_size, seed);
    let b_host = make_random_host_matrix(validate_size, seed.wrapping_add(1));
    let reference = cpu_reference_f32(&a_host, &b_host);

    let a_dev = backend.upload_matrix(&a_host, spec)?;
    let b_dev = backend.upload_matrix(&b_host, spec)?;
    let out = backend.gemm(&a_dev, &b_dev, false, false)?;
    backend.synchronize()?;
    let out_f32 = backend.output_to_f32(&out)?;

    let mut max_abs = 0.0f32;
    let mut max_rel = 0.0f32;
    let (abs_thr, rel_thr) = choose_tolerance(spec.name);
    let mut passed = true;
    let mut failures = 0usize;

    for (idx, (out, ref_val)) in out_f32.iter().zip(reference.iter()).enumerate() {
        if !out.is_finite() {
            return Ok((
                false,
                f32::INFINITY,
                f32::INFINITY,
                Some("validation produced NaN/Inf".to_string()),
            ));
        }
        let diff = (*out - *ref_val).abs();
        max_abs = max_abs.max(diff);
        let rel = diff / (ref_val.abs() + 1e-12);
        max_rel = max_rel.max(rel);
        if diff > abs_thr + rel_thr * ref_val.abs() {
            passed = false;
            failures += 1;
            if failures >= 1 {
                let reason = format!(
                    "{} elements exceed atol+rtol*|ref|: max_abs={:.4e}, max_rel={:.4e} (first idx={})",
                    failures, max_abs, max_rel, idx
                );
                return Ok((false, max_abs, max_rel, Some(reason)));
            }
        }
    }

    let reason = if passed {
        None
    } else {
        Some("validation failed".to_string())
    };
    Ok((passed, max_abs, max_rel, reason))
}

fn choose_kernel_type(mixture: &[KernelMixtureEntry], rng: &mut StdRng) -> KernelType {
    if mixture.is_empty() {
        return KernelType::Gemm;
    }
    let total_weight: f64 = mixture.iter().map(|entry| entry.weight.max(0.0)).sum();
    if total_weight <= 0.0 {
        return mixture[0].kind;
    }
    let mut pick = rng.random::<f64>() * total_weight;
    for entry in mixture {
        let weight = entry.weight.max(0.0);
        if pick <= weight {
            return entry.kind;
        }
        pick -= weight;
    }
    mixture
        .last()
        .map(|entry| entry.kind)
        .unwrap_or(KernelType::Gemm)
}

fn estimate_kernel_work_flops(kind: KernelType, size: usize, burst_iters: u32) -> u128 {
    let n = size as u128;
    let iters = burst_iters as u128;
    match kind {
        KernelType::Gemm => 2 * n * n * n * iters,
        KernelType::Memcpy => n * n * iters,
        KernelType::Memset => n * n * iters,
        KernelType::Transpose => 2 * n * n * iters,
        KernelType::Elementwise => 2 * n * n * iters,
        KernelType::Reduction => n * n * iters,
        KernelType::Atomic => n * n * iters,
    }
}

#[derive(Clone)]
struct ResolvedKernelParams {
    precisions: Option<Vec<PrecisionSpec>>,
    precision_mixture: Option<Vec<PrecisionMixtureEntry>>,
    matrix_sizes: Vec<usize>,
    matrix_sizes_default: bool,
    warmup_iters: u32,
    burst_iters: u32,
    transpose_prob: f64,
    minor_mixture_rate: f64,
}

fn resolve_kernel_params(kind: KernelType, config: &StressRunConfig<'_>) -> ResolvedKernelParams {
    let override_item = config
        .kernel_param_overrides
        .iter()
        .find(|item| item.kind == kind);
    let precisions = override_item.and_then(|item| item.precisions.clone());
    let precision_mixture = override_item.and_then(|item| item.precision_mixture.clone());
    let matrix_sizes_default = override_item
        .and_then(|item| item.matrix_sizes.clone())
        .is_none();
    let matrix_sizes = override_item
        .and_then(|item| item.matrix_sizes.clone())
        .unwrap_or_else(|| config.matrix_sizes.to_vec());
    let warmup_iters = override_item
        .and_then(|item| item.warmup_iters)
        .unwrap_or(config.warmup_iters);
    let burst_iters = override_item
        .and_then(|item| item.burst_iters)
        .unwrap_or(config.burst_iters);
    let transpose_prob = override_item
        .and_then(|item| item.transpose_prob)
        .unwrap_or(config.transpose_prob);
    let minor_mixture_rate = override_item
        .and_then(|item| item.minor_mixture_rate)
        .unwrap_or(config.minor_mixture_rate);
    ResolvedKernelParams {
        precisions,
        precision_mixture,
        matrix_sizes,
        matrix_sizes_default,
        warmup_iters,
        burst_iters,
        transpose_prob,
        minor_mixture_rate,
    }
}

fn filter_supported_kernel_precisions<B: Backend>(
    backend: &B,
    overrides: &[KernelParamOverride],
) -> Vec<KernelParamOverride> {
    let mut out = Vec::with_capacity(overrides.len());
    for item in overrides {
        let mut cloned = item.clone();
        if let Some(specs) = &item.precisions {
            let mut supported = Vec::new();
            for spec in specs {
                if backend.supports_precision(spec).is_ok() {
                    supported.push(*spec);
                } else {
                    println!(
                        "Kernel {} precision {} unsupported on this device, skipping it",
                        item.kind.as_str(),
                        spec.name
                    );
                }
            }
            cloned.precisions = if supported.is_empty() {
                None
            } else {
                Some(supported)
            };
        }
        if let Some(mixture) = &item.precision_mixture {
            let mut supported = Vec::new();
            for entry in mixture {
                if backend.supports_precision(&entry.spec).is_ok() {
                    supported.push(*entry);
                } else {
                    println!(
                        "Kernel {} precision {} unsupported on this device, skipping it",
                        item.kind.as_str(),
                        entry.spec.name
                    );
                }
            }
            cloned.precision_mixture = if supported.is_empty() {
                None
            } else {
                Some(supported)
            };
        }
        out.push(cloned);
    }
    out
}

fn choose_precision_from_mixture(
    mixture: &[PrecisionMixtureEntry],
    rng: &mut StdRng,
) -> Option<PrecisionSpec> {
    if mixture.is_empty() {
        return None;
    }
    let total_weight: f64 = mixture.iter().map(|entry| entry.weight.max(0.0)).sum();
    if total_weight <= 0.0 {
        return Some(mixture[0].spec);
    }
    let mut pick = rng.random::<f64>() * total_weight;
    for entry in mixture {
        let weight = entry.weight.max(0.0);
        if pick <= weight {
            return Some(entry.spec);
        }
        pick -= weight;
    }
    mixture.last().map(|entry| entry.spec)
}

pub fn run_stress_for_precision<B: Backend>(
    backend: &mut B,
    spec: PrecisionSpec,
    config: StressRunConfig<'_>,
) -> StressResult {
    let mut result = StressResult {
        precision: spec.name.to_string(),
        supported: true,
        ..StressResult::default()
    };

    if let Err(reason) = backend.supports_precision(&spec) {
        result.supported = false;
        result.first_error = Some(format!("SKIP: {reason}"));
        return result;
    }

    if let Err(err) = backend.set_tf32(spec.tf32_enabled) {
        result.supported = false;
        result.first_error = Some(format!("tf32 setup failed: {err}"));
        return result;
    }

    // Probe for dtype support.
    let probe_a = make_random_host_matrix(8, config.base_seed.wrapping_add(1));
    let probe_b = make_random_host_matrix(8, config.base_seed.wrapping_add(2));
    if let Err(err) = (|| {
        let a_dev = backend.upload_matrix(&probe_a, &spec)?;
        let b_dev = backend.upload_matrix(&probe_b, &spec)?;
        let _ = backend.gemm(&a_dev, &b_dev, false, false)?;
        backend.synchronize()?;
        Ok::<(), BackendError>(())
    })() {
        result.supported = false;
        result.first_error = Some(format!("probe failed: {err}"));
        return result;
    }

    let mut rng = StdRng::seed_from_u64(config.base_seed);
    let start = Instant::now();
    let mut next_validate = config.validate_interval_s.max(0.0);
    let mut validation_seed = config.base_seed ^ 0x5F3759DF;
    let effective_overrides =
        filter_supported_kernel_precisions(backend, config.kernel_param_overrides);
    let effective_config = StressRunConfig {
        matrix_sizes: config.matrix_sizes,
        fp64_matrix_sizes: config.fp64_matrix_sizes,
        duration_s: config.duration_s,
        warmup_iters: config.warmup_iters,
        burst_iters: config.burst_iters,
        validate_interval_s: config.validate_interval_s,
        validate_size: config.validate_size,
        transpose_prob: config.transpose_prob,
        base_seed: config.base_seed,
        minor_mixture_rate: config.minor_mixture_rate,
        kernel_mixture: config.kernel_mixture,
        stream_mode: config.stream_mode,
        kernel_param_overrides: &effective_overrides,
    };

    while start.elapsed().as_secs_f64() < config.duration_s {
        let kernel_kind = choose_kernel_type(config.kernel_mixture, &mut rng);
        let params = resolve_kernel_params(kernel_kind, &effective_config);
        let op_spec = if let Some(specs) = &params.precisions {
            *specs.choose(&mut rng).unwrap_or(&spec)
        } else {
            spec
        };
        let size_pool = if op_spec.kind == PrecisionKind::FP64 && params.matrix_sizes_default {
            effective_config.fp64_matrix_sizes
        } else {
            &params.matrix_sizes
        };
        let size = if rng.random::<f64>() > params.minor_mixture_rate {
            *size_pool.choose(&mut rng).unwrap_or(&size_pool[0])
        } else {
            let small_sizes = [127usize, 256, 511, 512, 1023];
            *small_sizes.choose(&mut rng).unwrap()
        };
        let op_seed = rng.random::<u64>();

        let op_elapsed = match backend.run_kernel_path(
            &op_spec,
            kernel_kind,
            size,
            params.warmup_iters,
            params.burst_iters,
            params.transpose_prob,
            op_seed,
            effective_config.stream_mode,
        ) {
            Ok(value) => value,
            Err(err) => {
                result.first_error = Some(format!("runtime error: {err}"));
                result.first_error_at_s = Some(start.elapsed().as_secs_f64());
                break;
            }
        };

        let flops = estimate_kernel_work_flops(kernel_kind, size, params.burst_iters) as f64;
        let inst_tflops = if op_elapsed > 0.0 {
            flops / op_elapsed / 1e12
        } else {
            0.0
        };
        let elapsed_total = start.elapsed().as_secs_f64();

        println!(
            "[{}] t={:6.1}s/{:.0}s | {:10} | p={:11} | size={:5} | inst={:7.2} TFLOPS(eqv)",
            spec.name,
            elapsed_total,
            effective_config.duration_s,
            kernel_kind.as_str(),
            op_spec.name,
            size,
            inst_tflops
        );

        result.iterations += params.burst_iters as u64;
        result.total_flops += estimate_kernel_work_flops(kernel_kind, size, params.burst_iters);
        result.compute_s += op_elapsed;
        result.elapsed_s = elapsed_total;
        if result.compute_s > 0.0 {
            result.tflops = (result.total_flops as f64 / result.compute_s) / 1e12;
        }

        let _ = backend.empty_cache();

        if elapsed_total >= next_validate {
            match validate_precision(
                backend,
                &spec,
                effective_config.validate_size,
                validation_seed,
            ) {
                Ok((passed, max_abs, max_rel, reason)) => {
                    let status = if passed { "OK" } else { "FAIL" };
                    println!(
                        "[{}] validate | abs={:.3e} | rel={:.3e} | {}",
                        spec.name, max_abs, max_rel, status
                    );
                    result.validations += 1;
                    result.max_abs_error = result.max_abs_error.max(max_abs);
                    result.max_rel_error = result.max_rel_error.max(max_rel);
                    if !passed {
                        result.validation_failures += 1;
                        if result.first_error.is_none() {
                            result.first_error = reason;
                            result.first_error_at_s = Some(start.elapsed().as_secs_f64());
                        }
                        break;
                    }
                    next_validate = elapsed_total + effective_config.validate_interval_s.max(0.0);
                    validation_seed = validation_seed.wrapping_add(1);
                }
                Err(err) => {
                    result.first_error = Some(format!("validation error: {err}"));
                    result.first_error_at_s = Some(start.elapsed().as_secs_f64());
                    break;
                }
            }
        }

        if op_elapsed < 0.01 {
            std::thread::yield_now();
        }
    }

    result.elapsed_s = start.elapsed().as_secs_f64();
    if result.compute_s > 0.0 {
        result.tflops = (result.total_flops as f64 / result.compute_s) / 1e12;
    }
    result
}

pub fn run_stress_mixed<B: Backend>(
    backend: &mut B,
    precisions: &[PrecisionSpec],
    config: StressRunConfig<'_>,
) -> Vec<StressResult> {
    use std::collections::HashMap;

    let mut results: Vec<StressResult> = precisions
        .iter()
        .map(|spec| StressResult {
            precision: spec.name.to_string(),
            supported: true,
            ..StressResult::default()
        })
        .collect();
    let mut index_by_name = HashMap::new();
    for (idx, spec) in precisions.iter().enumerate() {
        index_by_name.insert(spec.name, idx);
    }

    let mut supported = Vec::new();
    for spec in precisions {
        if let Err(reason) = backend.supports_precision(spec) {
            if let Some(idx) = index_by_name.get(spec.name) {
                results[*idx].supported = false;
                results[*idx].first_error = Some(format!("SKIP: {reason}"));
            }
            continue;
        }
        if let Err(err) = backend.set_tf32(spec.tf32_enabled) {
            if let Some(idx) = index_by_name.get(spec.name) {
                results[*idx].supported = false;
                results[*idx].first_error = Some(format!("tf32 setup failed: {err}"));
            }
            continue;
        }
        let probe_a = make_random_host_matrix(8, config.base_seed.wrapping_add(1));
        let probe_b = make_random_host_matrix(8, config.base_seed.wrapping_add(2));
        let probe_res = (|| {
            let a_dev = backend.upload_matrix(&probe_a, spec)?;
            let b_dev = backend.upload_matrix(&probe_b, spec)?;
            let _ = backend.gemm(&a_dev, &b_dev, false, false)?;
            backend.synchronize()?;
            Ok::<(), BackendError>(())
        })();
        if let Err(err) = probe_res {
            if let Some(idx) = index_by_name.get(spec.name) {
                results[*idx].supported = false;
                results[*idx].first_error = Some(format!("probe failed: {err}"));
            }
            continue;
        }
        supported.push(*spec);
    }

    if supported.is_empty() {
        return results;
    }

    let mut rng = StdRng::seed_from_u64(config.base_seed);
    let start = Instant::now();
    let mut next_validate = config.validate_interval_s.max(0.0);
    let mut validation_seed = config.base_seed ^ 0x5F3759DF;
    let effective_overrides =
        filter_supported_kernel_precisions(backend, config.kernel_param_overrides);
    let effective_config = StressRunConfig {
        matrix_sizes: config.matrix_sizes,
        fp64_matrix_sizes: config.fp64_matrix_sizes,
        duration_s: config.duration_s,
        warmup_iters: config.warmup_iters,
        burst_iters: config.burst_iters,
        validate_interval_s: config.validate_interval_s,
        validate_size: config.validate_size,
        transpose_prob: config.transpose_prob,
        base_seed: config.base_seed,
        minor_mixture_rate: config.minor_mixture_rate,
        kernel_mixture: config.kernel_mixture,
        stream_mode: config.stream_mode,
        kernel_param_overrides: &effective_overrides,
    };

    while start.elapsed().as_secs_f64() < config.duration_s {
        let kernel_kind = choose_kernel_type(config.kernel_mixture, &mut rng);
        let params = resolve_kernel_params(kernel_kind, &effective_config);
        let op_spec = if let Some(mixture) = &params.precision_mixture {
            choose_precision_from_mixture(mixture, &mut rng)
                .unwrap_or_else(|| supported[0])
        } else if let Some(specs) = &params.precisions {
            *specs.choose(&mut rng).unwrap_or(&supported[0])
        } else {
            *supported.choose(&mut rng).unwrap_or(&supported[0])
        };
        let size_pool = if op_spec.kind == PrecisionKind::FP64 && params.matrix_sizes_default {
            effective_config.fp64_matrix_sizes
        } else {
            &params.matrix_sizes
        };
        let size = if rng.random::<f64>() > params.minor_mixture_rate {
            *size_pool.choose(&mut rng).unwrap_or(&size_pool[0])
        } else {
            let small_sizes = [127usize, 256, 511, 512, 1023];
            *small_sizes.choose(&mut rng).unwrap()
        };
        let op_seed = rng.random::<u64>();
        if let Err(err) = backend.set_tf32(op_spec.tf32_enabled) {
            if let Some(idx) = index_by_name.get(op_spec.name) {
                results[*idx].first_error = Some(format!("tf32 setup failed: {err}"));
                results[*idx].first_error_at_s = Some(start.elapsed().as_secs_f64());
            }
            break;
        }

        let op_elapsed = match backend.run_kernel_path(
            &op_spec,
            kernel_kind,
            size,
            params.warmup_iters,
            params.burst_iters,
            params.transpose_prob,
            op_seed,
            effective_config.stream_mode,
        ) {
            Ok(value) => value,
            Err(err) => {
                if let Some(idx) = index_by_name.get(op_spec.name) {
                    results[*idx].first_error = Some(format!("runtime error: {err}"));
                    results[*idx].first_error_at_s = Some(start.elapsed().as_secs_f64());
                }
                break;
            }
        };

        let flops = estimate_kernel_work_flops(kernel_kind, size, params.burst_iters) as f64;
        let inst_tflops = if op_elapsed > 0.0 {
            flops / op_elapsed / 1e12
        } else {
            0.0
        };
        let elapsed_total = start.elapsed().as_secs_f64();

        println!(
            "[{}] t={:6.1}s/{:.0}s | {:10} | p={:11} | size={:5} | inst={:7.2} TFLOPS(eqv)",
            "MIX",
            elapsed_total,
            effective_config.duration_s,
            kernel_kind.as_str(),
            op_spec.name,
            size,
            inst_tflops
        );

        if let Some(idx) = index_by_name.get(op_spec.name) {
            let result = &mut results[*idx];
            result.iterations += params.burst_iters as u64;
            result.total_flops += estimate_kernel_work_flops(kernel_kind, size, params.burst_iters);
            result.compute_s += op_elapsed;
            if result.compute_s > 0.0 {
                result.tflops = (result.total_flops as f64 / result.compute_s) / 1e12;
            }
        }

        let _ = backend.empty_cache();

        if elapsed_total >= next_validate {
            match validate_precision(
                backend,
                &op_spec,
                effective_config.validate_size,
                validation_seed,
            ) {
                Ok((passed, max_abs, max_rel, reason)) => {
                    let status = if passed { "OK" } else { "FAIL" };
                    println!(
                        "[{}] validate | abs={:.3e} | rel={:.3e} | {}",
                        op_spec.name, max_abs, max_rel, status
                    );
                    if let Some(idx) = index_by_name.get(op_spec.name) {
                        let result = &mut results[*idx];
                        result.validations += 1;
                        result.max_abs_error = result.max_abs_error.max(max_abs);
                        result.max_rel_error = result.max_rel_error.max(max_rel);
                        if !passed {
                            result.validation_failures += 1;
                            if result.first_error.is_none() {
                                result.first_error = reason;
                                result.first_error_at_s = Some(start.elapsed().as_secs_f64());
                            }
                            break;
                        }
                    }
                    next_validate = elapsed_total + effective_config.validate_interval_s.max(0.0);
                    validation_seed = validation_seed.wrapping_add(1);
                }
                Err(err) => {
                    if let Some(idx) = index_by_name.get(op_spec.name) {
                        results[*idx].first_error = Some(format!("validation error: {err}"));
                        results[*idx].first_error_at_s = Some(start.elapsed().as_secs_f64());
                    }
                    break;
                }
            }
        }

        if op_elapsed < 0.01 {
            std::thread::yield_now();
        }
    }

    let total_elapsed = start.elapsed().as_secs_f64();
    for result in &mut results {
        result.elapsed_s = total_elapsed;
        if result.compute_s > 0.0 {
            result.tflops = (result.total_flops as f64 / result.compute_s) / 1e12;
        }
    }
    results
}
