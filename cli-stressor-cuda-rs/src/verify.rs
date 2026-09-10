//! Ride-on-load verification: shared types, the deterministic pattern oracle,
//! the IntAlu host reference, and the machine-readable verdict structures.
//!
//! Design (verified against NVIDIA's bundled `gpu_stressor.exe` and HYDRA's
//! `HydraGpuStress.exe` at the disassembly level): the checker lives inside the
//! load pipeline and inspects the buffers the load itself is hammering, the
//! detector carries an injection self-test gate, and only a compact per-check
//! error report crosses D2H. Expected values are recomputed from the element
//! index (splitmix finalizer), so no golden buffer is ever stored.

use serde::Serialize;

/// Deterministic 32-bit oracle: splitmix64 finalizer, high word.
///
/// Mirrored in device code (`vhash32` in `verify_kernels.rs`); keep the two in
/// lockstep or every pattern check fails.
pub fn vhash32(x: u64) -> u32 {
    let mut x = x;
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 32) as u32
}

/// Expected value for element `i` under `seed`. The `* (i + 1)` fold keeps
/// seed=0 distinct from the zero pattern. Mirror of device `vexpected`.
pub fn vexpected_host(i: u64, seed: u64) -> u32 {
    vhash32(seed ^ (0x9E37_79B9_7F4A_7C15u64).wrapping_mul(i + 1))
}

/// Per-lane compact error report, device-shared with the verify kernels.
///
/// Modeled on HYDRA's per-window 400B readback (total_errors / idx bounds /
/// expected-vs-actual / bit-position histogram), shrunk to a single buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerifyReport {
    pub magic: u32,
    pub total_errors: u32,
    /// Lowest faulting element index (init 0xFFFFFFFF).
    pub idx_min: u32,
    /// Highest faulting element index (init 0).
    pub idx_max: u32,
    pub first_exp: u32,
    pub first_act: u32,
    /// atomicCAS lock for the first-error capture (init 0xFFFFFFFF).
    pub first_lock: u32,
    /// Count of faults whose (act^exp) lowest set bit is position N.
    pub bit_hist: [u32; 32],
    pub checked: u64,
    /// Per-block completion counter (host compares against launched blocks).
    pub done: u32,
}

impl VerifyReport {
    pub const MAGIC: u32 = 0xADBA_0000;
    /// The injection self-test constants (HYDRA heritage): the known error is
    /// injected at element 0xADBA with bit 22 flipped.
    pub const SELF_TEST_IDX: u32 = 0xADBA;
    pub const SELF_TEST_BIT: u32 = 22;

    pub fn init() -> Self {
        Self {
            magic: Self::MAGIC,
            total_errors: 0,
            idx_min: u32::MAX,
            idx_max: 0,
            first_exp: 0,
            first_act: 0,
            first_lock: u32::MAX,
            bit_hist: [0; 32],
            checked: 0,
            done: 0,
        }
    }
}

/// Reduction counters for one GEMM output scan (`gemm_stats_reduce`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GemmStats {
    pub nonfinite: u32,
    pub zero: u32,
    pub abs_sum: f32,
    pub done: u32,
}

impl GemmStats {
    pub fn init() -> Self {
        Self {
            nonfinite: 0,
            zero: 0,
            abs_sum: 0.0,
            done: 0,
        }
    }
}

// The report structs are device-shared: they must satisfy cudarc's
// device-copy marker traits. SAFETY: plain-old-data, all fields are POD
// scalars valid as zero bits. Kept in the defining module so the impls are
// local to this crate (the `extern crate self` alias makes other modules see
// these types as foreign).
#[cfg(feature = "cuda")]
mod device_copy_impls {
    use super::{GemmStats, SampleStats, VerifyReport};
    unsafe impl cudarc::driver::DeviceRepr for VerifyReport {}
    unsafe impl cudarc::driver::ValidAsZeroBits for VerifyReport {}
    unsafe impl cudarc::driver::DeviceRepr for GemmStats {}
    unsafe impl cudarc::driver::ValidAsZeroBits for GemmStats {}
    unsafe impl cudarc::driver::DeviceRepr for SampleStats {}
    unsafe impl cudarc::driver::ValidAsZeroBits for SampleStats {}
}

/// Counters for one sampled dot-product cross-check (`gemm_sample_check`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SampleStats {
    pub checked: u32,
    pub mismatch: u32,
    pub nonfinite: u32,
    /// Max |recomputed - stored|, bit-trick atomicMax on the int repr.
    pub max_abs_diff_bits: u32,
    pub first_bad: u32,
    pub done: u32,
}

impl SampleStats {
    pub fn init() -> Self {
        Self {
            checked: 0,
            mismatch: 0,
            nonfinite: 0,
            max_abs_diff_bits: 0,
            first_bad: u32::MAX,
            done: 0,
        }
    }

