//! Ride-on-load verification engine: NVRTC kernels that inspect the buffers
//! the stress paths themselves are hammering, plus the injection self-test
//! gate that proves the detection pipeline works before the run starts.
//!
//! Semantics verified one-hand against NVIDIA's bundled `gpu_stressor.exe`
//! (checker runs on the workload's own A/B/C, histogram reduced on the
//! workload stream, compact struct D2H) and HYDRA's `HydraGpuStress.exe`
//! (per-window pattern write + verify cycles, per-window error block,
//! injection gate at idx 0xADBA / bit 22 with every other window clean).

use std::cell::RefCell;
use std::sync::Arc;

use cudarc::driver::{
    CudaContext, CudaFunction, CudaModule, CudaSlice, CudaStream, DeviceRepr, LaunchConfig,
    PushKernelArg, ValidAsZeroBits,
};
use cudarc::nvrtc::{CompileOptions, compile_ptx_with_opts};

use cli_stressor_cuda_rs::{
    BackendError, DetectorStats, DeviceInfo, GemmStats, SampleStats, SelfTestCheck, SelfTestReport,
    VerifyReport,
};

use super::int_alu::nvrtc_arch_for;
use super::kernels::load_kernel;

/// Block cap for the grid-stride stats kernel.
const STATS_GRID_CAP: u32 = 4096;
/// Self-test scratch size in u32 words (4 MiB, far above the 0xADBA index).
const SAMPLE_SCRATCH_WORDS: usize = 1 << 20;

const VERIFY_SRC: &str = r#"
// ---- report structs (host-mirrored in src/verify.rs; keep in sync) ----
struct VerifyReport {
    unsigned int magic;
    unsigned int total_errors;
    unsigned int idx_min;      // init 0xFFFFFFFF
    unsigned int idx_max;
    unsigned int first_exp;
    unsigned int first_act;
    unsigned int first_lock;   // init 0xFFFFFFFF
    unsigned int bit_hist[32];
    unsigned long long checked;
    unsigned int done;
};
struct GemmStats {
    unsigned int nonfinite;
    unsigned int zero;
    float abs_sum;
    unsigned int done;
};
struct SampleStats {
    unsigned int checked;
    unsigned int mismatch;
    unsigned int nonfinite;
    unsigned int max_abs_diff_bits;
    unsigned int first_bad;    // init 0xFFFFFFFF
    unsigned int done;
};

__device__ unsigned int vhash32(unsigned long long x) {
    x ^= x >> 30; x *= 0xBF58476D1CE4E5B9ULL;
    x ^= x >> 27; x *= 0x94D049BB133111EBULL;
    x ^= x >> 31;
    return (unsigned int)(x >> 32);
}
__device__ unsigned int vexpected(unsigned long long i, unsigned long long seed) {
    return vhash32(seed ^ (0x9E3779B97F4A7C15ULL * (i + 1)));
}

// Manual half->float (no fp16 headers under NVRTC defaults).
__device__ float v_h2f(unsigned short h) {
    unsigned int sign = (unsigned int)(h & 0x8000u) << 16;
    unsigned int exp = (h >> 10) & 0x1Fu;
    unsigned int man = h & 0x03FFu;
    unsigned int bits;
    if (exp == 0u) {
        if (man == 0u) {
            bits = sign;
        } else {
            int e = -1;
            unsigned int m = man;
            do { m <<= 1; e++; } while (!(m & 0x400u));
            m &= 0x3FFu;
            bits = sign | (((127u - 15u) - (unsigned int)e) << 23) | (m << 13);
        }
    } else if (exp == 31u) {
        bits = sign | 0x7F800000u | (man << 13);
    } else {
        bits = sign | ((exp - 15u + 127u) << 23) | (man << 13);
    }
    return __uint_as_float(bits);
}
// type_code: 0=f32, 1=f64, 2=f16, 3=bf16
__device__ float v_load(const void* p, unsigned long long i, unsigned int tc) {
    if (tc == 1u) return (float)((const double*)p)[i];
    if (tc == 2u) return v_h2f(((const unsigned short*)p)[i]);
    if (tc == 3u) return __uint_as_float(((unsigned int)((const unsigned short*)p)[i]) << 16);
    return ((const float*)p)[i];
}
__device__ int v_isfinite(float v) {
    // NaN fails v==v; Inf exceeds the max finite magnitude.
    return (v == v) && (v <= 3.4028234663852886e38f) && (v >= -3.4028234663852886e38f);
}

