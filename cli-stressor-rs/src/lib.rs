extern crate self as cli_stressor_rs;

// See the [features] table in Cargo.toml for the cuda11/cuda12/cuda13 split.
// Enabling two at once makes cudarc compile both header generations (each
// cfg band re-declares the same types), flooding the build with duplicate
// definitions before this error is even reached — the selector flags are
// simply not combinable.
#[cfg(any(
    all(feature = "cuda11", feature = "cuda12"),
    all(feature = "cuda11", feature = "cuda13"),
    all(feature = "cuda12", feature = "cuda13"),
))]
compile_error!("features `cuda11`/`cuda12`/`cuda13` are mutually exclusive — pass exactly one");
#[cfg(all(
    feature = "cuda",
    not(any(feature = "cuda11", feature = "cuda12", feature = "cuda13"))
))]
compile_error!(
    "feature `cuda` requires a CUDA generation selector: enable `cuda11`, `cuda12`, or `cuda13`"
);

// Reuse the CLI implementation as the bundled optimizer worker. The optimizer
// calls runner::run_from_args only inside its isolated child process.
#[path = "main.rs"]
pub mod runner;

use rand::rngs::StdRng;
use rand::seq::IndexedRandom;
use rand::{RngExt, SeedableRng};
use rand_distr::StandardNormal;
use rayon::prelude::*;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub mod verify;
pub use verify::{
    DetectorStats, GemmStats, SampleStats, SdcEvent, SdcTickEvent, SelfTestCheck, SelfTestReport,
    VerdictClass, VerifyConfig, VerifyReport, classify, dp4a_ref, intalu_ref_element,
    vexpected_host, vhash32,
};

// Duty-cycle power pulsing (MODS VkStressPulse borrow): pure schedule math,
// applied by the Vulkan render loop at frame boundaries.
pub mod pulse;
pub use pulse::{DutyMeter, PulseConfig, PulseMode, PulseState};

// NVML-backed in-run telemetry (clock/temp/power) for SDC event context.
pub mod telemetry;
pub use telemetry::{Telemetry, TelemetrySample};

// Heavy FurMark-style renderer lives in the lib so the bin target and the
// runner share one type definition.
#[cfg(feature = "vulkan")]
#[path = "vulkan_gfx_stressor.rs"]
pub mod vulkan_gfx_stressor;

#[cfg(feature = "vulkan")]
pub mod vulkan_render;

// Styled crate-wide println: every log line goes through `stylize`. Declared
// BEFORE the cuda_backend module so the backend's ([GEMM-LT] / [VERIFY] / …)
// lines get the same coloring as the run-loop lines; the video backend below
// relies on this textual order too.
macro_rules! println {
    () => { std::println!() };
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        std::println!("{}", nvoc_cli_common::color::stylize(&msg, false));
    }};
}

// CUDA backend lives in the lib so integration tests (and the bundled
// optimizer worker) can drive it directly.
#[cfg(feature = "cuda")]
pub mod cuda_backend;

// NVENC→NVDEC video-engine stress backend: runtime-loads the driver-shipped
// video libraries (no ffmpeg); compiled only under --features video.
// Declared after the `println!` macro so the engine's logging resolves it.
#[cfg(feature = "video")]
pub mod video_backend;
#[cfg(feature = "video")]
pub use video_backend::{VideoConfig, VideoEngine, VideoStats};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrecisionKind {
    FP64,
    FP32,
    TF32,
    FP16,
    BF16,
    FP8E4M3FN,
    /// 4-bit float (E2M1) with block scaling, cublasLt-only (Blackwell).
    FP4E2M1,
    /// E5M2 (CUDA_R_8F_E5M2), cublasLt-only, same SM89+ gate as E4M3FN.
    FP8E5M2,
    INT8,
    /// 4-bit signed integer (CUDA_R_4I), cublasLt-only, IMMA on SM75+.
    INT4,
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

#[derive(Debug, Default, Clone, serde::Serialize)]
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
    /// Ride-on-load detector counters attributed to this precision.
    #[serde(default)]
    pub detectors: DetectorStats,
    /// Approximate workload elements produced (Σ size² per op), the coverage
    /// denominator for the verdict.
    #[serde(default)]
    pub elements_produced: u64,
    /// First-encounter cublasLt algo report lines for this precision
    /// (shape + selected algo config; empty on the legacy cublas path).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gemm_algos: Vec<String>,
}

// Manual Debug: the SDC sink/telemetry fields are not Debug.
impl std::fmt::Debug for StressRunConfig<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StressRunConfig")
            .field("duration_s", &self.duration_s)
            .field("warmup_iters", &self.warmup_iters)
            .field("burst_iters", &self.burst_iters)
            .field("base_seed", &self.base_seed)
            .field("stream_mode", &self.stream_mode)
            .field("verify", &self.verify)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy)]
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
    pub verify: VerifyConfig,
    /// SDC event sink (op-level fault records enriched with telemetry).
    /// `None` = events are aggregated as before but not streamed.
    pub sdc_event_sink: Option<&'a dyn Fn(&SdcEvent)>,
    /// Telemetry provider for SDC event context (None = NVML unavailable).
    pub telemetry: Option<&'a Telemetry>,
}

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub total_mem_gb: Option<f64>,
    pub compute_capability: Option<(i32, i32)>,
    /// L2 cache size in bytes as reported by the driver (CUDA device
    /// attribute; the Vulkan-only path uses VkPhysicalDeviceVulkan13Properties).
    /// `None` = the driver did not report it.
    pub l2_cache_bytes: Option<u64>,
}

