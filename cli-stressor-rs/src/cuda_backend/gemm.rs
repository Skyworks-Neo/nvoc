//! GEMM stress path — cuBLAS matrix multiply across the FP precisions.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::time::Instant;

use cudarc::cublas::{Gemm, GemmConfig, result as cublas_res, sys as cublas_sys};
use cudarc::driver::{DevicePtr, DevicePtrMut, DeviceRepr};
use half::{bf16, f16};

use cli_stressor_rs::{
    BackendError, PrecisionKind, PrecisionSpec, StreamMode, VerifyConfig, choose_tolerance,
    cpu_reference_int_tn, cpu_reference_int4_tn, int4_pack, make_random_host_matrix,
};
use rayon::prelude::*;
// FP8/FP4 operand codecs are cuda12-only (cuda-11040 has no FP8/FP4 datatype
// bindings and those stress arms are cfg-excluded).
#[cfg(not(feature = "cuda11"))]
use cli_stressor_rs::{Fp4ScaleMode, fp4_quantize_matrix, fp8_e4m3_quantize, fp8_e5m2_quantize};
use cudarc::driver::{LaunchConfig, PushKernelArg, ValidAsZeroBits};

use super::backend::CudaBackend;
use super::gemm_lt::{LtDtype, LtShapeKey};
use super::verify_kernels::lane_seed;

pub(super) struct GemmPathConfig<'a> {
    pub spec: &'a PrecisionSpec,
    pub size: usize,
    pub warmup_iters: u32,
    pub burst_iters: u32,
    pub transpose_prob: f64,
    pub seed: u64,
    pub stream_mode: StreamMode,
    pub verify: VerifyConfig,
}

/// Per-lane operand bundle for one Lt GEMM op (host data + FP4 block scales).
struct LtOpData<AB> {
    a: Vec<AB>,
    b: Vec<AB>,
    scale_a: Vec<u8>,
    scale_b: Vec<u8>,
}

/// Device-side operand generation (`--gpu-generate`): the NVRTC fill kernels
/// quantize uniform [-2, 2] values directly into VRAM, removing the host RNG,
/// the quantize pass, and the H2D copy from the burst pipeline entirely. The
/// per-arm generation closure launches the dtype's fill kernels; the runner
/// stays generic over the operand element type.
struct LtDeviceOps<AB> {
    a: cudarc::driver::CudaSlice<AB>,
    b: cudarc::driver::CudaSlice<AB>,
    scale_a: Option<cudarc::driver::CudaSlice<u8>>,
    scale_b: Option<cudarc::driver::CudaSlice<u8>>,
}

/// Fill-kernel launch signature shared by the FP8/FP4 arms (kernels + stream
/// are handed over by the runner to avoid re-borrowing the backend).
type LtDeviceGen<'a, AB> = dyn Fn(
        &super::gpu_fill::GpuFillKernels,
        &std::sync::Arc<cudarc::driver::CudaStream>,
        u64,
        usize,
    ) -> Result<LtDeviceOps<AB>, BackendError>
    + 'a;

/// Tolerance for the sampled cross-check, scaled for the accumulation length:
/// rounding error of a K-term dot product grows ~√K relative to the 1024-wide
/// sidecar the base table was calibrated at.
fn sample_tolerance(spec: &PrecisionSpec, size: usize) -> (f32, f32) {
    let (atol, rtol) = choose_tolerance(spec.name);
    let scale = (size as f32 / 1024.0).sqrt().max(1.0);
    (atol * scale, rtol)
}

impl CudaBackend {
    /// Ride-on-load verification for one finished GEMM burst: a full scan of
    /// the C buffers (non-finite / zero / |C| sum) plus sampled dot-product
    /// cross-checks recomputed from the same A/B the GEMM consumed. `AB` is
    /// the operand element type, `CT` the C output element type (they differ
    /// for the cublasLt FP8/FP4 paths: u8 operands, f16 outputs).
    #[allow(clippy::too_many_arguments)]
    fn run_gemm_verify<AB: DeviceRepr + ValidAsZeroBits, CT: DeviceRepr + ValidAsZeroBits>(
        &mut self,
        spec: &PrecisionSpec,
        a_devs: &[cudarc::driver::CudaSlice<AB>],
        b_devs: &[cudarc::driver::CudaSlice<AB>],
        c_devs: &[cudarc::driver::CudaSlice<CT>],
        size: usize,
        ta: bool,
        tb: bool,
        tc: u32,
        ab_code: u32,
        scale_a: Option<&[cudarc::driver::CudaSlice<u8>]>,
        scale_b: Option<&[cudarc::driver::CudaSlice<u8>]>,
        seed: u64,
        verify: VerifyConfig,
    ) -> Result<(), BackendError> {
        if self.verify.is_none() || !verify.enabled || !verify.due(verify.gemm_every, seed) {
            return Ok(());
        }
        let scale_cols = size as u32;
        let streams: Vec<_> = (0..a_devs.len())
            .map(|l| self.stream_for_lane(l).clone())
            .collect();
        let engine = self.verify.as_mut().expect("checked above");
        // Small (L2-resident) outputs get the full-coverage recompute —
        // 100% of elements instead of the sampled subset (closes the
        // sampling blind spot for narrow SDC bands).
        if verify.full_check_max_size > 0 && size <= verify.full_check_max_size {
            let (atol_f, rtol_f) = sample_tolerance(spec, size);
            for lane in 0..a_devs.len() {
                engine.gemm_full_check(
                    &streams[lane],
                    &a_devs[lane],
                    &b_devs[lane],
                    &c_devs[lane],
                    size,
                    ta,
                    tb,
                    tc,
                    ab_code,
                    scale_a.map(|s| &s[lane]),
                    scale_b.map(|s| &s[lane]),
                    scale_cols,
                    atol_f,
                    rtol_f,
                )?;
            }
            return Ok(());
        }
        let m = (verify.gemm_samples as usize).min(size * size);
        let (atol, rtol) = sample_tolerance(spec, size);
        let samples: Vec<u32> = {
            let mut rng = StdRng::seed_from_u64(seed ^ 0x5EED_5EED_0001);
            (0..2 * m)
                .map(|_| rng.random::<u32>() % size as u32)
                .collect()
        };
        for lane in 0..a_devs.len() {
            engine.gemm_check(
                &streams[lane],
                &a_devs[lane],
                &b_devs[lane],
                &c_devs[lane],
                size,
                ta,
                tb,
                tc,
                ab_code,
                scale_a.map(|s| &s[lane]),
                scale_b.map(|s| &s[lane]),
                scale_cols,
                &samples,
                atol,
                rtol,
            )?;
        }
        Ok(())
    }
}