extern "C" __global__ void verify_pattern_fill(
    unsigned int* buf, unsigned long long n, unsigned long long seed)
{
    unsigned long long idx = (unsigned long long)blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) return;
    buf[idx] = vexpected(idx, seed);
}

extern "C" __global__ void verify_compare(
    const unsigned int* buf, unsigned long long n, unsigned long long seed,
    VerifyReport* report)
{
    unsigned long long idx = (unsigned long long)blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) return;
    if (threadIdx.x == 0) atomicAdd(&report->done, 1u);
    unsigned int act = buf[idx];
    unsigned int exp = vexpected(idx, seed);
    if (act != exp) {
        unsigned int x = act ^ exp;
        atomicAdd(&report->total_errors, 1u);
        atomicMin(&report->idx_min, (unsigned int)idx);
        atomicMax(&report->idx_max, (unsigned int)idx);
        unsigned int prev = atomicCAS(&report->first_lock, 0xFFFFFFFFu, 0u);
        if (prev == 0xFFFFFFFFu) {
            report->first_exp = exp;
            report->first_act = act;
        }
        unsigned int bit = __popc(x ^ (x - 1u)) - 1u;  // lowest set bit position
        atomicAdd(&report->bit_hist[bit], 1u);
    }
}

extern "C" __global__ void verify_inject_error(
    unsigned int* buf, unsigned int idx, unsigned int mask)
{
    buf[idx] ^= mask;
}

// Block-reduced scan of a GEMM output buffer: non-finite / exact-zero counts
// and an |C| sum, one atomic per block per counter.
extern "C" __global__ void gemm_stats_reduce(
    const void* c, unsigned long long n, unsigned int tc, GemmStats* out)
{
    __shared__ unsigned int s_nonfinite;
    __shared__ unsigned int s_zero;
    __shared__ float s_sum;
    if (threadIdx.x == 0) { s_nonfinite = 0u; s_zero = 0u; s_sum = 0.0f; }
    __syncthreads();
    unsigned long long stride = (unsigned long long)gridDim.x * blockDim.x;
    unsigned int nf = 0u, z = 0u;
    float s = 0.0f;
    for (unsigned long long idx = (unsigned long long)blockIdx.x * blockDim.x + threadIdx.x;
         idx < n; idx += stride) {
        float v = v_load(c, idx, tc);
        if (!v_isfinite(v)) {
            nf++;
        } else {
            if (v == 0.0f) z++;
            float a = v >= 0.0f ? v : -v;
            s += a;
        }
    }
    if (nf) atomicAdd(&s_nonfinite, nf);
    if (z) atomicAdd(&s_zero, z);
    atomicAdd(&s_sum, s);
    __syncthreads();
    if (threadIdx.x == 0) {
        if (s_nonfinite) atomicAdd(&out->nonfinite, s_nonfinite);
        if (s_zero) atomicAdd(&out->zero, s_zero);
        atomicAdd(&out->abs_sum, s_sum);
        atomicAdd(&out->done, 1u);
    }
}