/// Memory-pressure mode for the ride-on-load verify engine (borrowed from
/// MODS VkStressFb's big-L2 / small-L2 split).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MemMode {
    /// Resident-block working set derived from the reported L2 size
    /// (0.75×L2, clamped 8–128 MiB); slab walk stays opt-in.
    #[default]
    Auto,
    /// Same working-set derivation as [`MemMode::Auto`] (explicit spelling).
    Resident,
    /// Force the address-walk slab stream mode (the existing `--verify-slab`
    /// behavior): the pattern windows sweep a VRAM-sized pool so checks
    /// cover pages far beyond L2.
    Stream,
}

/// Resident-block working set (in u32 words) for a given L2 size. The
/// historical default is 64 MiB; the L2-derived working set targets ~3/4 of
/// L2 so pattern checks stay cache-resident while the burst workload
/// thrashes around them.
pub fn resident_words_for_l2(l2_bytes: Option<u64>) -> usize {
    const DEFAULT_WORDS: usize = 64 << 18; // 64 MiB / 4
    const MIN_BYTES: usize = 8 << 20;
    const MAX_BYTES: usize = 128 << 20;
    match l2_bytes {
        Some(l2) if l2 > 0 => ((l2 as usize) * 3 / 4).clamp(MIN_BYTES, MAX_BYTES) / 4,
        _ => DEFAULT_WORDS,
    }
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
            // E5M2: same SM89+ hardware gate as E4M3 (the sm_89 PTX supports
            // E5M2 mma), but the cuBLASLt kernel inventory for Ada varies by
            // build — some return zero E5M2 candidates (observed on RTX 4060;
            // E4M3 is Ada's inference-native format). Runtime arbitration
            // decides: zero candidates downgrade the precision to SKIP with
            // the [CUDA] versions line as the correlating record.
            PrecisionKind::FP8E5M2 => {
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
            // FP4 (E2M1 with block scaling) rides the Blackwell FP4 tensor
            // cores via cublasLt; the runtime driver also needs a CUDA 12.8+
            // (R570+) cublasLt, which surfaces as a clear heuristic error.
            PrecisionKind::FP4E2M1 => {
                let sm = self.compute_capability.unwrap_or((0, 0));
                let fp4 = (sm.0 == 10 && (sm.1 == 0 || sm.1 == 3)) || sm.0 == 12;
                if fp4 {
                    Ok(())
                } else {
                    Err(format!(
                        "FP4 requires SM100/SM103/SM120 (current: SM{}.{})",
                        sm.0, sm.1
                    ))
                }
            }
            // INT4 GEMM is the IMMA 4-bit integer tensor-op path (CUDA_R_4I,
            // COMPUTE_32I) — Turing introduced INT4 IMMA at SM75.
            PrecisionKind::INT4 => {
                let sm = self.compute_capability.unwrap_or((0, 0));
                if sm.0 > 7 || (sm.0 == 7 && sm.1 >= 5) {
                    Ok(())
                } else {
                    Err(format!(
                        "INT4 GEMM requires SM75 or higher (current: SM{}.{})",
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
    pub l2_cache_bytes: Option<u64>,
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
    /// A dtype/kernel capability gap on this driver/device (e.g. cublasLt has
    /// no CUDA_R_4I candidate). The dispatch loops soft-skip it per precision
    /// instead of aborting the whole run.
    #[error("backend unsupported: {0}")]
    Unsupported(String),
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

    /// First ride-on-load detector failure since the last drain, if any.
    fn verify_failure(&self) -> Option<String> {
        None
    }
    /// Drain accumulated detector counters (called once per dispatch-loop op).
    fn verify_drain(&mut self) -> DetectorStats {
        DetectorStats::default()
    }
    /// Resident-block periodic check (called per dispatch-loop op; the engine
    /// self-throttles to one check per interval).
    fn verify_tick(&self, _interval_s: f64) {}
    /// Drain a structured resident-tick fault, if one fired since the last
    /// call. Default: None (backends without the verify engine).
    fn sdc_tick_fault(&self) -> Option<SdcTickEvent> {
        None
    }
    /// (resident-tick count, total bytes compared) for the run summary.
    /// Default: (0, 0) (backends without the verify engine).
    fn verify_tick_stats(&self) -> (u64, u64) {
        (0, 0)
    }
    /// Exact INT GEMM validation (INT8/INT4): seeded host reference vs device
    /// result with strict integer equality. Returns `(passed, failure_reason)`;
    /// Err only for infrastructure errors.
    fn validate_int_exact(
        &mut self,
        _kind: PrecisionKind,
        _size: usize,
        _seed: u64,
    ) -> Result<(bool, Option<String>), BackendError> {
        Err(BackendError::Other(
            "INT exact validation unsupported on this backend".to_string(),
        ))
    }
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
    pub verify: VerifyConfig,
}

fn validation_enabled(validate_interval_s: f64) -> bool {
    validate_interval_s > 0.0
}

/// Whether a precision is an integer (INT8/16/32) stress path.
///
/// INT paths have no FP reference (`validate_precision` is FP-centric): INT8
/// GEMM is covered by the exact integer sidecar (`validate_int8_exact`) and
/// the `intalu` chain by the bit-exact host reference (`intalu_ref_element`),
/// so the FP element loop is skipped for them.
fn is_int_precision(kind: PrecisionKind) -> bool {
    matches!(
        kind,
        PrecisionKind::INT8 | PrecisionKind::INT4 | PrecisionKind::INT16 | PrecisionKind::INT32
    )
}

/// Whether the dtype has a `Backend::upload_matrix`/`gemm` FP validation path
/// (gates the dtype probe and the periodic host-reference validation). FP8/FP4
/// have no such path — they are cublasLt stress modes whose correctness is
/// covered by the ride-on-load sampled cross-checks on the actual stress
/// buffers — and the INT precisions run through the dedicated gemm (INT8/INT4
/// exact sidecars) and intalu (INT16/32) paths instead.
fn has_fp_upload_path(kind: PrecisionKind) -> bool {
    !is_int_precision(kind)
        && !matches!(
            kind,
            PrecisionKind::FP8E4M3FN | PrecisionKind::FP8E5M2 | PrecisionKind::FP4E2M1
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
/// - INT4 → `Gemm` only (cublasLt CUDA_R_4I; the element-sized memcpy/memset
///   kernel paths have no half-byte element notion).
/// - INT16/INT32 → `IntAlu` only.
pub fn kernel_precision_compatible(kind: KernelType, precision: PrecisionKind) -> bool {
    match (kind, precision) {
        // IntAlu is meaningful only for the dedicated integer precisions.
        (KernelType::IntAlu, precision) => is_int_precision(precision),
        (_, PrecisionKind::INT16) | (_, PrecisionKind::INT32) => false,
        // FP4/INT4 are cublasLt GEMM-only modes (half-byte packed operands).
        (KernelType::Gemm, PrecisionKind::FP4E2M1 | PrecisionKind::INT4) => true,
        (_, PrecisionKind::FP4E2M1 | PrecisionKind::INT4) => false,
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
            "fp8e4m3",
            PrecisionSpec {
                name: "FP8 E4M3FN",
                kind: PrecisionKind::FP8E4M3FN,
                tf32_enabled: None,
            },
        ),
        (
            "fp8e5m2",
            PrecisionSpec {
                name: "FP8 E5M2",
                kind: PrecisionKind::FP8E5M2,
                tf32_enabled: None,
            },
        ),
        (
            "e5m2",
            PrecisionSpec {
                name: "FP8 E5M2",
                kind: PrecisionKind::FP8E5M2,
                tf32_enabled: None,
            },
        ),
        (
            "fp4",
            PrecisionSpec {
                name: "FP4 E2M1",
                kind: PrecisionKind::FP4E2M1,
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
            "int4",
            PrecisionSpec {
                name: "INT4",
                kind: PrecisionKind::INT4,
                tf32_enabled: None,
            },
        ),
        (
            "i4",
            PrecisionSpec {
                name: "INT4",
                kind: PrecisionKind::INT4,
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
        // Both FP8 variants decode exactly on both sides of the compare; the
        // band absorbs the f16 C-output rounding (quantization error itself
        // cancels — both sides read the same quantized operands).
        "FP8 E5M2" => (1.5, 1.5),
        // FP4: both operands and block scales decode exactly on both sides of
        // the compare; the band absorbs the f16 C-output rounding.
        "FP4 E2M1" => (2.0, 2.0),
        // INT4 sampled cross-checks recompute the exact i32 dot product (the
        // f32 accumulator is exact up to 2^24, far above 49·K).
        "INT4" => (0.0, 0.0),
        _ => (1e-2, 1e-2),
    }
}

pub fn per_element_allclose(diff: &[f32], reference: &[f32], atol: f32, rtol: f32) -> bool {
    diff.iter()
        .zip(reference.iter())
        .all(|(d, r)| *d <= atol + rtol * r.abs())
}

/// Number of CPU threads used for host-side stress-buffer generation.
///
/// Sized to the target machine's CPU count (`available_parallelism`), with a
/// conservative fallback of 8 when detection fails. Never hardcoded: the
/// stressor must scale from laptop parts to multi-socket workstations.
pub fn rng_thread_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8)
}

fn rng_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(rng_thread_count())
            .thread_name(|i| format!("stress-rng-{i}"))
            .build()
            .unwrap_or_else(|_| {
                // A 1-thread pool cannot fail to build; parallelism is a
                // perf knob, not a correctness requirement.
                rayon::ThreadPoolBuilder::new()
                    .num_threads(1)
                    .build()
                    .unwrap()
            })
    })
}

/// SplitMix64: fast non-crypto PRNG for stress input data. Stress buffers
/// never feed validation (which uses its own small `StdRng` matrices), so
/// crypto quality is unnecessary; throughput is (this runs between bursts
/// and steals wall-clock from actual stress time).
#[inline(always)]
pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

const RNG_CHUNK: usize = 1 << 16;

#[cfg(test)]
mod rng_perf_tests {
    use super::*;

    /// Not a pass/fail test: run manually with
    /// `cargo test --release -- --ignored tiled_fill_throughput --nocapture`
    /// to check host generation throughput on the target machine.
    #[test]
    #[ignore]
    fn tiled_fill_throughput() {
        let gib = 2usize << 30;
        let mut buf = vec![0u8; gib];
        let t = std::time::Instant::now();
        tiled_random_bytes(&mut buf, 42);
        let elapsed = t.elapsed().as_secs_f64();
        println!(
            "tiled fill: {:.2} GiB in {:.3} s = {:.2} GB/s (threads={})",
            gib as f64 / (1 << 30) as f64,
            elapsed,
            gib as f64 / elapsed / 1e9,
            rng_thread_count()
        );
    }
}

/// Parallel-fill a byte buffer with splitmix64 output, chunk-deterministic
/// for a given seed (same seed => same bytes regardless of thread count).
pub fn fill_random_bytes(buf: &mut [u8], seed: u64) {
    rng_pool().install(|| {
        buf.par_chunks_mut(RNG_CHUNK)
            .enumerate()
            .for_each(|(ci, c)| {
                let mut state = seed.wrapping_add((ci as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                let mut words = c.chunks_exact_mut(8);
                for w in &mut words {
                    let x = splitmix64(&mut state).to_le_bytes();
                    w.copy_from_slice(&x);
                }
                let rem = words.into_remainder();
                if !rem.is_empty() {
                    let x = splitmix64(&mut state).to_le_bytes();
                    rem.copy_from_slice(&x[..rem.len()]);
                }
            });
    });
}

/// Parallel-fill an f32 buffer with uniform [0,1) values (24-bit mantissa),
/// chunk-deterministic for a given seed.
pub fn fill_random_f32(buf: &mut [f32], seed: u64) {
    rng_pool().install(|| {
        buf.par_chunks_mut(RNG_CHUNK)
            .enumerate()
            .for_each(|(ci, c)| {
                let mut state = seed.wrapping_add((ci as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                for v in c.iter_mut() {
                    let x = splitmix64(&mut state);
                    *v = ((x >> 40) as f32) * (1.0f32 / 16_777_216.0f32);
                }
            });
    });
}

/// Parallel-fill an i32 buffer with random bits, chunk-deterministic.
pub fn fill_random_i32(buf: &mut [i32], seed: u64) {
    rng_pool().install(|| {
        buf.par_chunks_mut(RNG_CHUNK)
            .enumerate()
            .for_each(|(ci, c)| {
                let mut state = seed.wrapping_add((ci as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                for v in c.iter_mut() {
                    *v = (splitmix64(&mut state) >> 32) as i32;
                }
            });
    });
}

/// Size of the random tile generated per buffer before tiling-repeat.
pub const RNG_TILE_BYTES: usize = 4 << 20;

/// Cheapest full-random fill for paths that do not care about content
/// uniqueness (memcpy never reads its source): generate one random tile,
/// then repeat it in parallel at tile granularity. The repeat must be
/// parallel, not a single `copy_within` walk: the backing allocation is
/// lazily committed, so first-touch page faults on a fresh multi-GiB
/// buffer are kernel-serialized per thread (~1 GB/s single-core).
pub fn tiled_random_bytes(buf: &mut [u8], seed: u64) {
    if buf.len() <= RNG_TILE_BYTES {
        fill_random_bytes(buf, seed);
        return;
    }
    let (tile, rest) = buf.split_at_mut(RNG_TILE_BYTES);
    fill_random_bytes(tile, seed);
    rng_pool().install(|| {
        rest.par_chunks_mut(RNG_TILE_BYTES).for_each(|chunk| {
            chunk.copy_from_slice(&tile[..chunk.len()]);
        });
    });
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
    rng_pool().install(|| {
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
                    "{} elements exceed atol+rtol*|ref|: max_abs={:.4e}, max_rel={:.4e} (first idx={}, ref={:.6e}, got={:.6e}, diff={:.4e})",
                    failures, max_abs, max_rel, idx, *ref_val, *out, diff
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

/// Exact INT8 GEMM reference over `size²` inputs with i32 arithmetic
/// (inputs are pre-quantized to [-127, 127], so the accumulator cannot
/// overflow). Rayon-parallel; sizes stay ≤ a few hundred so the host cost is
/// tens of milliseconds.
///
/// SEMANTICS: matches the device, where the verbatim cuBLAS column-major call
/// makes the row-major output `C[i][j] = Σ_k A[k][j]·B[i][k]` (the A/B roles
/// are swapped relative to a textbook A*B; see `gemm_sample_check`).
pub fn cpu_reference_int8(a: &[i8], b: &[i8], size: usize) -> Vec<i32> {
    (0..size)
        .into_par_iter()
        .flat_map_iter(|i| {
            (0..size).map(move |j| {
                let mut acc = 0i32;
                for k in 0..size {
                    acc += a[k * size + j] as i32 * b[i * size + k] as i32;
                }
                acc
            })
        })
        .collect()
}

/// Exact integer GEMM reference for the cublasLt TN layout (TRANSA=T,
/// TRANSB=N over row-major uploaded operands): `C[i][j] = Σ_k A[j][k]·B[i][k]`.
/// This is the identity the INT8/INT4 exact sidecars validate against (see
/// the `gemm_sample_check` comment for the row-major/col-major derivation).
pub fn cpu_reference_int_tn(a: &[i8], b: &[i8], size: usize) -> Vec<i32> {
    (0..size)
        .into_par_iter()
        .flat_map_iter(|i| {
            (0..size).map(move |j| {
                let mut acc = 0i32;
                for k in 0..size {
                    acc += a[j * size + k] as i32 * b[i * size + k] as i32;
                }
                acc
            })
        })
        .collect()
}

/// Exact INT4 GEMM reference over packed 4-bit operands (two signed nibbles
/// per byte, low nibble first — same packing as `int4_pack`).
pub fn cpu_reference_int4_tn(a_packed: &[u8], b_packed: &[u8], size: usize) -> Vec<i32> {
    (0..size)
        .into_par_iter()
        .flat_map_iter(|i| {
            (0..size).map(move |j| {
                let mut acc = 0i32;
                for k in 0..size {
                    acc += int4_unpack_nib(a_packed, j * size + k) as i32
                        * int4_unpack_nib(b_packed, i * size + k) as i32;
                }
                acc
            })
        })
        .collect()
}

// ---- FP8 (E4M3FN) / FP4 (E2M1) / INT4 codec ---------------------------------
//
// cublasLt stress operands that have no native Rust type. The encodings here
// are mirrored bit-for-bit by the NVRTC verify kernels in
// `verify_kernels.rs` (`v_ab_load` type codes) — keep the two in lockstep.
//
// E4M3FN (CUDA_R_8F_E4M3): bias 7, denormals man·2⁻⁹, max ±448, NaN only at
// 0x7F/0xFF, no Inf. E2M1 (CUDA_R_4F_E2M1): bias 1, ±{0, .5, 1, 1.5, 2, 3, 4,
// 6}, no NaN/Inf. Both are monotonic over their finite bit patterns, which the
// quantizers exploit with a decode-table lookup + nearest-round (ties to even
// low-bit pattern).

/// Decode one E4M3FN byte to f32 (exact).
pub fn fp8_e4m3_decode(bits: u8) -> f32 {
    let sign = if bits & 0x80 != 0 { -1.0f32 } else { 1.0 };
    let exp = (bits >> 3) & 0xF;
    let man = (bits & 0x7) as f32;
    match exp {
        0 => {
            if man == 0.0 {
                f32::from_bits((bits as u32 & 0x80) << 24) // ±0
            } else {
                sign * man * 2f32.powi(-9)
            }
        }
        15 if (bits & 0x7F) == 0x7F => f32::NAN,
        15 => sign * (1.0 + man / 8.0) * 2f32.powi(8),
        e => sign * (1.0 + man / 8.0) * 2f32.powi(e as i32 - 7),
    }
}

/// The full 256-entry E4M3FN decode table (decode is self-inverse-checked in
/// tests; the quantizer searches it for the nearest representable value).
pub fn fp8_e4m3_decode_table() -> [f32; 256] {
    let mut table = [0.0f32; 256];
    for (bits, slot) in table.iter_mut().enumerate() {
        *slot = fp8_e4m3_decode(bits as u8);
    }
    table
}

/// Quantize f32 → E4M3FN with round-to-nearest-even (values beyond ±448
/// saturate to ±448; NaN/Inf map to the 0x7F NaN pattern).
///
/// O(1) bit math rather than a decode-table search: the stress path
/// quantizes ~10⁸ elements per burst op, and the original search put a
/// multi-second host gap between bursts (observed as a ~15 s idle "lead-in"
/// with the GPU downclocked on 8192² operands).
pub fn fp8_e4m3_quantize(x: f32) -> u8 {
    let bits = x.to_bits();
    let sign = ((bits >> 24) as u8) & 0x80;
    let ax = bits & 0x7FFF_FFFF;
    if ax >= 0x7F80_0000 {
        return 0x7F; // NaN/Inf → the NaN pattern (no Inf exists in E4M3FN)
    }
    if ax > 448f32.to_bits() {
        // 448 is the max finite; anything above saturates (RN would need a
        // 512 step that does not exist in the E4M3FN range).
        return sign | 0x7E;
    }
    if ax < 2f32.powi(-6).to_bits() {
        // Denormal band: value = m·2⁻⁹ with 3-bit m; RN-even on m = ax·2⁹
        // (exact — power-of-two scaling). m == 8 rounds up into the min
        // normal (2⁻⁶, pattern 0x08).
        let scaled = f32::from_bits(ax) * 512.0;
        let t = scaled.trunc();
        let frac = scaled - t;
        let m = if frac > 0.5 {
            t + 1.0
        } else if frac < 0.5 || (t as i32) & 1 == 0 {
            t
        } else {
            t + 1.0
        };
        return sign | (m as u8);
    }
    // Normal band: RN-even the 23-bit f32 mantissa down to 3 bits. Within
    // the saturated range the exponent bump cannot spill past exp field 15:
    // values ≤ 448 carry mantissa ≤ 6/8 there, so keep+round ≤ 6.
    let exp8 = (((ax >> 23) as i32) - 127 + 7) as u32;
    let man = ax & 0x7F_FFFF;
    let keep = man >> 20;
    let rest = man & 0xF_FFFF;
    let round = (rest >> 19) & 1;
    let sticky = rest & 0x7_FFFF;
    let mut m3 = keep;
    if round == 1 && (sticky != 0 || (keep & 1) == 1) {
        m3 += 1;
    }
    let (exp8, m3) = if m3 >= 8 { (exp8 + 1, 0) } else { (exp8, m3) };
    sign | ((exp8 << 3) as u8) | (m3 as u8)
}

/// Decode one E5M2 byte (CUDA_R_8F_E5M2: bias 15, denorm man·2⁻¹⁶, max
/// finite ±57344, Inf/NaN in the top exp field 31 — unlike E4M3, exp field
/// 15 is an ordinary exponent).
pub fn fp8_e5m2_decode(bits: u8) -> f32 {
    let sign = if bits & 0x80 != 0 { -1.0f32 } else { 1.0 };
    let exp = (bits >> 2) & 0x1F;
    let man = (bits & 0x3) as f32;
    match exp {
        0 if man == 0.0 => sign * 0.0,
        0 => sign * man * 2f32.powi(-16),
        31 if man == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        e => sign * (1.0 + man / 4.0) * 2f32.powi(e as i32 - 15),
    }
}

/// Quantize f32 → E5M2 with round-to-nearest-even, saturating semantics
/// (matches the hardware `cvt.rn.satfinite.e5m2` conversion: finite overflow
/// clamps to ±57344 instead of producing Inf operands that would poison the
/// GEMM output; NaN maps to the 0x7F NaN pattern).
pub fn fp8_e5m2_quantize(x: f32) -> u8 {
    let bits = x.to_bits();
    let sign = ((bits >> 24) as u8) & 0x80;
    let ax = bits & 0x7FFF_FFFF;
    if ax == 0x7F80_0000 {
        return sign | 0x7C; // Inf → Inf (IEEE-faithful)
    }
    if ax > 0x7F80_0000 {
        return 0x7F; // NaN → the canonical NaN pattern
    }
    if ax > 57344f32.to_bits() {
        return sign | 0x7B; // saturate above the max finite
    }
    if ax < 2f32.powi(-14).to_bits() {
        // Denormal band: value = m·2⁻¹⁶ with 2-bit m; RN-even on m = ax·2¹⁶.
        // m == 4 rounds up into the min normal (2⁻¹⁴, pattern 0x04).
        let scaled = f32::from_bits(ax) * 65536.0;
        let t = scaled.trunc();
        let frac = scaled - t;
        let m = if frac > 0.5 {
            t + 1.0
        } else if frac < 0.5 || (t as i32) & 1 == 0 {
            t
        } else {
            t + 1.0
        };
        return sign | (m as u8);
    }
    // Normal band: RN-even the 23-bit f32 mantissa down to 2 bits. The bump
    // cannot reach the reserved exp field 31: values ≤ 57344 carry mantissa
    // ≤ 3/4 at exp field 30, so keep+round ≤ 3 there.
    let exp8 = (((ax >> 23) as i32) - 127 + 15) as u32;
    let man = ax & 0x7F_FFFF;
    let keep = man >> 21;
    let rest = man & 0x1F_FFFF;
    let round = (rest >> 20) & 1;
    let sticky = rest & 0xF_FFFF;
    let mut m2 = keep;
    if round == 1 && (sticky != 0 || (keep & 1) == 1) {
        m2 += 1;
    }
    let (exp8, m2) = if m2 >= 4 { (exp8 + 1, 0) } else { (exp8, m2) };
    sign | ((exp8 << 2) as u8) | (m2 as u8)
}

/// Decode one packed FP4 nibble (low 4 bits; sign bit + 3 magnitude bits).
pub fn fp4_e2m1_decode(nib: u8) -> f32 {
    const TABLE: [f32; 8] = [0.0, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0];
    let sign = if nib & 0x8 != 0 { -1.0f32 } else { 1.0 };
    sign * TABLE[(nib & 0x7) as usize]
}

/// Quantize f32 → E2M1 with round-to-nearest-even (ties to even nibble).
/// Values beyond ±6 saturate to ±6.
pub fn fp4_e2m1_quantize(x: f32) -> u8 {
    let sign = if x.is_sign_negative() { 0x8u8 } else { 0 };
    let ax = x.abs();
    if ax >= 6.0 {
        return sign | 0x7;
    }
    if ax < 0.25 {
        // Below the RN midpoint between 0 and 0.5.
        return sign;
    }
    // Magnitude patterns 0..=7 are monotonic; 7 == 6.0 (the max).
    let (mut lo, mut hi) = (0u8, 7u8);
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if fp4_e2m1_decode(mid) <= ax {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let dl = ax - fp4_e2m1_decode(lo);
    let du = fp4_e2m1_decode(hi) - ax;
    let mag = if dl < du {
        lo
    } else if du < dl {
        hi
    } else if lo & 1 == 0 {
        lo
    } else {
        hi
    };
    sign | mag
}

/// Pack 4-bit values (FP4 nibbles or INT4) two-per-byte, low nibble first.
/// This matches the cublasLt packing convention for CUDA_R_4F_E2M1 /
/// CUDA_R_4I (element 2i in the low nibble, 2i+1 in the high nibble).
pub fn pack4(values: &[u8]) -> Vec<u8> {
    values
        .chunks(2)
        .map(|pair| (pair[0] & 0xF) | ((pair.get(1).map_or(0, |v| *v & 0xF)) << 4))
        .collect()
}

/// Unpack one 4-bit value by element index (low nibble first).
pub fn unpack4(packed: &[u8], idx: usize) -> u8 {
    let byte = packed[idx >> 1];
    if idx & 1 == 0 { byte & 0xF } else { byte >> 4 }
}

/// Unpack one signed INT4 value by element index.
pub fn int4_unpack_nib(packed: &[u8], idx: usize) -> i8 {
    let nib = unpack4(packed, idx) as i8;
    if nib & 0x8 != 0 { nib - 16 } else { nib }
}

/// Pack signed INT4 values two-per-byte (low nibble first).
pub fn int4_pack(values: &[i8]) -> Vec<u8> {
    let nibs: Vec<u8> = values.iter().map(|v| *v as u8).collect();
    pack4(&nibs)
}

/// FP4 block-scale format. NVFP4 = 16-element blocks with E4M3 scales
/// (`CUBLASLT_MATMUL_MATRIX_SCALE_VEC16_UE4M3`); MXFP4 = 32-element blocks
/// with power-of-two UE8M0 scales (`..._VEC32_UE8M0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fp4ScaleMode {
    Nvfp4,
    Mxfp4,
}

impl Fp4ScaleMode {
    pub fn block_len(self) -> usize {
        match self {
            Fp4ScaleMode::Nvfp4 => 16,
            Fp4ScaleMode::Mxfp4 => 32,
        }
    }
}

/// Quantize one block of K-major values into packed E2M1 nibbles plus one
/// scale byte. NVFP4 scale = E4M3(max|v|/6); MXFP4 scale = 2^⌈log2(max|v|/6)⌉.
/// Decoded value = `fp4_e2m1_decode(nib) × scale`.
pub fn fp4_quantize_block(values: &[f32], mode: Fp4ScaleMode) -> (Vec<u8>, u8) {
    let max_abs = values.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    match mode {
        Fp4ScaleMode::Nvfp4 => {
            // scale = RN_e4m3(max/6); a zero (or sub-min-denormal/6) max
            // quantizes the whole block to zero with scale 0.
            let scale = if max_abs > 0.0 {
                fp8_e4m3_quantize(max_abs / 6.0)
            } else {
                0
            };
            let scale_f = fp8_e4m3_decode(scale);
            let nibs: Vec<u8> = if scale_f > 0.0 {
                values
                    .iter()
                    .map(|v| fp4_e2m1_quantize(v / scale_f))
                    .collect()
            } else {
                vec![0; values.len()]
            };
            (pack4(&nibs), scale)
        }
        Fp4ScaleMode::Mxfp4 => {
            // scale = 2^⌈log2(max/6)⌉ as UE8M0 (biased-127 exponent byte).
            let scale = if max_abs > 0.0 {
                let ratio = max_abs / 6.0;
                let e = ratio.log2().ceil() as i32;
                (e + 127).clamp(0, 254) as u8
            } else {
                0
            };
            let scale_f = 2f32.powi(scale as i32 - 127);
            let nibs: Vec<u8> = if scale_f > 0.0 {
                values
                    .iter()
                    .map(|v| fp4_e2m1_quantize(v / scale_f))
                    .collect()
            } else {
                vec![0; values.len()]
            };
            (pack4(&nibs), scale)
        }
    }
}

/// Quantize a K-major operand into packed nibbles + one scale byte per block.
/// Returns `(packed_bytes, scales)` with scales laid out `[kblocks][cols]`
/// row-major (scale block index = k / block_len, column = the operand's
/// non-K coordinate).
///
/// Rayon-parallel over columns with a scratch buffer per column: the earlier
/// per-block `Vec` allocations put ~8M allocator calls on the 8192² burst
/// path (same host-gap failure mode as the FP8 quantizer search).
pub fn fp4_quantize_matrix(values: &[f32], size: usize, mode: Fp4ScaleMode) -> (Vec<u8>, Vec<u8>) {
    let block = mode.block_len();
    let kblocks = size.div_ceil(block);
    let cols: Vec<(Vec<u8>, Vec<u8>)> = (0..size)
        .into_par_iter()
        .map(|col| {
            let mut bytes = vec![0u8; size.div_ceil(2)];
            let mut col_scales = vec![0u8; kblocks];
            let mut block_vals = vec![0f32; block];
            for kb in 0..kblocks {
                let start = kb * block;
                let end = (start + block).min(size);
                for (slot, k) in block_vals.iter_mut().zip(start..end) {
                    *slot = values[col * size + k];
                }
                // Zero-pad the tail (K is a block-length multiple at the call
                // site; the pad keeps the packing math total for odd sizes).
                for slot in block_vals[end - start..block].iter_mut() {
                    *slot = 0.0;
                }
                let (block_packed, scale) = fp4_quantize_block(&block_vals, mode);
                // Clamp the copy to the column's byte slot: when size < block
                // (unit-test shapes) only the first `size` nibbles are real,
                // and pack4's zero padding beyond them is dropped.
                let slot = bytes.len() - kb * block / 2;
                let byte_len = block_packed.len().min(slot);
                bytes[kb * block / 2..kb * block / 2 + byte_len]
                    .copy_from_slice(&block_packed[..byte_len]);
                col_scales[kb] = scale;
            }
            (bytes, col_scales)
        })
        .collect();
    let mut packed = Vec::with_capacity(size * size.div_ceil(2));
    let mut scales = vec![0u8; kblocks * size];
    for (col, (bytes, col_scales)) in cols.into_iter().enumerate() {
        packed.extend_from_slice(&bytes);
        for (kb, scale) in col_scales.into_iter().enumerate() {
            scales[kb * size + col] = scale;
        }
    }
    (packed, scales)
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

    // Probe for dtype support (FP path: upload + gemm). INT and FP8/FP4
    // precisions are exercised through the dedicated cublasLt stress paths and
    // intalu kernel paths, not this FP upload/gemm pipeline, so skip the probe
    // for them.
    let probe_a = make_random_host_matrix(8, config.base_seed.wrapping_add(1));
    let probe_b = make_random_host_matrix(8, config.base_seed.wrapping_add(2));
    // Not collapsed: the inner `if let Err` requires `let_chains` to merge with
    // the `has_fp_upload_path` guard, which is nightly-only.
    #[allow(clippy::collapsible_if)]
    if has_fp_upload_path(spec.kind) {
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
        verify: config.verify,
        sdc_event_sink: config.sdc_event_sink,
        telemetry: config.telemetry,
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
            verify: effective_config.verify,
        }) {
            Ok(value) => value,
            Err(err) => {
                // An incompatible (kernel, precision) combo is expected when the
                // user mixes INT precisions with the full kernel pool (e.g.
                // INT16 has no GEMM). Soft-skip it instead of aborting the run.
                if !kernel_precision_compatible(kernel_kind, op_spec.kind) {
                    continue;
                }
                // A driver-level capability gap (no cublasLt candidate for
                // this dtype) downgrades the precision to SKIP.
                if let BackendError::Unsupported(reason) = err {
                    result.supported = false;
                    result.first_error = Some(format!("SKIP: {reason}"));
                    result.first_error_at_s = Some(start.elapsed().as_secs_f64());
                    return result;
                }
                result.first_error = Some(format!("runtime error: {err}"));
                result.first_error_at_s = Some(start.elapsed().as_secs_f64());
                break;
            }
        };

        // Ride-on-load detectors: same fail-fast policy as the mixed loop.
        let detector_failed = backend.verify_failure();
        let detector_deltas = backend.verify_drain();
        result.detectors.merge(&detector_deltas);
        if let Some(fail) = detector_failed {
            if result.first_error.is_none() {
                result.first_error = Some(fail.clone());
                result.first_error_at_s = Some(start.elapsed().as_secs_f64());
            }
            println!("[VERIFY] {:11} | {} | FAIL", kernel_kind.as_str(), fail);
            break;
        }

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

        if validate_enabled && has_fp_upload_path(spec.kind) && elapsed_total >= next_validate {
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
        // Skip the FP upload/gemm probe for INT and FP8/FP4 precisions (see run_stress_once).
        let probe_res = if has_fp_upload_path(spec.kind) {
            (|| {
                let a_dev = backend.upload_matrix(&probe_a, spec)?;
                let b_dev = backend.upload_matrix(&probe_b, spec)?;
                let _ = backend.gemm(&a_dev, &b_dev, false, false)?;
                backend.synchronize()?;
                Ok::<(), BackendError>(())
            })()
        } else {
            Ok::<(), BackendError>(())
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
        verify: config.verify,
        sdc_event_sink: config.sdc_event_sink,
        telemetry: config.telemetry,
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
            verify: effective_config.verify,
        }) {
            Ok(value) => value,
            Err(err) => {
                // Soft-skip an incompatible (kernel, precision) combo (e.g.
                // INT16 + GEMM) and keep producing load on the valid combos.
                if !kernel_precision_compatible(kernel_kind, op_spec.kind) {
                    continue;
                }
                // A driver-level capability gap (no cublasLt candidate for
                // this dtype) downgrades the precision to SKIP instead of
                // aborting the whole run.
                if let BackendError::Unsupported(reason) = err {
                    if let Some(idx) = index_by_name.get(op_spec.name) {
                        if results[*idx].supported {
                            // Say it once, loudly: without this line a
                            // single-precision run looks like a silent hang.
                            println!("[{}] SKIP: {reason}", op_spec.name);
                        }
                        results[*idx].supported = false;
                        if results[*idx].first_error.is_none() {
                            results[*idx].first_error = Some(format!("SKIP: {reason}"));
                            results[*idx].first_error_at_s = Some(start.elapsed().as_secs_f64());
                        }
                    }
                    // Nothing left producing load: stop instead of burning
                    // the remaining duration in skip cycles.
                    if results.iter().all(|r| !r.supported) {
                        println!("all precisions unsupported on this device - stopping early");
                        break;
                    }
                    continue;
                }
                if let Some(idx) = index_by_name.get(op_spec.name) {
                    results[*idx].first_error = Some(format!("runtime error: {err}"));
                    results[*idx].first_error_at_s = Some(start.elapsed().as_secs_f64());
                }
                break;
            }
        };

        // Ride-on-load detectors: a hard failure stops the run (same policy as
        // a validation failure); counters attribute to the active precision.
        let detector_failed = backend.verify_failure();
        let detector_deltas = backend.verify_drain();
        if let Some(idx) = index_by_name.get(op_spec.name) {
            let result = &mut results[*idx];
            result.detectors.merge(&detector_deltas);
            if detector_failed.is_some() && result.first_error.is_none() {
                result.first_error = detector_failed.clone();
                result.first_error_at_s = Some(start.elapsed().as_secs_f64());
            }
        }
        if detector_failed.is_some() {
            println!(
                "[VERIFY] {:11} | {} | FAIL",
                kernel_kind.as_str(),
                detector_failed.as_deref().unwrap_or("")
            );
            if !effective_config.verify.continue_on_error {
                break;
            }
        }

        // Resident-block periodic check (self-throttled inside the engine).
        backend.verify_tick(effective_config.verify.resident_interval_s);

        // SDC event stream: structured fault records with the telemetry
        // snapshot taken at detection time (clock/temp/power context).
        if let Some(sink) = effective_config.sdc_event_sink {
            let unix_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let t_rel_ms = start.elapsed().as_millis() as u64;
            if let Some(fault) = backend.sdc_tick_fault() {
                sink(&SdcEvent {
                    kind: "resident_tick",
                    unix_ms,
                    t_rel_ms,
                    op: "resident".into(),
                    seed: None,
                    total_errors: Some(fault.total_errors),
                    idx_min: Some(fault.idx_min),
                    idx_max: Some(fault.idx_max),
                    first_expected: Some(fault.first_expected),
                    first_actual: Some(fault.first_actual),
                    detail: None,
                    telemetry: effective_config.telemetry.as_ref().and_then(|t| t.sample()),
                });
            }
            if detector_failed.is_some() {
                sink(&SdcEvent {
                    kind: "burst_op",
                    unix_ms,
                    t_rel_ms,
                    op: op_spec.name.to_string(),
                    seed: Some(op_seed),
                    total_errors: None,
                    idx_min: None,
                    idx_max: None,
                    first_expected: None,
                    first_actual: None,
                    detail: detector_failed.clone(),
                    telemetry: effective_config.telemetry.as_ref().and_then(|t| t.sample()),
                });
            }
        }

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
            result.elements_produced += (size * size) as u64;
            if result.compute_s > 0.0 {
                result.tflops = (result.total_flops as f64 / result.compute_s) / 1e12;
            }
        }

        let _ = backend.empty_cache();

        if validate_enabled && elapsed_total >= next_validate {
            if is_int_precision(op_spec.kind) {
                // INT8/INT4 GEMM get the exact integer sidecar; INT16/32
                // intalu chains are covered per-op by the gather-based host
                // reference.
                if matches!(op_spec.kind, PrecisionKind::INT8 | PrecisionKind::INT4)
                    && kernel_kind == KernelType::Gemm
                {
                    match backend.validate_int_exact(
                        op_spec.kind,
                        effective_config.verify.int8_validate_size,
                        validation_seed,
                    ) {
                        Ok((true, _)) => {
                            println!("[{}] validate | exact | OK", op_spec.name);
                        }
                        Ok((false, reason)) => {
                            println!("[{}] validate | exact | FAIL", op_spec.name);
                            if let Some(idx) = index_by_name.get(op_spec.name) {
                                let result = &mut results[*idx];
                                result.validation_failures += 1;
                                if result.first_error.is_none() {
                                    result.first_error = reason;
                                    result.first_error_at_s = Some(start.elapsed().as_secs_f64());
                                }
                            }
                            break;
                        }
                        Err(err) => {
                            if let Some(idx) = index_by_name.get(op_spec.name) {
                                results[*idx].first_error =
                                    Some(format!("int validation error: {err}"));
                                results[*idx].first_error_at_s =
                                    Some(start.elapsed().as_secs_f64());
                            }
                            break;
                        }
                    }
                }
            } else if has_fp_upload_path(op_spec.kind) {
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
            next_validate = elapsed_total + effective_config.validate_interval_s;
            validation_seed = validation_seed.wrapping_add(1);
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
            l2_cache_bytes: None,
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
            l2_cache_bytes: None,
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