impl CudaBackend {
    /// Shared cublasLt GEMM stress runner: per-lane buffer prep via the
    /// dtype-specific `prep` closure, plan build with the first-encounter
    /// `[GEMM-LT]` algo report (candidate timing under `--gemm-algo-list`),
    /// warmup/burst loops, and the ride-on-load verify. `tc_code`/`ab_code`
    /// are the verify kernels' C/operand type codes.
    #[allow(clippy::too_many_arguments)]
    fn run_gemm_path_lt<AB, CT, Prep>(
        &mut self,
        dtype: LtDtype,
        spec: &PrecisionSpec,
        size: usize,
        ta: bool,
        tb: bool,
        fast_accum: bool,
        warmup_iters: u32,
        burst_iters: u32,
        stream_mode: StreamMode,
        seed: u64,
        rng: &mut StdRng,
        verify: VerifyConfig,
        verify_operands: bool,
        tc_code: u32,
        ab_code: u32,
        device_gen: Option<&LtDeviceGen<'_, AB>>,
        mut prep: Prep,
    ) -> Result<f64, BackendError>
    where
        AB: DeviceRepr + ValidAsZeroBits,
        CT: DeviceRepr + ValidAsZeroBits,
        Prep: FnMut(&mut StdRng, usize) -> LtOpData<AB>,
    {
        let lane_count = Self::lane_count(stream_mode);
        let key = LtShapeKey {
            dtype,
            size: size as u32,
            ta,
            tb,
            fast_accum,
        };

        // Per-lane operand buffers (FP4 also carries one scale byte per
        // 16/32-element block). With `--gpu-generate` the quantized operands
        // come straight from the device fill kernels; otherwise the host
        // prep closure generates + quantizes + uploads.
        let mut a_devs = Vec::with_capacity(lane_count);
        let mut b_devs = Vec::with_capacity(lane_count);
        let mut c_devs = Vec::with_capacity(lane_count);
        let mut scale_a_devs: Option<Vec<cudarc::driver::CudaSlice<u8>>> = None;
        let mut scale_b_devs: Option<Vec<cudarc::driver::CudaSlice<u8>>> = None;
        for lane in 0..lane_count {
            let stream = self.stream_for_lane(lane);
            if let Some(fill) = device_gen {
                let kernels = self.gpu_fill.as_ref().ok_or_else(|| {
                    BackendError::Other("gpu fill kernels unavailable".to_string())
                })?;
                let ops = fill(kernels, stream, lane_seed(seed, lane), size)?;
                a_devs.push(ops.a);
                b_devs.push(ops.b);
                if let Some(sa) = ops.scale_a {
                    scale_a_devs.get_or_insert_with(Vec::new).push(sa);
                }
                if let Some(sb) = ops.scale_b {
                    scale_b_devs.get_or_insert_with(Vec::new).push(sb);
                }
            } else {
                let op = prep(rng, size);
                a_devs.push(
                    stream
                        .clone_htod(&op.a)
                        .map_err(|err| BackendError::Other(err.to_string()))?,
                );
                b_devs.push(
                    stream
                        .clone_htod(&op.b)
                        .map_err(|err| BackendError::Other(err.to_string()))?,
                );
                if !op.scale_a.is_empty() {
                    scale_a_devs.get_or_insert_with(Vec::new).push(
                        stream
                            .clone_htod(&op.scale_a)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                }
                if !op.scale_b.is_empty() {
                    scale_b_devs.get_or_insert_with(Vec::new).push(
                        stream
                            .clone_htod(&op.scale_b)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                }
            }
            c_devs.push(
                stream
                    .alloc_zeros::<CT>(size * size)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
        }

        // First-encounter plan build + algo report. The heuristic query runs
        // on lane 0; other lanes build their own (identical) plans lazily at
        // first launch. Zero candidates prints no report (the Unsupported
        // error from the first launch carries the diagnosis) — the key is
        // still marked so the report does not repeat per op.
        let candidates = self.lt_for_lane_mut(0).heuristic_count(key)?;
        let selected = self.lt_options().algo_index.unwrap_or(0);
        if !self.lt_reported.contains(&key) {
            self.lt_reported.insert(key);
            if candidates > 0 {
                let config = self
                    .lt_for_lane(0)
                    .algo_config_at(key, selected.min(candidates - 1))
                    .unwrap_or_else(|| "n/a".to_string());
                let line = format!(
                    "[GEMM-LT] {} {}x{} ta={} tb={} | algo #{}/{}: {} ws={:.1}MiB",
                    dtype.name(),
                    size,
                    size,
                    if ta { "T" } else { "N" },
                    if tb { "T" } else { "N" },
                    selected,
                    candidates,
                    config,
                    self.lt_for_lane(0).workspace_mib(),
                );
                println!("{}", line);
                self.gemm_algo_reports
                    .push(line.trim_start_matches("[GEMM-LT] ").to_string());
                if self.lt_options().listing && candidates > 1 {
                    self.list_gemm_lt_candidates(
                        key,
                        candidates,
                        &a_devs[0],
                        &b_devs[0],
                        &mut c_devs[0],
                        scale_a_devs.as_ref().map(|s| &s[0]),
                        scale_b_devs.as_ref().map(|s| &s[0]),
                        dtype,
                    )?;
                }
            }
        }

        // alpha/beta: f32 for the float compute types, f64 for COMPUTE_64F
        // (its scale type is CUDA_R_64F), i32 for COMPUTE_32I.
        let alpha_f: f32 = 1.0;
        let beta_f: f32 = 0.0;
        let alpha_d: f64 = 1.0;
        let beta_d: f64 = 0.0;
        let alpha_i: i32 = 1;
        let beta_i: i32 = 0;
        let (alpha_ptr, beta_ptr): (*const std::ffi::c_void, *const std::ffi::c_void) = match dtype
        {
            LtDtype::Int8 | LtDtype::Int4 => (
                (&alpha_i as *const i32) as *const _,
                (&beta_i as *const i32) as *const _,
            ),
            LtDtype::F64 => (
                (&alpha_d as *const f64) as *const _,
                (&beta_d as *const f64) as *const _,
            ),
            _ => (
                (&alpha_f as *const f32) as *const _,
                (&beta_f as *const f32) as *const _,
            ),
        };

        // One launch on `lane` (device-pointer guards scoped to the launch).
        let mut launch_lane = |backend: &mut CudaBackend,
                               lane: usize,
                               algo_index: usize|
         -> Result<(), BackendError> {
            let stream = backend.stream_for_lane(lane).clone();
            let (a_ptr, _ga) = a_devs[lane].device_ptr(&stream);
            let (b_ptr, _gb) = b_devs[lane].device_ptr(&stream);
            let (c_ptr, _gc) = c_devs[lane].device_ptr_mut(&stream);
            let sa = scale_a_devs.as_ref().map(|s| s[lane].device_ptr(&stream));
            let sb = scale_b_devs.as_ref().map(|s| s[lane].device_ptr(&stream));
            backend.lt_for_lane_mut(lane).launch(
                &stream,
                key,
                algo_index,
                a_ptr as *const _,
                b_ptr as *const _,
                c_ptr as *mut _,
                sa.map(|(p, _)| p as *const _),
                sb.map(|(p, _)| p as *const _),
                alpha_ptr,
                beta_ptr,
            )
        };

        for _ in 0..warmup_iters {
            for lane in 0..lane_count {
                launch_lane(self, lane, selected)?;
            }
        }
        for lane in 0..lane_count {
            self.stream_for_lane(lane)
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        let op_start = Instant::now();
        for _ in 0..burst_iters {
            for lane in 0..lane_count {
                launch_lane(self, lane, selected)?;
            }
        }
        for lane in 0..lane_count {
            self.stream_for_lane(lane)
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }

        if verify_operands {
            self.run_gemm_verify(
                spec,
                &a_devs,
                &b_devs,
                &c_devs,
                size,
                ta,
                tb,
                tc_code,
                ab_code,
                scale_a_devs.as_deref(),
                scale_b_devs.as_deref(),
                seed,
                verify,
            )?;
        }
        Ok(op_start.elapsed().as_secs_f64())
    }

    /// `--gemm-algo-list`: time every heuristic candidate for a freshly met
    /// shape (lane 0, fixed iters each) and print a TFLOPS table — the
    /// built-in form of the unlocked-ada-gemm "enumerate + time" probe for
    /// diagnosing suboptimal kernel selection (e.g. the FP16 bimodal 49/63
    /// TF readings on unlocked cards).
    #[allow(clippy::too_many_arguments)]
    fn list_gemm_lt_candidates<AB, CT>(
        &mut self,
        key: LtShapeKey,
        candidates: usize,
        a: &cudarc::driver::CudaSlice<AB>,
        b: &cudarc::driver::CudaSlice<AB>,
        c: &mut cudarc::driver::CudaSlice<CT>,
        scale_a: Option<&cudarc::driver::CudaSlice<u8>>,
        scale_b: Option<&cudarc::driver::CudaSlice<u8>>,
        dtype: LtDtype,
    ) -> Result<(), BackendError>
    where
        AB: DeviceRepr + ValidAsZeroBits,
        CT: DeviceRepr + ValidAsZeroBits,
    {
        const LIST_ITERS: u32 = 3;
        println!("[GEMM-LT] candidate list (timed, lane 0, {LIST_ITERS} iters each):");
        let stream = self.stream_for_lane(0).clone();
        let alpha_f: f32 = 1.0;
        let beta_f: f32 = 0.0;
        let alpha_d: f64 = 1.0;
        let beta_d: f64 = 0.0;
        let alpha_i: i32 = 1;
        let beta_i: i32 = 0;
        let (alpha_ptr, beta_ptr): (*const std::ffi::c_void, *const std::ffi::c_void) = match dtype
        {
            LtDtype::Int8 | LtDtype::Int4 => (
                (&alpha_i as *const i32) as *const _,
                (&beta_i as *const i32) as *const _,
            ),
            LtDtype::F64 => (
                (&alpha_d as *const f64) as *const _,
                (&beta_d as *const f64) as *const _,
            ),
            _ => (
                (&alpha_f as *const f32) as *const _,
                (&beta_f as *const f32) as *const _,
            ),
        };
        let (a_ptr, _ga) = a.device_ptr(&stream);
        let (b_ptr, _gb) = b.device_ptr(&stream);
        let (c_ptr, _gc) = c.device_ptr_mut(&stream);
        // Pointers captured once (the SyncOnDrop guards stay alive in `sa`/`sb`
        // for the whole listing).
        let sa = scale_a.map(|s| s.device_ptr(&stream));
        let sb = scale_b.map(|s| s.device_ptr(&stream));
        let sa_ptr = sa
            .as_ref()
            .map_or(std::ptr::null::<std::ffi::c_void>(), |(p, _)| {
                *p as *const std::ffi::c_void
            });
        let sb_ptr = sb
            .as_ref()
            .map_or(std::ptr::null::<std::ffi::c_void>(), |(p, _)| {
                *p as *const std::ffi::c_void
            });
        // Only FP4 shapes carry scale pointers; None keeps the descriptor
        // free of A/B_SCALE_POINTER attributes on the other dtypes.
        let sa_arg = scale_a.is_some().then_some(sa_ptr);
        let sb_arg = scale_b.is_some().then_some(sb_ptr);
        for idx in 0..candidates {
            // One warmup launch per candidate, then the timed window.
            self.lt_for_lane_mut(0).launch(
                &stream,
                key,
                idx,
                a_ptr as *const _,
                b_ptr as *const _,
                c_ptr as *mut _,
                sa_arg,
                sb_arg,
                alpha_ptr,
                beta_ptr,
            )?;
            stream
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let start = Instant::now();
            for _ in 0..LIST_ITERS {
                self.lt_for_lane_mut(0).launch(
                    &stream,
                    key,
                    idx,
                    a_ptr as *const _,
                    b_ptr as *const _,
                    c_ptr as *mut _,
                    sa_arg,
                    sb_arg,
                    alpha_ptr,
                    beta_ptr,
                )?;
            }
            stream
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let elapsed = start.elapsed().as_secs_f64();
            let tflops = if elapsed > 0.0 {
                2.0 * (key.size as f64).powi(3) * LIST_ITERS as f64 / elapsed / 1e12
            } else {
                0.0
            };
            let config = self
                .lt_for_lane(0)
                .algo_config_at(key, idx)
                .unwrap_or_else(|| "n/a".to_string());
            println!("[GEMM-LT]   #{idx}: {tflops:8.2} TFLOPS | {config}");
        }
        Ok(())
    }
}

impl CudaBackend {
    pub(super) fn run_gemm_path(
        &mut self,
        config: GemmPathConfig<'_>,
    ) -> Result<f64, BackendError> {
        let GemmPathConfig {
            spec,
            size,
            warmup_iters,
            burst_iters,
            transpose_prob,
            seed,
            stream_mode,
            verify,
        } = config;
        let mut rng = StdRng::seed_from_u64(seed);
        let transpose_a = rng.random::<f64>() < transpose_prob;
        let transpose_b = rng.random::<f64>() < transpose_prob;
        let lane_count = Self::lane_count(stream_mode);
        let op_a = if transpose_a {
            cublas_sys::cublasOperation_t::CUBLAS_OP_T
        } else {
            cublas_sys::cublasOperation_t::CUBLAS_OP_N
        };
        let op_b = if transpose_b {
            cublas_sys::cublasOperation_t::CUBLAS_OP_T
        } else {
            cublas_sys::cublasOperation_t::CUBLAS_OP_N
        };

        match spec.kind {
            PrecisionKind::BF16 => {
                if self.gemm_legacy_cublas {
                let cfg = GemmConfig {
                    transa: op_a,
                    transb: op_b,
                    m: size as i32,
                    n: size as i32,
                    k: size as i32,
                    alpha: bf16::from_f32(1.0),
                    lda: size as i32,
                    ldb: size as i32,
                    beta: bf16::from_f32(0.0),
                    ldc: size as i32,
                };
                let mut a_devs = Vec::with_capacity(lane_count);
                let mut b_devs = Vec::with_capacity(lane_count);
                for lane in 0..lane_count {
                    let stream = self.stream_for_lane(lane);
                    let a_host = make_random_host_matrix(size, rng.random::<u64>());
                    let b_host = make_random_host_matrix(size, rng.random::<u64>());
                    let a: Vec<bf16> = a_host.data.iter().map(|v| bf16::from_f32(*v)).collect();
                    let b: Vec<bf16> = b_host.data.iter().map(|v| bf16::from_f32(*v)).collect();
                    a_devs.push(
                        stream
                            .clone_htod(&a)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                    b_devs.push(
                        stream
                            .clone_htod(&b)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                }
                // Allocate the output buffers once and reuse them across all
                // iterations. With beta = 0 every gemm overwrites C, so reusing
                // the same buffer is numerically identical, and it removes a
                // synchronous device alloc + zero of size*size per gemm that
                // otherwise serializes the streams and starves the GPU.
                let mut c_devs = Vec::with_capacity(lane_count);
                for lane in 0..lane_count {
                    c_devs.push(
                        self.stream_for_lane(lane)
                            .alloc_zeros::<bf16>(size * size)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                }
                for _ in 0..warmup_iters {
                    for lane in 0..lane_count {
                        let blas = self.blas_for_lane(lane);
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c_devs[lane])
                                .map_err(|err| BackendError::Other(err.to_string()))?;
                        }
                    }
                }
                for lane in 0..lane_count {
                    self.stream_for_lane(lane)
                        .synchronize()
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                let op_start = Instant::now();
                for _ in 0..burst_iters {
                    for lane in 0..lane_count {
                        let blas = self.blas_for_lane(lane);
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c_devs[lane])
                                .map_err(|err| BackendError::Other(err.to_string()))?;
                        }
                    }
                }
                for lane in 0..lane_count {
                    self.stream_for_lane(lane)
                        .synchronize()
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                self.run_gemm_verify(
                    spec,
                    &a_devs,
                    &b_devs,
                    &c_devs,
                    size,
                    transpose_a,
                    transpose_b,
                    3,
                    3,
                    None,
                    None,
                    seed,
                    verify,
                )?;
                Ok(op_start.elapsed().as_secs_f64())
                } else {
                // Default cublasLt lane (see the FP16 arm for the rationale).
                self.run_gemm_path_lt::<bf16, bf16, _>(
                    LtDtype::Bf16,
                    spec,
                    size,
                    transpose_a,
                    transpose_b,
                    false,
                    warmup_iters,
                    burst_iters,
                    stream_mode,
                    seed,
                    &mut rng,
                    verify,
                    true,
                    3,
                    3,
                    None,
                    |rng, sz| {
                        let a_host = make_random_host_matrix(sz, rng.random::<u64>());
                        let b_host = make_random_host_matrix(sz, rng.random::<u64>());
                        LtOpData {
                            a: a_host.data.iter().map(|v| bf16::from_f32(*v)).collect(),
                            b: b_host.data.iter().map(|v| bf16::from_f32(*v)).collect(),
                            scale_a: Vec::new(),
                            scale_b: Vec::new(),
                        }
                    },
                )
                }
            }
            PrecisionKind::FP16 => {
                if self.gemm_legacy_cublas {
                let cfg = GemmConfig {
                    transa: op_a,
                    transb: op_b,
                    m: size as i32,
                    n: size as i32,
                    k: size as i32,
                    alpha: f16::from_f32(1.0),
                    lda: size as i32,
                    ldb: size as i32,
                    beta: f16::from_f32(0.0),
                    ldc: size as i32,
                };
                let mut a_devs = Vec::with_capacity(lane_count);
                let mut b_devs = Vec::with_capacity(lane_count);
                for lane in 0..lane_count {
                    let stream = self.stream_for_lane(lane);
                    let a_host = make_random_host_matrix(size, rng.random::<u64>());
                    let b_host = make_random_host_matrix(size, rng.random::<u64>());
                    let a: Vec<f16> = a_host.data.iter().map(|v| f16::from_f32(*v)).collect();
                    let b: Vec<f16> = b_host.data.iter().map(|v| f16::from_f32(*v)).collect();
                    a_devs.push(
                        stream
                            .clone_htod(&a)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                    b_devs.push(
                        stream
                            .clone_htod(&b)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                }
                // Reuse output buffers across iterations (beta = 0 overwrites C).
                let mut c_devs = Vec::with_capacity(lane_count);
                for lane in 0..lane_count {
                    c_devs.push(
                        self.stream_for_lane(lane)
                            .alloc_zeros::<f16>(size * size)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                }
                for _ in 0..warmup_iters {
                    for lane in 0..lane_count {
                        let blas = self.blas_for_lane(lane);
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c_devs[lane])
                                .map_err(|err| BackendError::Other(err.to_string()))?;
                        }
                    }
                }
                for lane in 0..lane_count {
                    self.stream_for_lane(lane)
                        .synchronize()
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                let op_start = Instant::now();
                for _ in 0..burst_iters {
                    for lane in 0..lane_count {
                        let blas = self.blas_for_lane(lane);
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c_devs[lane])
                                .map_err(|err| BackendError::Other(err.to_string()))?;
                        }
                    }
                }
                for lane in 0..lane_count {
                    self.stream_for_lane(lane)
                        .synchronize()
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                self.run_gemm_verify(
                    spec,
                    &a_devs,
                    &b_devs,
                    &c_devs,
                    size,
                    transpose_a,
                    transpose_b,
                    2,
                    2,
                    None,
                    None,
                    seed,
                    verify,
                )?;
                Ok(op_start.elapsed().as_secs_f64())
                } else {
                // Default cublasLt lane: same CUBLAS_COMPUTE_32F/f32-scale
                // semantics as the classic GemmEx call above, plus the
                // heuristic candidate report and algo pinning.
                self.run_gemm_path_lt::<f16, f16, _>(
                    LtDtype::F16,
                    spec,
                    size,
                    transpose_a,
                    transpose_b,
                    false,
                    warmup_iters,
                    burst_iters,
                    stream_mode,
                    seed,
                    &mut rng,
                    verify,
                    true,
                    2,
                    2,
                    None,
                    |rng, sz| {
                        let a_host = make_random_host_matrix(sz, rng.random::<u64>());
                        let b_host = make_random_host_matrix(sz, rng.random::<u64>());
                        LtOpData {
                            a: a_host.data.iter().map(|v| f16::from_f32(*v)).collect(),
                            b: b_host.data.iter().map(|v| f16::from_f32(*v)).collect(),
                            scale_a: Vec::new(),
                            scale_b: Vec::new(),
                        }
                    },
                )
                }
            }
            PrecisionKind::FP32 | PrecisionKind::TF32 => {
                if self.gemm_legacy_cublas {
                let cfg = GemmConfig {
                    transa: op_a,
                    transb: op_b,
                    m: size as i32,
                    n: size as i32,
                    k: size as i32,
                    alpha: 1.0f32,
                    lda: size as i32,
                    ldb: size as i32,
                    beta: 0.0f32,
                    ldc: size as i32,
                };
                let mut a_devs = Vec::with_capacity(lane_count);
                let mut b_devs = Vec::with_capacity(lane_count);
                for lane in 0..lane_count {
                    let stream = self.stream_for_lane(lane);
                    let a_host = make_random_host_matrix(size, rng.random::<u64>());
                    let b_host = make_random_host_matrix(size, rng.random::<u64>());
                    a_devs.push(
                        stream
                            .clone_htod(&a_host.data)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                    b_devs.push(
                        stream
                            .clone_htod(&b_host.data)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                }
                // Reuse output buffers across iterations (beta = 0 overwrites C).
                let mut c_devs = Vec::with_capacity(lane_count);
                for lane in 0..lane_count {
                    c_devs.push(
                        self.stream_for_lane(lane)
                            .alloc_zeros::<f32>(size * size)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                }
                for _ in 0..warmup_iters {
                    for lane in 0..lane_count {
                        let blas = self.blas_for_lane(lane);
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c_devs[lane])
                                .map_err(|err| BackendError::Other(err.to_string()))?;
                        }
                    }
                }
                for lane in 0..lane_count {
                    self.stream_for_lane(lane)
                        .synchronize()
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                let op_start = Instant::now();
                for _ in 0..burst_iters {
                    for lane in 0..lane_count {
                        let blas = self.blas_for_lane(lane);
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c_devs[lane])
                                .map_err(|err| BackendError::Other(err.to_string()))?;
                        }
                    }
                }
                for lane in 0..lane_count {
                    self.stream_for_lane(lane)
                        .synchronize()
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                self.run_gemm_verify(
                    spec,
                    &a_devs,
                    &b_devs,
                    &c_devs,
                    size,
                    transpose_a,
                    transpose_b,
                    0,
                    0,
                    None,
                    None,
                    seed,
                    verify,
                )?;
                Ok(op_start.elapsed().as_secs_f64())
                } else {
                // Default cublasLt lane (see the FP16 arm). TF32 rides the
                // desc-level FAST_TF32 compute type; FP32 uses plain 32F.
                let dtype = if spec.kind == PrecisionKind::TF32 {
                    LtDtype::Tf32
                } else {
                    LtDtype::F32
                };
                self.run_gemm_path_lt::<f32, f32, _>(
                    dtype,
                    spec,
                    size,
                    transpose_a,
                    transpose_b,
                    false,
                    warmup_iters,
                    burst_iters,
                    stream_mode,
                    seed,
                    &mut rng,
                    verify,
                    true,
                    0,
                    0,
                    None,
                    |rng, sz| {
                        let a_host = make_random_host_matrix(sz, rng.random::<u64>());
                        let b_host = make_random_host_matrix(sz, rng.random::<u64>());
                        LtOpData {
                            a: a_host.data,
                            b: b_host.data,
                            scale_a: Vec::new(),
                            scale_b: Vec::new(),
                        }
                    },
                )
                }
            }
            PrecisionKind::FP64 => {
                if self.gemm_legacy_cublas {
                let cfg = GemmConfig {
                    transa: op_a,
                    transb: op_b,
                    m: size as i32,
                    n: size as i32,
                    k: size as i32,
                    alpha: 1.0f64,
                    lda: size as i32,
                    ldb: size as i32,
                    beta: 0.0f64,
                    ldc: size as i32,
                };
                let mut a_devs = Vec::with_capacity(lane_count);
                let mut b_devs = Vec::with_capacity(lane_count);
                for lane in 0..lane_count {
                    let stream = self.stream_for_lane(lane);
                    let a_host = make_random_host_matrix(size, rng.random::<u64>());
                    let b_host = make_random_host_matrix(size, rng.random::<u64>());
                    let a: Vec<f64> = a_host.data.iter().map(|v| *v as f64).collect();
                    let b: Vec<f64> = b_host.data.iter().map(|v| *v as f64).collect();
                    a_devs.push(
                        stream
                            .clone_htod(&a)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                    b_devs.push(
                        stream
                            .clone_htod(&b)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                }
                // Reuse output buffers across iterations (beta = 0 overwrites C).
                let mut c_devs = Vec::with_capacity(lane_count);
                for lane in 0..lane_count {
                    c_devs.push(
                        self.stream_for_lane(lane)
                            .alloc_zeros::<f64>(size * size)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                }
                for _ in 0..warmup_iters {
                    for lane in 0..lane_count {
                        let blas = self.blas_for_lane(lane);
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c_devs[lane])
                                .map_err(|err| BackendError::Other(err.to_string()))?;
                        }
                    }
                }
                for lane in 0..lane_count {
                    self.stream_for_lane(lane)
                        .synchronize()
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                let op_start = Instant::now();
                for _ in 0..burst_iters {
                    for lane in 0..lane_count {
                        let blas = self.blas_for_lane(lane);
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c_devs[lane])
                                .map_err(|err| BackendError::Other(err.to_string()))?;
                        }
                    }
                }
                for lane in 0..lane_count {
                    self.stream_for_lane(lane)
                        .synchronize()
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                self.run_gemm_verify(
                    spec,
                    &a_devs,
                    &b_devs,
                    &c_devs,
                    size,
                    transpose_a,
                    transpose_b,
                    1,
                    1,
                    None,
                    None,
                    seed,
                    verify,
                )?;
                Ok(op_start.elapsed().as_secs_f64())
                } else {
                // Default cublasLt lane (see the FP16 arm).
                self.run_gemm_path_lt::<f64, f64, _>(
                    LtDtype::F64,
                    spec,
                    size,
                    transpose_a,
                    transpose_b,
                    false,
                    warmup_iters,
                    burst_iters,
                    stream_mode,
                    seed,
                    &mut rng,
                    verify,
                    true,
                    1,
                    1,
                    None,
                    |rng, sz| {
                        let a_host = make_random_host_matrix(sz, rng.random::<u64>());
                        let b_host = make_random_host_matrix(sz, rng.random::<u64>());
                        LtOpData {
                            a: a_host.data.iter().map(|v| *v as f64).collect(),
                            b: b_host.data.iter().map(|v| *v as f64).collect(),
                            scale_a: Vec::new(),
                            scale_b: Vec::new(),
                        }
                    },
                )
                }
            }
            PrecisionKind::INT8 => {
                if self.gemm_legacy_cublas {
                // cuBLAS typed INT8 GEMM: C(i32) = A(i8) * B(i8). With
                // computeType = CUBLAS_COMPUTE_32I and algo = DEFAULT_TENSOR_OP,
                // cuBLAS auto-dispatches to the IMMA INT8 tensor cores on
                // Turing+ and a DP4A / scalar INT8 path on older GPUs.
                //
                // The INT8 tensor-op path requires every dimension to be a
                // multiple of 16; odd/unaligned sizes (the default pool contains
                // 2049, 4097, 8193, …) make cuBLAS return
                // CUBLAS_STATUS_NOT_SUPPORTED. Round the working size up to the
                // next multiple of 16 — the data is random and we only measure
                // throughput, so the alignment padding does not affect the
                // stress intent.
                let size = (size + 15) & !15;
                let m = size as i32;
                let n = size as i32;
                let k = size as i32;
                let alpha: i32 = 1;
                let beta: i32 = 0;
                let a_type = cublas_sys::cudaDataType::CUDA_R_8I;
                let b_type = cublas_sys::cudaDataType::CUDA_R_8I;
                let c_type = cublas_sys::cudaDataType::CUDA_R_32I;
                let compute = cublas_sys::cublasComputeType_t::CUBLAS_COMPUTE_32I;
                let algo = cublas_sys::cublasGemmAlgo_t::CUBLAS_GEMM_DEFAULT_TENSOR_OP;

                // Quantize the random FP host matrices into signed int8 in
                // [-127, 127]; values are kept small so the per-element INT8
                // dot products never overflow the INT32 accumulator.
                let mut a_devs = Vec::with_capacity(lane_count);
                let mut b_devs = Vec::with_capacity(lane_count);
                for lane in 0..lane_count {
                    let stream = self.stream_for_lane(lane);
                    let a_host = make_random_host_matrix(size, rng.random::<u64>());
                    let b_host = make_random_host_matrix(size, rng.random::<u64>());
                    let a: Vec<i8> = a_host
                        .data
                        .iter()
                        .map(|v| (*v * 127.0).clamp(-127.0, 127.0) as i8)
                        .collect();
                    let b: Vec<i8> = b_host
                        .data
                        .iter()
                        .map(|v| (*v * 127.0).clamp(-127.0, 127.0) as i8)
                        .collect();
                    a_devs.push(
                        stream
                            .clone_htod(&a)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                    b_devs.push(
                        stream
                            .clone_htod(&b)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                }
                // Reuse output buffers across iterations (beta = 0 overwrites C).
                let mut c_devs = Vec::with_capacity(lane_count);
                for lane in 0..lane_count {
                    c_devs.push(
                        self.stream_for_lane(lane)
                            .alloc_zeros::<i32>(size * size)
                            .map_err(|err| BackendError::Other(err.to_string()))?,
                    );
                }
                for _ in 0..warmup_iters {
                    for lane in 0..lane_count {
                        Self::launch_int8_gemm(
                            self.blas_for_lane(lane),
                            self.stream_for_lane(lane),
                            op_a,
                            op_b,
                            m,
                            n,
                            k,
                            &alpha,
                            &a_devs[lane],
                            m,
                            &b_devs[lane],
                            k,
                            &beta,
                            &mut c_devs[lane],
                            m,
                            compute,
                            algo,
                            a_type,
                            b_type,
                            c_type,
                        )?;
                    }
                }
                for lane in 0..lane_count {
                    self.stream_for_lane(lane)
                        .synchronize()
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                let op_start = Instant::now();
                for _ in 0..burst_iters {
                    for lane in 0..lane_count {
                        Self::launch_int8_gemm(
                            self.blas_for_lane(lane),
                            self.stream_for_lane(lane),
                            op_a,
                            op_b,
                            m,
                            n,
                            k,
                            &alpha,
                            &a_devs[lane],
                            m,
                            &b_devs[lane],
                            k,
                            &beta,
                            &mut c_devs[lane],
                            m,
                            compute,
                            algo,
                            a_type,
                            b_type,
                            c_type,
                        )?;
                    }
                }
                for lane in 0..lane_count {
                    self.stream_for_lane(lane)
                        .synchronize()
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                Ok(op_start.elapsed().as_secs_f64())
                } else {
                // Default cublasLt lane: the IMMA tensor-op path is TN-only,
                // so the transpose flags are forced to T/N. The classic path's
                // randomized transposes (transpose_prob) mostly fall off the
                // tensor-core kernels onto the slower DP4A/scalar paths — the
                // [GEMM-LT] algo report makes the selection visible, and
                // --gemm-algo-list times the candidates.
                self.run_gemm_path_lt::<i8, i32, _>(
                    LtDtype::Int8,
                    spec,
                    (size + 15) & !15,
                    true,
                    false,
                    false,
                    warmup_iters,
                    burst_iters,
                    stream_mode,
                    seed,
                    &mut rng,
                    verify,
                    false, // No ride-on-load sampled check: i32 dot products
                           // exceed 2^24 and the f32 compare would be
                           // inexact; the periodic exact sidecar covers INT8.
                    8,     // C output: i32 (stats scan only, unused here)
                    0,     // A/B operand code unused (no verify)
                    None,
                    |rng, sz| {
                        let a_host = make_random_host_matrix(sz, rng.random::<u64>());
                        let b_host = make_random_host_matrix(sz, rng.random::<u64>());
                        LtOpData {
                            a: a_host
                                .data
                                .iter()
                                .map(|v| (*v * 127.0).clamp(-127.0, 127.0) as i8)
                                .collect(),
                            b: b_host
                                .data
                                .iter()
                                .map(|v| (*v * 127.0).clamp(-127.0, 127.0) as i8)
                                .collect(),
                            scale_a: Vec::new(),
                            scale_b: Vec::new(),
                        }
                    },
                )
                }
            }
            // cublasLt FP8 (E4M3FN / E5M2): TN layout is mandatory and the
            // leading dimensions must be 16-byte multiples (mirrors the INT8
            // size rounding). FAST_ACCUM is opt-in (--fp8-fast-accum): the
            // default segmented-accumulation path is precision-safe at any K
            // (unlocked-ada-gemm measured 10-15% relative error at large K on
            // the fast path), which matters for the ride-on-load compare.
            #[cfg(not(feature = "cuda11"))]
            PrecisionKind::FP8E4M3FN | PrecisionKind::FP8E5M2 => {
                let size = (size + 15) & !15;
                let e5m2 = spec.kind == PrecisionKind::FP8E5M2;

                let (dtype, ab_code) = if e5m2 {
                    (LtDtype::Fp8E5m2, 9u32)
                } else {
                    (LtDtype::Fp8E4m3, 4u32)
                };
                let lt_fill = |kernels: &super::gpu_fill::GpuFillKernels,
                               stream: &std::sync::Arc<cudarc::driver::CudaStream>,
                               fill_seed: u64,
                               sz: usize|
                 -> Result<LtDeviceOps<u8>, BackendError> {
                    let n = sz * sz;
                        let a = stream
                            .alloc_zeros::<u8>(n)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        let b = stream
                            .alloc_zeros::<u8>(n)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        let e5 = u32::from(e5m2);
                        let cfg = LaunchConfig::for_num_elems(n.min(u32::MAX as usize) as u32);
                        for (buf, fill) in [(&a, fill_seed), (&b, fill_seed ^ 0xA5A5_5A5A_5A5A_5A5A)] {
                            unsafe {
                                stream
                                    .launch_builder(&kernels.fp8_fn)
                                    .arg(buf)
                                    .arg(&(n as u64))
                                    .arg(&fill)
                                    .arg(&e5)
                                    .launch(cfg)
                                    .map_err(|err| BackendError::Other(err.to_string()))?;
                            }
                        }
                    Ok(LtDeviceOps {
                        a,
                        b,
                        scale_a: None,
                        scale_b: None,
                    })
                };
                let device_gen: Option<&LtDeviceGen<'_, u8>> =
                    if self.gpu_generate_enabled()
                {
                    Some(&lt_fill)
                } else {
                    None
                };
                let quant: fn(f32) -> u8 = if e5m2 {
                    fp8_e5m2_quantize
                } else {
                    fp8_e4m3_quantize
                };
                self.run_gemm_path_lt::<u8, f16, _>(
                    dtype,
                    spec,
                    size,
                    true,
                    false,
                    self.fp8_fast_accum,
                    warmup_iters,
                    burst_iters,
                    stream_mode,
                    seed,
                    &mut rng,
                    verify,
                    true,
                    2, // C output: f16
                    ab_code,
                    device_gen,
                    |rng, sz| {
                        let a_host = make_random_host_matrix(sz, rng.random::<u64>());
                        let b_host = make_random_host_matrix(sz, rng.random::<u64>());
                        // Parallel quantize: 8192² × 2 = 134M elements would
                        // otherwise stall the burst pipeline between ops.
                        LtOpData {
                            a: a_host
                                .data
                                .par_iter()
                                .map(|v| quant(*v))
                                .collect(),
                            b: b_host
                                .data
                                .par_iter()
                                .map(|v| quant(*v))
                                .collect(),
                            scale_a: Vec::new(),
                            scale_b: Vec::new(),
                        }
                    },
                )
            }
            #[cfg(feature = "cuda11")]
            PrecisionKind::FP8E4M3FN | PrecisionKind::FP8E5M2 => Err(BackendError::Other(
                "FP8 GEMM requires the cuda12 build (CUDA 11.8+ header surface); rebuild without --features cuda11"
                    .to_string(),
            )),
            // cublasLt FP4 (E2M1 + block scales): Blackwell FP4 tensor cores
            // (SM100/SM103/SM120) on a CUDA 12.8+/R570+ cublasLt. TN is
            // mandatory; sizes round to a 32-element multiple (scale block +
            // byte packing + ld alignment). NVFP4 (VEC16_E4M3 scales) is
            // preferred; its absence in the heuristics falls back to MXFP4
            // (VEC32_UE8M0).
            #[cfg(not(feature = "cuda11"))]
            PrecisionKind::FP4E2M1 => {
                let size = (size + 31) & !31;
                let probe_key = LtShapeKey {
                    dtype: LtDtype::Fp4Nvfp4,
                    size: size as u32,
                    ta: true,
                    tb: false,
                    fast_accum: false,
                };
                let nv_count = self.lt_for_lane_mut(0).heuristic_count(probe_key)?;
                let (dtype, mode, ab_code) = if nv_count > 0 {
                    (LtDtype::Fp4Nvfp4, Fp4ScaleMode::Nvfp4, 5u32)
                } else {
                    println!(
                        "[GEMM-LT] FP4 NVFP4 scale mode unavailable (driver cublasLt < 12.8?); falling back to MXFP4 (UE8M0 block scales)"
                    );
                    (LtDtype::Fp4Mxfp4, Fp4ScaleMode::Mxfp4, 6u32)
                };
                let lt_fill = |kernels: &super::gpu_fill::GpuFillKernels,
                               stream: &std::sync::Arc<cudarc::driver::CudaStream>,
                               fill_seed: u64,
                               sz: usize|
                 -> Result<LtDeviceOps<u8>, BackendError> {
                    let block_len = match mode {
                            Fp4ScaleMode::Mxfp4 => 32usize,
                            Fp4ScaleMode::Nvfp4 => 16usize,
                        };
                        let kblocks = sz / block_len;
                        let a = stream
                            .alloc_zeros::<u8>(sz * sz / 2)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        let b = stream
                            .alloc_zeros::<u8>(sz * sz / 2)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        let sa = stream
                            .alloc_zeros::<u8>(kblocks * sz)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        let sb = stream
                            .alloc_zeros::<u8>(kblocks * sz)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        let mxfp4 = u32::from(mode == Fp4ScaleMode::Mxfp4);
                        let cfg = LaunchConfig::for_num_elems(
                            (sz * kblocks).min(u32::MAX as usize) as u32,
                        );
                        for (buf, scales, fill) in [
                            (&a, &sa, fill_seed),
                            (&b, &sb, fill_seed ^ 0xA5A5_5A5A_5A5A_5A5A),
                        ] {
                            unsafe {
                                stream
                                    .launch_builder(&kernels.fp4_fn)
                                    .arg(buf)
                                    .arg(scales)
                                    .arg(&(sz as u64))
                                    .arg(&(sz as u64))
                                    .arg(&(kblocks as u64))
                                    .arg(&(block_len as u32))
                                    .arg(&mxfp4)
                                    .arg(&fill)
                                    .launch(cfg)
                                    .map_err(|err| BackendError::Other(err.to_string()))?;
                            }
                        }
                    Ok(LtDeviceOps {
                        a,
                        b,
                        scale_a: Some(sa),
                        scale_b: Some(sb),
                    })
                };
                let device_gen: Option<&LtDeviceGen<'_, u8>> =
                    if self.gpu_generate_enabled()
                {
                    Some(&lt_fill)
                } else {
                    None
                };
                self.run_gemm_path_lt::<u8, f16, _>(
                    dtype,
                    spec,
                    size,
                    true,
                    false,
                    false,
                    warmup_iters,
                    burst_iters,
                    stream_mode,
                    seed,
                    &mut rng,
                    verify,
                    true,
                    2, // C output: f16
                    ab_code,
                    device_gen,
                    move |rng, sz| {
                        let a_host = make_random_host_matrix(sz, rng.random::<u64>());
                        let b_host = make_random_host_matrix(sz, rng.random::<u64>());
                        let (a_packed, a_scales) =
                            fp4_quantize_matrix(&a_host.data, sz, mode);
                        let (b_packed, b_scales) =
                            fp4_quantize_matrix(&b_host.data, sz, mode);
                        LtOpData {
                            a: a_packed,
                            b: b_packed,
                            scale_a: a_scales,
                            scale_b: b_scales,
                        }
                    },
                )
            }
            #[cfg(feature = "cuda11")]
            PrecisionKind::FP4E2M1 => Err(BackendError::Other(
                "FP4 GEMM requires the cuda12 build (CUDA 12.8+ header surface)".to_string(),
            )),
            // cublasLt INT4 (CUDA_R_4I): IMMA 4-bit integer tensor ops, TN
            // mandatory, sizes round to a 32-element multiple (2 nibbles per
            // byte + ld alignment). The sampled cross-check is exact (products
            // ≤ 64·K < 2^24) with atol/rtol = 0, plus the periodic exact
            // integer sidecar.
            PrecisionKind::INT4 => {
                let size = (size + 31) & !31;
                self.run_gemm_path_lt::<u8, i32, _>(
                    LtDtype::Int4,
                    spec,
                    size,
                    true,
                    false,
                    false,
                    warmup_iters,
                    burst_iters,
                    stream_mode,
                    seed,
                    &mut rng,
                    verify,
                    true,
                    8, // C output: i32
                    7, // A/B operand: packed signed nibbles
                    None,
                    |rng, sz| {
                        let a_host = make_random_host_matrix(sz, rng.random::<u64>());
                        let b_host = make_random_host_matrix(sz, rng.random::<u64>());
                        let a_q: Vec<i8> = a_host
                            .data
                            .iter()
                            .map(|v| ((*v * 7.5).round() as i8).clamp(-8, 7))
                            .collect();
                        let b_q: Vec<i8> = b_host
                            .data
                            .iter()
                            .map(|v| ((*v * 7.5).round() as i8).clamp(-8, 7))
                            .collect();
                        LtOpData {
                            a: int4_pack(&a_q),
                            b: int4_pack(&b_q),
                            scale_a: Vec::new(),
                            scale_b: Vec::new(),
                        }
                    },
                )
            }
            // cuBLAS exposes no integer-GEMM compute path for INT16/INT32
            // (CUDA_R_16I / CUDA_R_32I are storage types only). Integer ALU
            // stress for those widths is provided by the `intalu` kernel type.
            PrecisionKind::INT16 | PrecisionKind::INT32 => Err(BackendError::Other(
                "GEMM unsupported for INT16/INT32; use --kernel-types intalu".to_string(),
            )),
        }
    }

    /// One INT8 cuBLAS GEMM launch on `blas`/`stream` for the typed
    /// `C(i32) = alpha * A(i8) * B(i8) + beta * C(i32)` operation.
    #[allow(clippy::too_many_arguments)]
    fn launch_int8_gemm(
        blas: &cudarc::cublas::CudaBlas,
        stream: &std::sync::Arc<cudarc::driver::CudaStream>,
        op_a: cublas_sys::cublasOperation_t,
        op_b: cublas_sys::cublasOperation_t,
        m: i32,
        n: i32,
        k: i32,
        alpha: &i32,
        a: &cudarc::driver::CudaSlice<i8>,
        lda: i32,
        b: &cudarc::driver::CudaSlice<i8>,
        ldb: i32,
        beta: &i32,
        c: &mut cudarc::driver::CudaSlice<i32>,
        ldc: i32,
        compute: cublas_sys::cublasComputeType_t,
        algo: cublas_sys::cublasGemmAlgo_t,
        a_type: cublas_sys::cudaDataType,
        b_type: cublas_sys::cudaDataType,
        c_type: cublas_sys::cudaDataType,
    ) -> Result<(), BackendError> {
        let (a_ptr, _a_sync) = a.device_ptr(stream);
        let (b_ptr, _b_sync) = b.device_ptr(stream);
        let (c_ptr, _c_sync) = c.device_ptr_mut(stream);
        unsafe {
            cublas_res::gemm_ex(
                *blas.handle(),
                op_a,
                op_b,
                m,
                n,
                k,
                alpha as *const i32 as *const _,
                a_ptr as *const _,
                a_type,
                lda,
                b_ptr as *const _,
                b_type,
                ldb,
                beta as *const i32 as *const _,
                c_ptr as *mut _,
                c_type,
                ldc,
                compute,
                algo,
            )
            .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        Ok(())
    }

    /// Exact INT8/INT4 GEMM validation: quantized host reference vs device C
    /// with strict integer equality. Runs the same cublasLt TN path as the
    /// stress op (a small matrix at the validate cadence keeps the host
    /// reference cost in the tens of milliseconds).
    pub(super) fn validate_int_exact_impl(
        &mut self,
        kind: PrecisionKind,
        size: usize,
        seed: u64,
    ) -> Result<(bool, Option<String>), BackendError> {
        let (dtype, round_mask, quant_scale, quant_min, quant_max) = match kind {
            PrecisionKind::INT8 => (LtDtype::Int8, 15usize, 127.0f64, -127i64, 127i64),
            PrecisionKind::INT4 => (LtDtype::Int4, 31usize, 7.5f64, -8i64, 7i64),
            _ => {
                return Err(BackendError::Other(
                    "exact INT validation only covers INT8/INT4".to_string(),
                ));
            }
        };
        let size = (size + round_mask) & !round_mask;
        let mut rng = StdRng::seed_from_u64(seed);
        let a_host_f = make_random_host_matrix(size, rng.random::<u64>());
        let b_host_f = make_random_host_matrix(size, rng.random::<u64>());
        let quant =
            |v: f32| ((v as f64 * quant_scale).round() as i64).clamp(quant_min, quant_max) as i8;
        let a_q: Vec<i8> = a_host_f.data.iter().map(|v| quant(*v)).collect();
        let b_q: Vec<i8> = b_host_f.data.iter().map(|v| quant(*v)).collect();

        // cublasLt TN launch on the same lane as the stress op (lane 0).
        let stream = self.stream.clone();
        let key = LtShapeKey {
            dtype,
            size: size as u32,
            ta: true,
            tb: false,
            fast_accum: false,
        };
        let pinned = self.lt_options().algo_index.unwrap_or(0);
        let host_c: Vec<i32> = match dtype {
            LtDtype::Int8 => {
                let a_dev = self
                    .stream
                    .clone_htod(&a_q)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                let b_dev = self
                    .stream
                    .clone_htod(&b_q)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                let mut c_dev = self
                    .stream
                    .alloc_zeros::<i32>(size * size)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                self.launch_lt_tn(&stream, key, pinned, &a_dev, &b_dev, &mut c_dev)?;
                self.stream
                    .synchronize()
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                let host = self
                    .stream
                    .clone_dtoh(&c_dev)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                self.stream
                    .synchronize()
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                host
            }
            LtDtype::Int4 => {
                let a_packed = int4_pack(&a_q);
                let b_packed = int4_pack(&b_q);
                let a_dev = self
                    .stream
                    .clone_htod(&a_packed)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                let b_dev = self
                    .stream
                    .clone_htod(&b_packed)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                let mut c_dev = self
                    .stream
                    .alloc_zeros::<i32>(size * size)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                self.launch_lt_tn(&stream, key, pinned, &a_dev, &b_dev, &mut c_dev)?;
                self.stream
                    .synchronize()
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                let host = self
                    .stream
                    .clone_dtoh(&c_dev)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                self.stream
                    .synchronize()
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                host
            }
            _ => unreachable!("guarded by the kind match above"),
        };

        // Exact integer reference for the TN identity (rayon-parallel).
        let reference = match dtype {
            LtDtype::Int8 => cpu_reference_int_tn(&a_q, &b_q, size),
            LtDtype::Int4 => {
                let a_packed = int4_pack(&a_q);
                let b_packed = int4_pack(&b_q);
                cpu_reference_int4_tn(&a_packed, &b_packed, size)
            }
            _ => unreachable!("guarded by the kind match above"),
        };
        for (idx, (actual, expected)) in host_c.iter().zip(reference.iter()).enumerate() {
            if actual != expected {
                let (i, j) = (idx / size, idx % size);
                return Ok((
                    false,
                    Some(format!(
                        "{} exact mismatch at [{i},{j}]: device={actual} reference={expected}",
                        dtype.name()
                    )),
                ));
            }
        }
        Ok((true, None))
    }

    /// One cublasLt TN INT GEMM launch (alpha=1, beta=0, i32 C) on lane 0.
    fn launch_lt_tn<AB: DeviceRepr + ValidAsZeroBits>(
        &mut self,
        stream: &std::sync::Arc<cudarc::driver::CudaStream>,
        key: LtShapeKey,
        algo_index: usize,
        a: &cudarc::driver::CudaSlice<AB>,
        b: &cudarc::driver::CudaSlice<AB>,
        c: &mut cudarc::driver::CudaSlice<i32>,
    ) -> Result<(), BackendError> {
        let (a_ptr, _ga) = a.device_ptr(stream);
        let (b_ptr, _gb) = b.device_ptr(stream);
        let (c_ptr, _gc) = c.device_ptr_mut(stream);
        let alpha: i32 = 1;
        let beta: i32 = 0;
        self.lt_for_lane_mut(0).launch(
            stream,
            key,
            algo_index,
            a_ptr as *const _,
            b_ptr as *const _,
            c_ptr as *mut _,
            None,
            None,
            (&alpha as *const i32) as *const _,
            (&beta as *const i32) as *const _,
        )
    }
}