// One thread per sampled output element: recompute the dot product from the
// same A/B the GEMM consumed (double accumulator) and compare against C.
//
// SEMANTICS: cudarc passes the cuBLAS column-major call through verbatim, so
// for row-major inputs the device C is (B*A) in row-major terms:
//   C[i][j] = sum_k op_ta(A)[k][j] * op_tb(B)[i][k]
// where op_ta(N)=identity / op(T)=transpose, etc. The naive loop below uses
// exactly that identity (A/B roles swapped relative to a textbook A*B).
extern "C" __global__ void gemm_sample_check(
    const void* a, const void* b, const void* c,
    unsigned int size, unsigned int ta, unsigned int tb, unsigned int tc,
    const unsigned int* samples, unsigned int m,
    float atol, float rtol, SampleStats* out)
{
    unsigned int k = blockIdx.x * blockDim.x + threadIdx.x;
    unsigned int mm = 0u, nf = 0u, ck = 0u;
    if (k < m) {
        ck = 1u;
        unsigned int i = samples[2u * k];
        unsigned int j = samples[2u * k + 1u];
        // Accumulator precision must match the device math: for fp64 outputs
        // (tc==1) a float resum injects ~1e-4 absolute error (K-term rounding),
        // which exceeds the fp64 tolerance band, so we accumulate in double and
        // read the doubles verbatim. For fp32/fp16/bf16 float accumulation
        // keeps the checker cheap on consumer GPUs (fp64 runs at 1/32 rate)
        // while staying orders of magnitude below their tolerances.
        double dacc = 0.0;
        float facc = 0.0f;
        for (unsigned int kk = 0u; kk < size; ++kk) {
            unsigned int ai = ta ? (j * size + kk) : (kk * size + j);
            unsigned int bi = tb ? (kk * size + i) : (i * size + kk);
            if (tc == 1u) {
                dacc += ((const double*)a)[ai] * ((const double*)b)[bi];
            } else {
                facc += v_load(a, ai, tc) * v_load(b, bi, tc);
            }
        }
        float cv = v_load(c, (unsigned long long)i * size + j, tc);
        float refv = (tc == 1u) ? (float)dacc : facc;
        if (!v_isfinite(cv) || !v_isfinite(refv)) {
            nf = 1u;
        } else {
            float diff = refv - cv;
            if (diff < 0.0f) diff = -diff;
            float arefv = refv >= 0.0f ? refv : -refv;
            if (diff > atol + rtol * arefv) {
                mm = 1u;
                // Non-negative floats order like their int bit pattern.
                atomicMax((int*)&out->max_abs_diff_bits, __float_as_int(diff));
                atomicCAS(&out->first_bad, 0xFFFFFFFFu, k);
            }
        }
    }
    if (mm) atomicAdd(&out->mismatch, mm);
    if (nf) atomicAdd(&out->nonfinite, nf);
    if (ck) atomicAdd(&out->checked, ck);
    if (threadIdx.x == 0) atomicAdd(&out->done, 1u);
}

// Gather sampled IntAlu ingredients + outputs so the host can recompute the
// chain reference without keeping a device copy of the input vector.
extern "C" __global__ void intalu_gather(
    const int* in, const int* out, unsigned int n,
    const unsigned int* samples, unsigned int m, unsigned int* gathered)
{
    unsigned int k = blockIdx.x * blockDim.x + threadIdx.x;
    if (k >= m) return;
    unsigned int idx = samples[k];
    unsigned int bidx = ((idx ^ 1u) < n) ? (idx ^ 1u) : 0u;
    gathered[3u * k] = (unsigned int)in[idx];
    gathered[3u * k + 1u] = (unsigned int)in[bidx];
    gathered[3u * k + 2u] = (unsigned int)out[idx];
}
"#;

