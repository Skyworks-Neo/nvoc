extern crate self as cli_stressor_cuda_rs;

// Reuse the CLI implementation as the bundled optimizer worker. The optimizer
// calls runner::run_from_env only inside its isolated child process.
#[path = "main.rs"]
pub mod runner;

use rand::rngs::StdRng;
use rand::seq::IndexedRandom;
use rand::{RngExt, SeedableRng};
use rand_distr::StandardNormal;
use rayon::prelude::*;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

macro_rules! println {
    () => { std::println!() };
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        std::println!("{}", nvoc_cli_common::color::stylize(&msg, false));
    }};
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrecisionKind {
    FP64,
    FP32,
    TF32,
    FP16,
    BF16,
    FP8E4M3FN,
    INT8,
    INT16,
    INT32,
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
    /// Integer ALU stress via a custom NVRTC kernel (INT8 DP4A on capable HW,
    /// scalar int32/16/8 MAD chains otherwise). Use `--precisions int32/int16`
    /// (or `int8`) with `--kernel-types intalu`.
    IntAlu,
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
            KernelType::IntAlu => "INTALU",
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
        "intalu" | "ialu" | "int" => Ok(KernelType::IntAlu),
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

impl DeviceInfo {
    pub fn supports_precision(&self, spec: &PrecisionSpec) -> Result<(), String> {
        match spec.kind {
            PrecisionKind::FP64 => Ok(()),
            PrecisionKind::FP32 | PrecisionKind::TF32 => Ok(()),
            PrecisionKind::FP16 => Ok(()),
            // Integer ALU (INT8/16/32) runs on every SM. INT8 GEMM tensor-core
            // (IMMA) availability is enforced at the kernel level (cuBLAS will
            // fall back to a scalar INT8 path below SM 7.5), so we never reject
            // an INT precision here — the stress tester always produces load.
            PrecisionKind::INT8 | PrecisionKind::INT16 | PrecisionKind::INT32 => Ok(()),
            PrecisionKind::BF16 => {
                let sm = self.compute_capability.unwrap_or((0, 0));
                if sm.0 >= 8 {
                    Ok(())
                } else {
                    Err(format!(
                        "BF16 requires SM80 or higher (current: SM{}.{})",
                        sm.0, sm.1
                    ))
                }
            }
            PrecisionKind::FP8E4M3FN => {
                let sm = self.compute_capability.unwrap_or((0, 0));
                if sm.0 >= 8 && (sm.0 > 8 || sm.1 >= 9) {
                    Ok(())
                } else {
                    Err(format!(
                        "FP8 requires SM89 or higher (current: SM{}.{})",
                        sm.0, sm.1
                    ))
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct PciBusAddress {
    pub domain: u32,
    pub bus: u32,
    pub device: u32,
    pub function: u32,
}

#[derive(Debug, Clone)]
pub struct CudaDeviceEnumInfo {
    pub device_index: u32,
    pub device_name: String,
    pub uuid: [u8; 16],
    pub pci_bus: Option<PciBusAddress>,
    pub compute_capability: Option<(i32, i32)>,
    pub total_mem_gb: Option<f64>,
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
    fn supports_kernel(&self, _kind: KernelType) -> bool {
        true
    }
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
    fn run_kernel_path(&mut self, request: KernelPathRequest<'_>) -> Result<f64, BackendError>;
    fn synchronize(&self) -> Result<(), BackendError>;
    fn empty_cache(&self) -> Result<(), BackendError>;
}

#[derive(Clone, Copy, Debug)]
pub struct KernelPathRequest<'a> {
    pub spec: &'a PrecisionSpec,
    pub kind: KernelType,
    pub size: usize,
    pub warmup_iters: u32,
    pub burst_iters: u32,
    pub transpose_prob: f64,
    pub seed: u64,
    pub stream_mode: StreamMode,
}

fn validation_enabled(validate_interval_s: f64) -> bool {
    validate_interval_s > 0.0
}

/// Whether a precision is an integer (INT8/16/32) stress path.
///
/// INT paths have no FP reference to compare against (`validate_precision` is
/// FP-centric) and the `intalu` kernel is an intentionally non-referenceable
/// MAD/hash chain, so per-element validation is skipped for them — they are
/// pure-load stress paths, like the atomic kernel.
fn is_int_precision(kind: PrecisionKind) -> bool {
    matches!(
        kind,
        PrecisionKind::INT8 | PrecisionKind::INT16 | PrecisionKind::INT32
    )
}

/// Whether a (kernel, precision) pair is a meaningful stress combination.
///
/// The kernel and precision axes are *mostly* orthogonal, but a few pairings
/// have no hardware path: cuBLAS exposes no INT16/INT32 GEMM, and the FP
/// upload/gemm validation path doesn't cover INT. The integer precisions map
/// to specific kernels:
///
/// - INT8 → `Gemm` (cuBLAS INT8 GEMM, IMMA tensor cores) **or** `IntAlu`.
/// - INT16/INT32 → `IntAlu` only.
///
/// Returning false lets the dispatch loop soft-skip the combo (and keep
/// producing load on the compatible ones) instead of hard-failing.
pub fn kernel_precision_compatible(kind: KernelType, precision: PrecisionKind) -> bool {
    match (kind, precision) {
        // IntAlu is meaningful only for the dedicated integer precisions.
        (KernelType::IntAlu, precision) => is_int_precision(precision),
        (_, PrecisionKind::INT16) | (_, PrecisionKind::INT32) => false,
        // INT8 GEMM is supported via cuBLAS; other kernels (memcpy/memset/…)
        // also accept an INT8 precision by element size, so allow them.
        _ => true,
    }
}

/// Reject explicit per-kernel IntAlu precision overrides that would mislabel
/// an INT32 workload as floating-point stress.
pub fn validate_intalu_precision_overrides(
    overrides: &[KernelParamOverride],
) -> Result<(), String> {
    for item in overrides {
        if item.kind != KernelType::IntAlu {
            continue;
        }

        let specs = item.precisions.iter().flatten().chain(
            item.precision_mixture
                .iter()
                .flatten()
                .map(|entry| &entry.spec),
        );
        let mut incompatible = Vec::new();
        for spec in specs {
            if !is_int_precision(spec.kind) && !incompatible.contains(&spec.name) {
                incompatible.push(spec.name);
            }
        }

        if !incompatible.is_empty() {
            return Err(format!(
                "INTALU supports only INT8/INT16/INT32; incompatible precision(s): {}",
                incompatible.join(", ")
            ));
        }
    }
    Ok(())
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
        (
            "int8",
            PrecisionSpec {
                name: "INT8",
                kind: PrecisionKind::INT8,
                tf32_enabled: None,
            },
        ),
        (
            "i8",
            PrecisionSpec {
                name: "INT8",
                kind: PrecisionKind::INT8,
                tf32_enabled: None,
            },
        ),
        (
            "int16",
            PrecisionSpec {
                name: "INT16",
                kind: PrecisionKind::INT16,
                tf32_enabled: None,
            },
        ),
        (
            "i16",
            PrecisionSpec {
                name: "INT16",
                kind: PrecisionKind::INT16,
                tf32_enabled: None,
            },
        ),
        (
            "int32",
            PrecisionSpec {
                name: "INT32",
                kind: PrecisionKind::INT32,
                tf32_enabled: None,
            },
        ),
        (
            "i32",
            PrecisionSpec {
                name: "INT32",
                kind: PrecisionKind::INT32,
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

fn parse_precision(raw: &str) -> Result<PrecisionSpec, String> {
    let specs = parse_precision_list(raw)?;
    if specs.len() != 1 {
        return Err(format!("expected a single precision, got: {raw}"));
    }
    Ok(specs[0])
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
        let kind = parse_kernel_type(name)?;
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
        let spec = parse_precision(name)?;
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
    let n = size * size;
    let mut data = Vec::<MaybeUninit<f32>>::with_capacity(n);
    // SAFETY: Every element is initialized by the parallel fill below before the
    // buffer is converted to `Vec<f32>`.
    unsafe {
        data.set_len(n);
    }
    // Fill in parallel so the GPU is not starved waiting on single-threaded
    // StandardNormal sampling (the dominant gap between bursts for large
    // matrices). Each chunk is seeded independently from `seed`, so the result
    // is fully deterministic for a given seed (reproducibility and the FP64
    // validation path, which compares GPU vs CPU over the same matrix, are
    // unaffected) while saturating all CPU cores.
    const CHUNK: usize = 1 << 16;
    data.par_chunks_mut(CHUNK)
        .enumerate()
        .for_each(|(ci, slice)| {
            let mut rng = StdRng::seed_from_u64(
                seed.wrapping_add((ci as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
            );
            for v in slice.iter_mut() {
                v.write(rng.sample(StandardNormal));
            }
        });
    let len = data.len();
    let capacity = data.capacity();
    let ptr = data.as_mut_ptr().cast::<f32>();
    std::mem::forget(data);
    // SAFETY: The parallel fill above writes all `len` elements exactly once,
    // and `MaybeUninit<f32>` has the same layout as `f32`.
    let data = unsafe { Vec::from_raw_parts(ptr, len, capacity) };
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

fn choose_weighted<'a, T, F>(items: &'a [T], rng: &mut StdRng, weight_of: F) -> Option<&'a T>
where
    F: Fn(&T) -> f64,
{
    if items.is_empty() {
        return None;
    }
    let total_weight: f64 = items.iter().map(|item| weight_of(item).max(0.0)).sum();
    if total_weight <= 0.0 {
        return items.first();
    }
    let mut pick = rng.random::<f64>() * total_weight;
    for item in items {
        let weight = weight_of(item).max(0.0);
        if pick <= weight {
            return Some(item);
        }
        pick -= weight;
    }
    items.last()
}

fn choose_kernel_type(mixture: &[KernelMixtureEntry], rng: &mut StdRng) -> KernelType {
    if mixture.is_empty() {
        return KernelType::Gemm;
    }
    choose_weighted(mixture, rng, |entry| entry.weight)
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
        // The IntAlu kernel performs a fixed-length data-dependent int MAD
        // chain per element (plus a DP4A dot on capable HW). Counted as
        // integer ops (IOPs); throughput is reported as TFLOPS(eqv).
        KernelType::IntAlu => {
            const INTALU_OPS_PER_ELEM: u128 = 64;
            n * n * INTALU_OPS_PER_ELEM * iters
        }
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
    choose_weighted(mixture, rng, |entry| entry.weight).map(|entry| entry.spec)
}

pub fn run_stress_for_precision<B: Backend>(
    backend: &mut B,
    spec: PrecisionSpec,
    config: StressRunConfig<'_>,
    abort_flag: Option<Arc<AtomicBool>>,
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

    // Probe for dtype support (FP path: upload + gemm). INT precisions are
    // exercised through the dedicated `gemm` (INT8) and `intalu` kernel paths,
    // not this FP upload/gemm pipeline, so skip the probe for them — they are
    // always supported on any SM.
    let probe_a = make_random_host_matrix(8, config.base_seed.wrapping_add(1));
    let probe_b = make_random_host_matrix(8, config.base_seed.wrapping_add(2));
    // Not collapsed: the inner `if let Err` requires `let_chains` to merge with
    // the `!is_int_precision` guard, which is nightly-only.
    #[allow(clippy::collapsible_if)]
    if !is_int_precision(spec.kind) {
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
    }

    let mut rng = StdRng::seed_from_u64(config.base_seed);
    let start = Instant::now();
    let validate_enabled = validation_enabled(config.validate_interval_s);
    let mut next_validate = config.validate_interval_s;
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
        if let Some(ref flag) = abort_flag
            && flag.load(Ordering::SeqCst)
        {
            eprintln!(
                "[ABORT] abort flag set, stopping stress for precision {}",
                spec.name
            );
            break;
        }
        let kernel_kind = choose_kernel_type(config.kernel_mixture, &mut rng);
        let params = resolve_kernel_params(kernel_kind, &effective_config);
        let op_spec = if let Some(specs) = &params.precisions {
            *specs.choose(&mut rng).unwrap_or(&spec)
        } else {
            spec
        };
        if !kernel_precision_compatible(kernel_kind, op_spec.kind) {
            continue;
        }
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

        let op_elapsed = match backend.run_kernel_path(KernelPathRequest {
            spec: &op_spec,
            kind: kernel_kind,
            size,
            warmup_iters: params.warmup_iters,
            burst_iters: params.burst_iters,
            transpose_prob: params.transpose_prob,
            seed: op_seed,
            stream_mode: effective_config.stream_mode,
        }) {
            Ok(value) => value,
            Err(err) => {
                // An incompatible (kernel, precision) combo is expected when the
                // user mixes INT precisions with the full kernel pool (e.g.
                // INT16 has no GEMM). Soft-skip it instead of aborting the run.
                if !kernel_precision_compatible(kernel_kind, op_spec.kind) {
                    continue;
                }
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

        if validate_enabled && !is_int_precision(spec.kind) && elapsed_total >= next_validate {
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
                    next_validate = elapsed_total + effective_config.validate_interval_s;
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
    abort_flag: Option<Arc<AtomicBool>>,
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
        // Skip the FP upload/gemm probe for INT precisions (see run_stress_once).
        let probe_res = if is_int_precision(spec.kind) {
            Ok::<(), BackendError>(())
        } else {
            (|| {
                let a_dev = backend.upload_matrix(&probe_a, spec)?;
                let b_dev = backend.upload_matrix(&probe_b, spec)?;
                let _ = backend.gemm(&a_dev, &b_dev, false, false)?;
                backend.synchronize()?;
                Ok::<(), BackendError>(())
            })()
        };
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
    let validate_enabled = validation_enabled(config.validate_interval_s);
    let mut next_validate = config.validate_interval_s;
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
        if let Some(ref flag) = abort_flag
            && flag.load(Ordering::SeqCst)
        {
            eprintln!("[ABORT] abort flag set, stopping mixed-kernel stress.");
            break;
        }
        let kernel_kind = choose_kernel_type(config.kernel_mixture, &mut rng);
        let params = resolve_kernel_params(kernel_kind, &effective_config);
        let op_spec = if let Some(mixture) = &params.precision_mixture {
            choose_precision_from_mixture(mixture, &mut rng).unwrap_or_else(|| supported[0])
        } else if let Some(specs) = &params.precisions {
            *specs.choose(&mut rng).unwrap_or(&supported[0])
        } else {
            *supported.choose(&mut rng).unwrap_or(&supported[0])
        };
        if !kernel_precision_compatible(kernel_kind, op_spec.kind) {
            continue;
        }
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

        let op_elapsed = match backend.run_kernel_path(KernelPathRequest {
            spec: &op_spec,
            kind: kernel_kind,
            size,
            warmup_iters: params.warmup_iters,
            burst_iters: params.burst_iters,
            transpose_prob: params.transpose_prob,
            seed: op_seed,
            stream_mode: effective_config.stream_mode,
        }) {
            Ok(value) => value,
            Err(err) => {
                // Soft-skip an incompatible (kernel, precision) combo (e.g.
                // INT16 + GEMM) and keep producing load on the valid combos.
                if !kernel_precision_compatible(kernel_kind, op_spec.kind) {
                    continue;
                }
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
            "[MIX] t={:6.1}s/{:.0}s | {:10} | p={:11} | size={:5} | inst={:7.2} TFLOPS(eqv)",
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

        if validate_enabled && !is_int_precision(op_spec.kind) && elapsed_total >= next_validate {
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
                    next_validate = elapsed_total + effective_config.validate_interval_s;
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

#[cfg(test)]
mod tests {
    use super::{
        DeviceInfo, KernelType, PrecisionKind, estimate_kernel_work_flops,
        parse_kernel_param_overrides, parse_kernel_type, parse_precision_list,
        validate_intalu_precision_overrides, validation_enabled,
    };

    #[test]
    fn validation_interval_zero_disables_validation() {
        assert!(!validation_enabled(0.0));
        assert!(!validation_enabled(-1.0));
        assert!(validation_enabled(0.1));
    }

    fn int_spec(kind: PrecisionKind) -> super::PrecisionSpec {
        super::PrecisionSpec {
            name: "INT",
            kind,
            tf32_enabled: None,
        }
    }

    #[test]
    fn parse_int_precision_keys_and_aliases() {
        let specs = parse_precision_list("int8,i8,int16,i16,int32,i32").expect("int keys parse");
        let kinds: Vec<_> = specs.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![
                PrecisionKind::INT8,
                PrecisionKind::INT8,
                PrecisionKind::INT16,
                PrecisionKind::INT16,
                PrecisionKind::INT32,
                PrecisionKind::INT32,
            ]
        );
    }

    #[test]
    fn unknown_int_precision_rejected() {
        assert!(parse_precision_list("int64").is_err());
    }

    #[test]
    fn parse_intalu_kernel_type() {
        assert_eq!(parse_kernel_type("intalu").unwrap(), KernelType::IntAlu);
        assert_eq!(parse_kernel_type("ialu").unwrap(), KernelType::IntAlu);
        assert_eq!(parse_kernel_type("int").unwrap(), KernelType::IntAlu);
        assert_eq!(KernelType::IntAlu.as_str(), "INTALU");
    }

    #[test]
    fn int_precisions_supported_on_any_sm() {
        // Integer ALU works on every architecture, so supports_precision must
        // never reject an INT precision regardless of compute capability.
        let old_gpu = DeviceInfo {
            name: "OldGPU".into(),
            total_mem_gb: Some(4.0),
            compute_capability: Some((6, 1)), // Pascal
        };
        for kind in [
            PrecisionKind::INT8,
            PrecisionKind::INT16,
            PrecisionKind::INT32,
        ] {
            assert!(old_gpu.supports_precision(&int_spec(kind)).is_ok());
        }
        // A GPU with unknown compute capability still gets INT stress.
        let unknown = DeviceInfo {
            name: "MysteryGPU".into(),
            total_mem_gb: None,
            compute_capability: None,
        };
        assert!(
            unknown
                .supports_precision(&int_spec(PrecisionKind::INT8))
                .is_ok()
        );
    }

    #[test]
    fn intalu_flops_estimate() {
        // n*n elements, 64 IOPs each, over `iters` burst iterations.
        assert_eq!(
            estimate_kernel_work_flops(KernelType::IntAlu, 1024, 10),
            1024u128 * 1024 * 64 * 10
        );
    }

    #[test]
    fn kernel_precision_compatibility_matrix() {
        use super::kernel_precision_compatible;
        // INT16/INT32 only run on IntAlu.
        assert!(!kernel_precision_compatible(
            KernelType::Gemm,
            PrecisionKind::INT16
        ));
        assert!(!kernel_precision_compatible(
            KernelType::Gemm,
            PrecisionKind::INT32
        ));
        assert!(kernel_precision_compatible(
            KernelType::IntAlu,
            PrecisionKind::INT16
        ));
        assert!(kernel_precision_compatible(
            KernelType::IntAlu,
            PrecisionKind::INT32
        ));
        // INT8 GEMM is supported (cuBLAS), and INT8 + IntAlu too.
        assert!(kernel_precision_compatible(
            KernelType::Gemm,
            PrecisionKind::INT8
        ));
        assert!(kernel_precision_compatible(
            KernelType::IntAlu,
            PrecisionKind::INT8
        ));
        // IntAlu must never silently execute an INT32 workload for an FP label.
        for kind in [
            PrecisionKind::FP64,
            PrecisionKind::FP32,
            PrecisionKind::TF32,
            PrecisionKind::FP16,
            PrecisionKind::BF16,
            PrecisionKind::FP8E4M3FN,
        ] {
            assert!(!kernel_precision_compatible(KernelType::IntAlu, kind));
        }
        // FP precisions pair with GEMM normally.
        assert!(kernel_precision_compatible(
            KernelType::Gemm,
            PrecisionKind::FP32
        ));
    }

    #[test]
    fn intalu_precision_overrides_reject_floats() {
        let invalid = parse_kernel_param_overrides("intalu:precisions=fp16|fp32").unwrap();
        let err = validate_intalu_precision_overrides(&invalid).unwrap_err();
        assert!(err.contains("INTALU supports only INT8/INT16/INT32"));
        assert!(err.contains("FP16"));
        assert!(err.contains("FP32"));

        let invalid_mixture =
            parse_kernel_param_overrides("intalu:precision_mixture=fp16:0.5|int32:0.5").unwrap();
        assert!(validate_intalu_precision_overrides(&invalid_mixture).is_err());

        let valid = parse_kernel_param_overrides("intalu:precisions=int8|int16|int32").unwrap();
        assert!(validate_intalu_precision_overrides(&valid).is_ok());
    }
}