    pub fn max_abs_diff(&self) -> f32 {
        f32::from_bits(self.max_abs_diff_bits)
    }
}

/// Tunables for the ride-on-load detectors. All serde-facing fields default
/// through [`VerifyConfig::default`], so existing profiles stay valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyConfig {
    pub enabled: bool,
    pub self_test: bool,
    /// Verify the memcpy destination every Nth burst-op (seed % N == 0).
    pub memcpy_every: u32,
    /// Replace every Nth memset burst iteration with pattern fill + verify.
    pub memset_every: u32,
    /// GEMM stats + sampled cross-check every Nth burst-op (seed % N == 0).
    pub gemm_every: u32,
    /// Sampled dot-product cross-checks per GEMM burst.
    pub gemm_samples: u32,
    /// Output sizes up to this edge length use the full-coverage recompute
    /// (100% of elements, ~one extra GEMM at naive efficiency) instead of
    /// sampling; 0 disables the full-coverage path.
    pub full_check_max_size: usize,
    /// Gathered IntAlu outputs per burst-op for the host reference compare.
    pub intalu_samples: u32,
    /// Matrix side for the exact INT8 GEMM validation.
    pub int8_validate_size: usize,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            self_test: true,
            memcpy_every: 1,
            memset_every: 8,
            gemm_every: 4,
            gemm_samples: 512,
            full_check_max_size: 1024,
            intalu_samples: 1024,
            int8_validate_size: 512,
        }
    }
}

impl VerifyConfig {
    /// Deterministic, stateless cadence: run on ops whose seed hits the gate.
    pub fn due(&self, every: u32, seed: u64) -> bool {
        self.enabled && every > 0 && seed.is_multiple_of(every as u64)
    }
}

/// Host-side IntAlu reference: bit-exact mirror of the `int_alu_stress` kernel
/// chain (`acc = acc*b + a`, LCG `b`, optional DP4A fold, mode narrowing).
///
/// The `dp4a` flag must match the JIT-time `__CUDA_ARCH__ >= 610` selection,
/// i.e. pass `sm_major > 6 || (sm_major == 6 && sm_minor >= 1)`.
pub fn intalu_ref_element(a: i32, mut b: i32, mode: u32, dp4a: bool) -> i32 {
    let mut acc = a;
    for _ in 0..32 {
        acc = acc.wrapping_mul(b).wrapping_add(a);
        b = b.wrapping_mul(1664525).wrapping_add(1013904223);
    }
    if dp4a {
        acc ^= dp4a_ref(a, b);
    }
    match mode {
        16 => (acc as i16 as i32) ^ ((acc >> 16) as i16 as i32),
        8 => {
            let b0 = (acc as i8) as i32;
            let b1 = ((acc >> 8) as i8) as i32;
            let b2 = ((acc >> 16) as i8) as i32;
            let b3 = ((acc >> 24) as i8) as i32;
            b0 ^ b1 ^ b2 ^ b3
        }
        _ => acc,
    }
}

/// Scalar mirror of the INT8 4-way dot product `__dp4a(a, b, 0)`.
pub fn dp4a_ref(a: i32, b: i32) -> i32 {
    let a_bytes = a.to_le_bytes();
    let b_bytes = b.to_le_bytes();
    let mut sum = 0i32;
    for k in 0..4 {
        sum += (a_bytes[k] as i8 as i32) * (b_bytes[k] as i8 as i32);
    }
    sum
}

/// Cumulative per-domain detector counters drained from the backend after ops.
#[derive(Debug, Default, Clone, Serialize)]
pub struct DetectorStats {
    pub ops_checked: u64,
    /// Pattern-compare words + sampled dot products + gathered IntAlu outputs.
    pub elements_checked: u64,
    pub total_errors: u64,
    pub mismatches: u64,
    pub nonfinite: u64,
    pub first_error: Option<String>,
}

impl DetectorStats {
    pub fn merge(&mut self, other: &DetectorStats) {
        self.ops_checked += other.ops_checked;
        self.elements_checked += other.elements_checked;
        self.total_errors += other.total_errors;
        self.mismatches += other.mismatches;
        self.nonfinite += other.nonfinite;
        if self.first_error.is_none() {
            self.first_error = other.first_error.clone();
        }
    }
}

/// One injection self-test gate outcome.
#[derive(Debug, Clone, Serialize)]
pub struct SelfTestCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SelfTestReport {
    pub passed: bool,
    pub checks: Vec<SelfTestCheck>,
}

/// Failure classes for the verdict, mirroring the official scanner's
/// StressErrorType in the subset nvoc can observe user-side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictClass {
    None,
    DataError,
    MemoryError,
    ApiError,
    SelfTestFailed,
}