/// Per-lane deterministic pattern seed (varies within one op across lanes).
pub(super) fn lane_seed(seed: u64, lane: usize) -> u64 {
    seed ^ (lane as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

// SAFETY: the device-copy marker impls for VerifyReport/GemmStats/SampleStats
// live in the defining module (`cli_stressor_cuda_rs::verify`) so they count
// as local under the crate's `extern crate self` alias.

pub struct VerifyEngine {
    _module: Arc<CudaModule>,
    fill_fn: CudaFunction,
    compare_fn: CudaFunction,
    inject_fn: CudaFunction,
    stats_fn: CudaFunction,
    sample_fn: CudaFunction,
    gather_fn: CudaFunction,
    /// Default stream, used by the self-test.
    stream: Arc<CudaStream>,
    /// One report slot per lane for the pattern-compare checks.
    reports: RefCell<Vec<CudaSlice<VerifyReport>>>,
    stats: RefCell<CudaSlice<GemmStats>>,
    sample_stats: RefCell<CudaSlice<SampleStats>>,
    /// Self-test scratch (2^20 words = 4 MiB, > the 0xADBA injection index).
    scratch: CudaSlice<u32>,
    /// Cumulative detector counters, drained by the dispatch loop.
    drained: RefCell<DetectorStats>,
    /// First hard failure observed by any detector (sticky until drained).
    failure: RefCell<Option<String>>,
    self_test: RefCell<Option<SelfTestReport>>,
}

impl VerifyEngine {
    pub fn build(
        ctx: &Arc<CudaContext>,
        info: &DeviceInfo,
        lane_count: usize,
    ) -> Result<Self, BackendError> {
        let arch = nvrtc_arch_for(info);
        let arch_static: &'static str = Box::leak(arch.into_boxed_str());
        let opts = CompileOptions {
            arch: Some(arch_static),
            ..Default::default()
        };
        let ptx = compile_ptx_with_opts(VERIFY_SRC, opts)
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let (module, fill_fn) = load_kernel(ctx, ptx, "verify_pattern_fill")?;
        let lookup = |name: &str| -> Result<CudaFunction, BackendError> {
            module
                .load_function(name)
                .map_err(|err| BackendError::Other(err.to_string()))
        };
        let compare_fn = lookup("verify_compare")?;
        let inject_fn = lookup("verify_inject_error")?;
        let stats_fn = lookup("gemm_stats_reduce")?;
        let sample_fn = lookup("gemm_sample_check")?;
        let gather_fn = lookup("intalu_gather")?;

        let stream = ctx.default_stream();
        let mut reports = Vec::with_capacity(lane_count);
        for _ in 0..lane_count {
            reports.push(
                stream
                    .alloc_zeros::<VerifyReport>(1)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
        }
        let stats = stream
            .alloc_zeros::<GemmStats>(1)
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let sample_stats = stream
            .alloc_zeros::<SampleStats>(1)
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let scratch = stream
            .alloc_zeros::<u32>(SAMPLE_SCRATCH_WORDS)
            .map_err(|err| BackendError::Other(err.to_string()))?;

        Ok(Self {
            _module: module,
            fill_fn,
            compare_fn,
            inject_fn,
            stats_fn,
            sample_fn,
            gather_fn,
            stream,
            reports: RefCell::new(reports),
            stats: RefCell::new(stats),
            sample_stats: RefCell::new(sample_stats),
            scratch,
            drained: RefCell::new(DetectorStats::default()),
            failure: RefCell::new(None),
            self_test: RefCell::new(None),
        })
    }

    // ---- pattern-compare primitives (all async launches; caller syncs) ----

    fn reset_report(&self, stream: &Arc<CudaStream>, lane: usize) -> Result<(), BackendError> {
        let mut reports = self.reports.borrow_mut();
        let report = &mut reports[lane];
        stream
            .memcpy_htod(&[VerifyReport::init()], report)
            .map_err(|err| BackendError::Other(err.to_string()))
    }

    fn launch_fill(
        &self,
        stream: &Arc<CudaStream>,
        buf: &CudaSlice<u32>,
        seed: u64,
    ) -> Result<u32, BackendError> {
        let n = buf.len() as u64;
        let cfg = LaunchConfig::for_num_elems(n as u32);
        unsafe {
            stream
                .launch_builder(&self.fill_fn)
                .arg(buf)
                .arg(&n)
                .arg(&seed)
                .launch(cfg)
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        Ok(cfg.grid_dim.0)
    }

    fn launch_compare(
        &self,
        stream: &Arc<CudaStream>,
        buf: &CudaSlice<u32>,
        seed: u64,
        lane: usize,
    ) -> Result<u32, BackendError> {
        let n = buf.len() as u64;
        let cfg = LaunchConfig::for_num_elems(n as u32);
        let mut reports = self.reports.borrow_mut();
        let report = &mut reports[lane];
        unsafe {
            stream
                .launch_builder(&self.compare_fn)
                .arg(buf)
                .arg(&n)
                .arg(&seed)
                .arg(&mut *report)
                .launch(cfg)
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        Ok(cfg.grid_dim.0)
    }

    fn read_report(
        &self,
        stream: &Arc<CudaStream>,
        lane: usize,
    ) -> Result<VerifyReport, BackendError> {
        let reports = self.reports.borrow();
        let host: Vec<VerifyReport> = stream
            .clone_dtoh(&reports[lane])
            .map_err(|err| BackendError::Other(err.to_string()))?;
        Ok(host[0])
    }

    fn absorb_pattern(&self, report: &VerifyReport, n_words: u64, expected_done: u32, label: &str) {
        let mut drained = self.drained.borrow_mut();
        drained.ops_checked += 1;
        drained.elements_checked += n_words;
        if report.done != expected_done {
            self.set_failure(format!(
                "{label}: report completion counter mismatch (done={}, expected={})",
                report.done, expected_done
            ));
            return;
        }
        if report.total_errors > 0 {
            let bits: Vec<String> = report
                .bit_hist
                .iter()
                .enumerate()
                .filter(|&(_, count)| *count > 0)
                .map(|(bit, count)| format!("bit{bit}={count}"))
                .collect();
            self.set_failure(format!(
                "{label}: {} wrong words (idx {}..{}, exp=0x{:08X} act=0x{:08X}, bits: {})",
                report.total_errors,
                report.idx_min,
                report.idx_max,
                report.first_exp,
                report.first_act,
                if bits.is_empty() {
                    "none".into()
                } else {
                    bits.join(",")
                }
            ));
        }
    }

    fn set_failure(&self, msg: String) {
        let mut failure = self.failure.borrow_mut();
        if failure.is_none() {
            self.drained.borrow_mut().first_error = Some(msg.clone());
            *failure = Some(msg);
        }
    }

    // ---- GEMM checks (stats scan + sampled dot-product cross-check) ----

    /// Run both GEMM checks on one lane's buffers. `tc`: 0=f32, 1=f64, 2=f16,
    /// 3=bf16. `samples_host` holds 2·m little-endian (i, j) index pairs.
    #[allow(clippy::too_many_arguments)]
    pub fn gemm_check<T: DeviceRepr + ValidAsZeroBits>(
        &self,
        stream: &Arc<CudaStream>,
        a: &CudaSlice<T>,
        b: &CudaSlice<T>,
        c: &CudaSlice<T>,
        size: usize,
        ta: bool,
        tb: bool,
        tc: u32,
        samples_host: &[u32],
        atol: f32,
        rtol: f32,
    ) -> Result<(), BackendError> {
        let n = (size * size) as u64;

        // Stats pass (grid-stride, block-reduced).
        {
            let mut stats = self.stats.borrow_mut();
            stream
                .memcpy_htod(&[GemmStats::init()], &mut *stats)
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        let mut scfg = LaunchConfig::for_num_elems(n as u32);
        if scfg.grid_dim.0 > STATS_GRID_CAP {
            scfg.grid_dim.0 = STATS_GRID_CAP;
        }
        unsafe {
            let mut stats = self.stats.borrow_mut();
            stream
                .launch_builder(&self.stats_fn)
                .arg(a)
                .arg(&n)
                .arg(&tc)
                .arg(&mut *stats)
                .launch(scfg)
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        stream
            .synchronize()
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let stats = stream
            .clone_dtoh(&*self.stats.borrow())
            .map_err(|err| BackendError::Other(err.to_string()))?[0];

        {
            let mut drained = self.drained.borrow_mut();
            drained.ops_checked += 1;
            drained.elements_checked += n;
            drained.nonfinite += stats.nonfinite as u64;
        }
        if stats.done != scfg.grid_dim.0 {
            self.set_failure(format!(
                "gemm stats: completion counter mismatch (done={}, expected={})",
                stats.done, scfg.grid_dim.0
            ));
        }
        if stats.nonfinite > 0 {
            self.set_failure(format!(
                "gemm output scan: {}/{} non-finite values (abs_sum={:.4e}, zero={})",
                stats.nonfinite, n, stats.abs_sum, stats.zero
            ));
        }

        // Sampled cross-check pass.
        if samples_host.is_empty() {
            return Ok(());
        }
        eprintln!(
            "[DBG] size={} ta={} tb={} tc={} samples[0..8]={:?} atol={} rtol={}",
            size,
            ta,
            tb,
            tc,
            &samples_host[..samples_host.len().min(8)],
            atol,
            rtol
        );
        let m = samples_host.len() / 2;
        let samples_dev = stream
            .clone_htod(samples_host)
            .map_err(|err| BackendError::Other(err.to_string()))?;
        {
            let mut sample_stats = self.sample_stats.borrow_mut();
            stream
                .memcpy_htod(&[SampleStats::init()], &mut *sample_stats)
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        let scfg2 = LaunchConfig::for_num_elems(m as u32);
        let ta_u = ta as u32;
        let tb_u = tb as u32;
        let size_u = size as u32;
        let m_u = m as u32;
        unsafe {
            let mut sample_stats = self.sample_stats.borrow_mut();
            stream
                .launch_builder(&self.sample_fn)
                .arg(a)
                .arg(b)
                .arg(c)
                .arg(&size_u)
                .arg(&ta_u)
                .arg(&tb_u)
                .arg(&tc)
                .arg(&samples_dev)
                .arg(&m_u)
                .arg(&atol)
                .arg(&rtol)
                .arg(&mut *sample_stats)
                .launch(scfg2)
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        stream
            .synchronize()
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let sample = stream
            .clone_dtoh(&*self.sample_stats.borrow())
            .map_err(|err| BackendError::Other(err.to_string()))?[0];

        {
            let mut drained = self.drained.borrow_mut();
            drained.elements_checked += sample.checked as u64;
            drained.mismatches += sample.mismatch as u64;
            drained.nonfinite += sample.nonfinite as u64;
        }
        if sample.done != scfg2.grid_dim.0 {
            self.set_failure(format!(
                "gemm sample check: completion counter mismatch (done={}, expected={})",
                sample.done, scfg2.grid_dim.0
            ));
        }
        if sample.nonfinite > 0 {
            self.set_failure(format!(
                "gemm sample check: {} sampled outputs non-finite",
                sample.nonfinite
            ));
        }
        if sample.mismatch > 0 {
            self.set_failure(format!(
                "gemm sample check: {}/{} sampled outputs exceed tolerance (first_bad={}, max_abs_diff={:.4e}, atol={:.2e}, rtol={:.2e})",
                sample.mismatch,
                sample.checked,
                if sample.first_bad == u32::MAX { 0 } else { sample.first_bad },
                sample.max_abs_diff(),
                atol,
                rtol
            ));
        }
        Ok(())
    }

    /// Gather sampled IntAlu (input_a, input_b, output) triples.
    pub(super) fn intalu_gather(
        &self,
        stream: &Arc<CudaStream>,
        input: &CudaSlice<i32>,
        output: &CudaSlice<i32>,
        samples_host: &[u32],
    ) -> Result<Vec<u32>, BackendError> {
        if samples_host.is_empty() {
            return Ok(Vec::new());
        }
        let m = samples_host.len() as u32;
        let samples_dev = stream
            .clone_htod(samples_host)
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let mut gathered = stream
            .alloc_zeros::<u32>(3 * samples_host.len())
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let cfg = LaunchConfig::for_num_elems(m);
        let n = input.len() as u32;
        unsafe {
            stream
                .launch_builder(&self.gather_fn)
                .arg(input)
                .arg(output)
                .arg(&n)
                .arg(&samples_dev)
                .arg(&m)
                .arg(&mut gathered)
                .launch(cfg)
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        stream
            .synchronize()
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let host: Vec<u32> = stream
            .clone_dtoh(&gathered)
            .map_err(|err| BackendError::Other(err.to_string()))?;
        Ok(host)
    }

    /// Record IntAlu sampled-compare outcomes (host-side reference compare).
    pub(super) fn absorb_intalu(&self, checked: u64, mismatches: u64, detail: Option<String>) {
        {
            let mut drained = self.drained.borrow_mut();
            drained.ops_checked += 1;
            drained.elements_checked += checked;
            drained.mismatches += mismatches;
        }
        if let Some(msg) = detail {
            self.set_failure(msg);
        }
    }

    /// Diagnostics-only: run just the sampled cross-check and return stats.
    #[allow(clippy::too_many_arguments)]
    pub fn debug_sample_check<T: DeviceRepr + ValidAsZeroBits>(
        &self,
        stream: &Arc<CudaStream>,
        a: &CudaSlice<T>,
        b: &CudaSlice<T>,
        c: &CudaSlice<T>,
        size: usize,
        ta: bool,
        tb: bool,
        tc: u32,
        samples_host: &[u32],
        atol: f32,
        rtol: f32,
    ) -> Result<SampleStats, BackendError> {
        let m = samples_host.len() / 2;
        let samples_dev = stream
            .clone_htod(samples_host)
            .map_err(|err| BackendError::Other(err.to_string()))?;
        stream
            .memcpy_htod(&[SampleStats::init()], &mut *self.sample_stats.borrow_mut())
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let scfg = LaunchConfig::for_num_elems(m as u32);
        let (ta_u, tb_u, size_u, m_u) = (ta as u32, tb as u32, size as u32, m as u32);
        unsafe {
            let mut sample_stats = self.sample_stats.borrow_mut();
            stream
                .launch_builder(&self.sample_fn)
                .arg(a)
                .arg(b)
                .arg(c)
                .arg(&size_u)
                .arg(&ta_u)
                .arg(&tb_u)
                .arg(&tc)
                .arg(&samples_dev)
                .arg(&m_u)
                .arg(&atol)
                .arg(&rtol)
                .arg(&mut *sample_stats)
                .launch(scfg)
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        stream
            .synchronize()
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let sample = stream
            .clone_dtoh(&*self.sample_stats.borrow())
            .map_err(|err| BackendError::Other(err.to_string()))?[0];
        Ok(sample)
    }

    /// HYDRA-style injection self-test: prove the detection pipeline catches
    /// exactly one known injected error and nothing else.
    pub fn run_self_test(&self) {
        let stream = self.stream.clone();
        let mut checks = Vec::new();
        let seed: u64 = 0xADBA;

        // Gate 1: clean pattern reports zero errors.
        let clean = (|| -> Result<(VerifyReport, u32), BackendError> {
            self.reset_report(&stream, 0)?;
            self.launch_fill(&stream, &self.scratch, seed)?;
            let cmp_done = self.launch_compare(&stream, &self.scratch, seed, 0)?;
            stream
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let report = self.read_report(&stream, 0)?;
            Ok((report, cmp_done))
        })();
        checks.push(match clean {
            Ok((report, cmp_done)) => {
                let passed = report.total_errors == 0 && report.done == cmp_done;
                SelfTestCheck {
                    name: "pattern_clean".into(),
                    passed,
                    detail: format!(
                        "total_errors={}, done={} (expected {cmp_done})",
                        report.total_errors, report.done
                    ),
                }
            }
            Err(err) => SelfTestCheck {
                name: "pattern_clean".into(),
                passed: false,
                detail: format!("kernel error: {err}"),
            },
        });

        // Gate 2: single injected error at 0xADBA / bit 22 is caught exactly.
        let injected = (|| -> Result<VerifyReport, BackendError> {
            self.reset_report(&stream, 0)?;
            // Re-fill so the gate does not depend on gate 1's outcome.
            self.launch_fill(&stream, &self.scratch, seed)?;
            stream
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let idx = VerifyReport::SELF_TEST_IDX;
            let mask = 1u32 << VerifyReport::SELF_TEST_BIT;
            {
                let one = LaunchConfig {
                    grid_dim: (1, 1, 1),
                    block_dim: (1, 1, 1),
                    shared_mem_bytes: 0,
                };
                unsafe {
                    // Shared-ref arg push on our own scratch-write kernel: the
                    // driver only needs the device address.
                    stream
                        .launch_builder(&self.inject_fn)
                        .arg(&self.scratch)
                        .arg(&idx)
                        .arg(&mask)
                        .launch(one)
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
            }
            let cmp_done = self.launch_compare(&stream, &self.scratch, seed, 0)?;
            stream
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let report = self.read_report(&stream, 0)?;
            // Restore the scratch to the clean pattern for later reuse.
            self.launch_fill(&stream, &self.scratch, seed)?;
            stream
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let _ = cmp_done;
            Ok(report)
        })();
        checks.push(match injected {
            Ok(report) => {
                let want_idx = VerifyReport::SELF_TEST_IDX;
                let want_bit = VerifyReport::SELF_TEST_BIT;
                let bit_hist_clean = report
                    .bit_hist
                    .iter()
                    .enumerate()
                    .all(|(bit, &count)| {
                        count == if bit as u32 == want_bit { 1 } else { 0 }
                    });
                let passed = report.total_errors == 1
                    && report.idx_min == want_idx
                    && report.idx_max == want_idx
                    && (report.first_exp ^ report.first_act) == (1u32 << want_bit)
                    && bit_hist_clean;
                SelfTestCheck {
                    name: "injection_capture".into(),
                    passed,
                    detail: format!(
                        "total_errors={}, idx={}..{} (want {}..{}), xor=0x{:08X} (want 0x{:08X}), bit_hist[{}]={}",
                        report.total_errors,
                        report.idx_min,
                        report.idx_max,
                        want_idx,
                        want_idx,
                        report.first_exp ^ report.first_act,
                        1u32 << want_bit,
                        want_bit,
                        report.bit_hist[want_bit as usize]
                    ),
                }
            }
            Err(err) => SelfTestCheck {
                name: "injection_capture".into(),
                passed: false,
                detail: format!("kernel error: {err}"),
            },
        });

        let report = SelfTestReport {
            passed: checks.iter().all(|check| check.passed),
            checks,
        };
        *self.self_test.borrow_mut() = Some(report);
    }

    pub(super) fn take_self_test(&self) -> Option<SelfTestReport> {
        self.self_test.borrow().clone()
    }

    // ---- public-in-crate accessors for the stress paths ----

    pub(super) fn failure(&self) -> Option<String> {
        self.failure.borrow().clone()
    }

    pub(super) fn drain(&self) -> DetectorStats {
        *self.failure.borrow_mut() = None;
        std::mem::take(&mut self.drained.borrow_mut())
    }

    /// Async pattern fill (no sync; caller orders and syncs).
    pub(super) fn fill(
        &self,
        stream: &Arc<CudaStream>,
        buf: &CudaSlice<u32>,
        seed: u64,
    ) -> Result<(), BackendError> {
        self.launch_fill(stream, buf, seed).map(|_| ())
    }

    /// Async compare into the lane report; returns the expected `done` delta.
    pub(super) fn compare(
        &self,
        stream: &Arc<CudaStream>,
        buf: &CudaSlice<u32>,
        seed: u64,
        lane: usize,
    ) -> Result<u32, BackendError> {
        self.launch_compare(stream, buf, seed, lane)
    }

    pub(super) fn reset_lane_report(
        &mut self,
        stream: &Arc<CudaStream>,
        lane: usize,
    ) -> Result<(), BackendError> {
        self.reset_report(stream, lane)
    }

    pub(super) fn lane_report(
        &self,
        stream: &Arc<CudaStream>,
        lane: usize,
    ) -> Result<VerifyReport, BackendError> {
        self.read_report(stream, lane)
    }

    pub(super) fn absorb(
        &self,
        report: &VerifyReport,
        n_words: u64,
        expected_done: u32,
        label: &str,
    ) {
        self.absorb_pattern(report, n_words, expected_done, label);
    }
}