impl VerdictClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            VerdictClass::None => "none",
            VerdictClass::DataError => "data_error",
            VerdictClass::MemoryError => "memory_error",
            VerdictClass::ApiError => "api_error",
            VerdictClass::SelfTestFailed => "self_test_failed",
        }
    }
}

/// Classify a stress run for the verdict from its failure signals.
pub fn classify(
    self_test_passed: bool,
    validation_failures: u64,
    detector_errors: u64,
    runtime_errors: u64,
) -> VerdictClass {
    if !self_test_passed {
        return VerdictClass::SelfTestFailed;
    }
    if runtime_errors > 0 {
        return VerdictClass::ApiError;
    }
    if detector_errors > 0 {
        return VerdictClass::MemoryError;
    }
    if validation_failures > 0 {
        return VerdictClass::DataError;
    }
    VerdictClass::None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vexpected_is_deterministic_and_seed_sensitive() {
        assert_eq!(vexpected_host(0, 1), vexpected_host(0, 1));
        assert_ne!(vexpected_host(0, 1), vexpected_host(0, 2));
        assert_ne!(vexpected_host(0, 1), vexpected_host(1, 1));
        assert_ne!(vexpected_host(0, 0), vexpected_host(0, 1));
    }

    #[test]
    fn intalu_ref_matches_known_vectors() {
        // Independent hand-rolled trace of the kernel chain for a tiny input:
        // a=1, b=2, mode=32, no dp4a.
        // acc=1; then 32x { acc = acc*b + a; b = b*1664525 + 1013904223 }
        // (all wrapping i32).
        let (a, mut b) = (1i32, 2i32);
        let mut acc = a;
        for _ in 0..32 {
            acc = acc.wrapping_mul(b).wrapping_add(a);
            b = b.wrapping_mul(1664525).wrapping_add(1013904223);
        }
        assert_eq!(intalu_ref_element(1, 2, 32, false), acc);
    }

    #[test]
    fn dp4a_ref_matches_definition() {
        // __dp4a(a, b, 0) = sum of per-byte signed 8-bit products,
        // least-significant byte first.
        // a = 0x807F0102 -> LE bytes [2, 1, 127, -128]
        // b = 0x017F0201 -> LE bytes [1, 2, 127, 1]
        let a = 0x807F_0102u32 as i32;
        let b = 0x017F_0201u32 as i32;
        let a_bytes = a.to_le_bytes();
        let b_bytes = b.to_le_bytes();
        let expect: i32 = a_bytes
            .iter()
            .zip(b_bytes.iter())
            .map(|(x, y)| (*x as i8 as i32) * (*y as i8 as i32))
            .sum();
        assert_eq!(dp4a_ref(a, b), expect);
    }

    #[test]
    fn mode_narrowing_matches_kernel_c_casts() {
        // mode 16 folds two signed 16-bit halves: (int)(short)acc ^ (int)(short)(acc >> 16).
        let acc: i32 = 0x8001_7FFFu32 as i32;
        // low16 = 0x7FFF; (acc >> 16) low16 = 0x8001 -> sign-extends to -32767.
        let widened = (acc as i16 as i32) ^ ((acc >> 16) as i16 as i32);
        assert_eq!(widened, -2);
        // mode 8 xors four sign-extended bytes.
        let acc8: i32 = 0x807F_0102u32 as i32;
        let expect8 = (acc8 as i8 as i32)
            ^ (((acc8 >> 8) as i8) as i32)
            ^ (((acc8 >> 16) as i8) as i32)
            ^ (((acc8 >> 24) as i8) as i32);
        assert_eq!(expect8, 2 ^ 1 ^ 127 ^ -128);
    }

    #[test]
    fn verify_config_cadence_is_stateless() {
        let cfg = VerifyConfig {
            memcpy_every: 3,
            ..VerifyConfig::default()
        };
        assert!(cfg.due(3, 9));
        assert!(!cfg.due(3, 10));
        assert!(!cfg.due(0, 0), "every=0 disables the check");
        let off = VerifyConfig {
            enabled: false,
            ..VerifyConfig::default()
        };
        assert!(!off.due(1, 0), "enabled=false disables every check");
    }

    #[test]
    fn report_layout_fits_the_compact_readback_budget() {
        // The whole point of the report is a tiny D2H; keep it under 512B.
        assert!(std::mem::size_of::<VerifyReport>() <= 512);
        // repr(C): 7×u32 + [u32;32] + u64 (align-8 pad) + u32 → 176.
        assert_eq!(std::mem::size_of::<VerifyReport>(), 176);
        assert_eq!(std::mem::size_of::<GemmStats>(), 16);
        assert_eq!(std::mem::size_of::<SampleStats>(), 24);
    }
}
