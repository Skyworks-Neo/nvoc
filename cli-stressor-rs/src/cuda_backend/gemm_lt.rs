//! cublasLt GEMM lane: cached descriptors, heuristic enumeration, algo
//! reporting/pinning — shared by the FP16/BF16/FP8/FP4/INT8/INT4 stress
//! paths.
//!
//! Why a separate lane from the classic `CudaBlas` handles: classic
//! `cublasGemmEx` exposes no user-visible algo identity, while cublasLt's
//! `cublasLtMatmulAlgoGetHeuristic` returns an explicit ranked candidate list
//! whose per-algo configuration (tile/stages/split-K/…) can be read back via
//! `cublasLtMatmulAlgoConfigGetAttribute` and pinned. Unlock cards can read
//! low when the library's static heuristics pick suboptimal tiles for the
//! modified hardware (see the unlocked-ada-gemm study); reporting + listing
//! makes that visible and pinning lets the operator pick a kernel by hand.
//!
//! Numeric parity with the classic path: cudarc's f16/bf16 `Gemm` hardcodes
//! `CUBLAS_COMPUTE_32F` with f32 alpha/beta, which is exactly what the Lt
//! descriptors below use, so FP16/BF16 readings stay comparable.

use std::collections::HashMap;
use std::sync::Arc;

use cudarc::cublaslt::result as lt_res;
use cudarc::cublaslt::sys as lt_sys;
use cudarc::driver::{CudaSlice, CudaStream, DevicePtr};

use cli_stressor_rs::BackendError;

use super::backend::CudaBackend;

/// GEMM dtype routed through cublasLt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(dead_code)] // FP8/FP4 variants are constructed only on the cuda12 build
pub(super) enum LtDtype {
    F16,
    Bf16,
    /// E4M3FN (`CUDA_R_8F_E4M3`), `CUBLAS_COMPUTE_32F`; FAST_ACCUM is part of
    /// the plan key. TN-only (cublasLt FP8 requirement).
    Fp8E4m3,
    /// E2M1 with 16-element E4M3 block scales (NVFP4,
    /// `MATRIX_SCALE_VEC16_UE4M3`). TN-only.
    Fp4Nvfp4,
    /// E2M1 with 32-element UE8M0 block scales (MXFP4,
    /// `MATRIX_SCALE_VEC32_UE8M0`). TN-only.
    Fp4Mxfp4,
    /// `CUDA_R_8I` → `CUDA_R_32I`, `CUBLAS_COMPUTE_32I` (IMMA tensor cores;
    /// the tensor-op path requires TN).
    Int8,
    /// `CUDA_R_4I` (two signed nibbles per byte) → `CUDA_R_32I`, IMMA; TN-only.
    Int4,
}

impl LtDtype {
    /// (A/B data type, C/D data type, scale type, compute type).
    fn types(
        self,
    ) -> (
        lt_sys::cudaDataType_t,
        lt_sys::cudaDataType_t,
        lt_sys::cudaDataType_t,
        lt_sys::cublasComputeType_t,
    ) {
        use lt_sys::cublasComputeType_t as ct;
        use lt_sys::cudaDataType_t as dt;
        match self {
            LtDtype::F16 => (
                dt::CUDA_R_16F,
                dt::CUDA_R_16F,
                dt::CUDA_R_32F,
                ct::CUBLAS_COMPUTE_32F,
            ),
            LtDtype::Bf16 => (
                dt::CUDA_R_16BF,
                dt::CUDA_R_16BF,
                dt::CUDA_R_32F,
                ct::CUBLAS_COMPUTE_32F,
            ),
            // FP8/FP4 datatypes only exist in the CUDA 11.8+/12.8+ header
            // surfaces; the cuda11 (R470-era) build excludes those arms, and
            // the hardware gate lives in `DeviceInfo::supports_precision`.
            #[cfg(not(feature = "cuda11"))]
            LtDtype::Fp8E4m3 => (
                dt::CUDA_R_8F_E4M3,
                dt::CUDA_R_16F,
                dt::CUDA_R_32F,
                ct::CUBLAS_COMPUTE_32F,
            ),
            #[cfg(not(feature = "cuda11"))]
            LtDtype::Fp4Nvfp4 | LtDtype::Fp4Mxfp4 => (
                dt::CUDA_R_4F_E2M1,
                dt::CUDA_R_16F,
                dt::CUDA_R_32F,
                ct::CUBLAS_COMPUTE_32F,
            ),
            #[cfg(feature = "cuda11")]
            LtDtype::Fp8E4m3 | LtDtype::Fp4Nvfp4 | LtDtype::Fp4Mxfp4 => {
                unreachable!("FP8/FP4 are cfg-excluded from the cuda11 build")
            }
            LtDtype::Int8 => (
                dt::CUDA_R_8I,
                dt::CUDA_R_32I,
                dt::CUDA_R_32I,
                ct::CUBLAS_COMPUTE_32I,
            ),
            LtDtype::Int4 => (
                dt::CUDA_R_4I,
                dt::CUDA_R_32I,
                dt::CUDA_R_32I,
                ct::CUBLAS_COMPUTE_32I,
            ),
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            LtDtype::F16 => "FP16",
            LtDtype::Bf16 => "BF16",
            LtDtype::Fp8E4m3 => "FP8 E4M3FN",
            LtDtype::Fp4Nvfp4 | LtDtype::Fp4Mxfp4 => "FP4 E2M1",
            LtDtype::Int8 => "INT8",
            LtDtype::Int4 => "INT4",
        }
    }
}

/// Plan key: everything that changes the descriptor or the heuristic output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct LtShapeKey {
    pub dtype: LtDtype,
    pub size: u32,
    pub ta: bool,
    pub tb: bool,
    pub fast_accum: bool,
}

/// Cached launch state for one shape (raw handles; owned by the lane's plan
/// map and destroyed in `Drop`).
pub(super) struct LtPlan {
    pub desc: lt_sys::cublasLtMatmulDesc_t,
    pub a_layout: lt_sys::cublasLtMatrixLayout_t,
    pub b_layout: lt_sys::cublasLtMatrixLayout_t,
    pub c_layout: lt_sys::cublasLtMatrixLayout_t,
    pub heuristics: Vec<lt_sys::cublasLtMatmulHeuristicResult_t>,
}

#[derive(Clone, Debug)]
pub(super) struct LtOptions {
    /// `--gemm-heuristics`: requested candidate count.
    pub heuristic_count: usize,
    /// `--gemm-algo-index`: pinned heuristic index (None = library top pick).
    pub algo_index: Option<usize>,
    /// `--gemm-algo-list`: time every candidate at first encounter.
    pub listing: bool,
}

impl Default for LtOptions {
    fn default() -> Self {
        Self {
            heuristic_count: 8,
            algo_index: None,
            listing: false,
        }
    }
}

pub(super) struct LtLane {
    handle: lt_sys::cublasLtHandle_t,
    pref: lt_sys::cublasLtMatmulPreference_t,
    workspace: CudaSlice<u8>,
    workspace_size: usize,
    options: LtOptions,
    plans: HashMap<LtShapeKey, LtPlan>,
}

impl LtLane {
    pub(super) fn new(
        stream: &Arc<CudaStream>,
        sm_major: i32,
        options: LtOptions,
    ) -> Result<Self, BackendError> {
        let handle = lt_res::create_handle().map_err(|err| BackendError::Other(err.to_string()))?;
        // Workspace sizing follows NVIDIA's cuBLASLt recommendation (and
        // cudarc's own CudaBlasLT): 32 MiB on Hopper+, 4 MiB elsewhere. It is
        // a heuristic input — larger workspaces unlock split-K candidates.
        let workspace_size: usize = if sm_major >= 9 { 32 << 20 } else { 4 << 20 };
        let workspace = stream
            .alloc_zeros::<u8>(workspace_size)
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let pref =
            lt_res::create_matmul_pref().map_err(|err| BackendError::Other(err.to_string()))?;
        unsafe {
            lt_res::set_matmul_pref_attribute(
                pref,
                lt_sys::cublasLtMatmulPreferenceAttributes_t::CUBLASLT_MATMUL_PREF_MAX_WORKSPACE_BYTES,
                (&workspace_size) as *const usize as *const _,
                std::mem::size_of::<usize>(),
            )
            .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        Ok(Self {
            handle,
            pref,
            workspace,
            workspace_size,
            options,
            plans: HashMap::new(),
        })
    }

    /// Fetch (or build) the plan for a shape; returns the candidate count. An
    /// empty list means the descriptor has no supported algo on this
    /// driver/device (e.g. FP4 on a pre-12.8 cublasLt) — callers fall back
    /// (NVFP4 → MXFP4) or fail with a clear message.
    pub(super) fn heuristic_count(&mut self, key: LtShapeKey) -> Result<usize, BackendError> {
        if !self.plans.contains_key(&key) {
            let plan = self.build_plan(key)?;
            self.plans.insert(key, plan);
        }
        Ok(self
            .plans
            .get(&key)
            .expect("plan was just built")
            .heuristics
            .len())
    }

    fn build_plan(&mut self, key: LtShapeKey) -> Result<LtPlan, BackendError> {
        let (ab_type, c_type, scale_type, compute) = key.dtype.types();
        // SAFETY: all descriptor/layout handles are owned by this plan and
        // destroyed in Drop; raw-pointer plumbing follows the cudarc
        // cublaslt::result wrappers.
        unsafe {
            let desc = lt_res::create_matmul_desc(compute, scale_type)
                .map_err(|err| BackendError::Other(err.to_string()))?;
            // cublasLt reuses the classic cublas op enum (bound in
            // cublas::sys, not cublaslt::sys).
            let op_t = cudarc::cublas::sys::cublasOperation_t::CUBLAS_OP_T;
            let op_n = cudarc::cublas::sys::cublasOperation_t::CUBLAS_OP_N;
            let ta_i: i32 = if key.ta { op_t } else { op_n } as i32;
            let tb_i: i32 = if key.tb { op_t } else { op_n } as i32;
            lt_res::set_matmul_desc_attribute(
                desc,
                lt_sys::cublasLtMatmulDescAttributes_t::CUBLASLT_MATMUL_DESC_TRANSA,
                (&ta_i) as *const i32 as *const _,
                std::mem::size_of::<i32>(),
            )
            .map_err(|err| BackendError::Other(err.to_string()))?;
            lt_res::set_matmul_desc_attribute(
                desc,
                lt_sys::cublasLtMatmulDescAttributes_t::CUBLASLT_MATMUL_DESC_TRANSB,
                (&tb_i) as *const i32 as *const _,
                std::mem::size_of::<i32>(),
            )
            .map_err(|err| BackendError::Other(err.to_string()))?;
            // FAST_ACCUM is an FP8-only attribute (CUDA 11.8+ header surface);
            // the cuda11 build never creates fast_accum plans.
            #[cfg(not(feature = "cuda11"))]
            if key.fast_accum {
                let one: i32 = 1;
                lt_res::set_matmul_desc_attribute(
                    desc,
                    lt_sys::cublasLtMatmulDescAttributes_t::CUBLASLT_MATMUL_DESC_FAST_ACCUM,
                    (&one) as *const i32 as *const _,
                    std::mem::size_of::<i32>(),
                )
                .map_err(|err| BackendError::Other(err.to_string()))?;
            }
            #[cfg(feature = "cuda11")]
            let _ = key.fast_accum;
            // Square stress shapes make every layout (size, size) with ld =
            // size regardless of the op flags; the row-major upload identity
            // is the same as the classic path (see the gemm_sample_check
            // comment in verify_kernels.rs).
            let rows = key.size as u64;
            let ld = key.size as i64;
            let a_layout = lt_res::create_matrix_layout(ab_type, rows, rows, ld)
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let b_layout = lt_res::create_matrix_layout(ab_type, rows, rows, ld)
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let c_layout = lt_res::create_matrix_layout(c_type, rows, rows, ld)
                .map_err(|err| BackendError::Other(err.to_string()))?;
            let mut heuristics = vec![
                lt_sys::cublasLtMatmulHeuristicResult_t {
                    algo: lt_sys::cublasLtMatmulAlgo_t { data: [0; 8] },
                    workspaceSize: 0,
                    state: lt_sys::cublasStatus_t::CUBLAS_STATUS_SUCCESS,
                    wavesCount: 0.0,
                    reserved: [0; 4],
                };
                self.options.heuristic_count
            ];
            let mut returned: i32 = 0;
            let status = lt_sys::cublasLtMatmulAlgoGetHeuristic(
                self.handle,
                desc,
                a_layout,
                b_layout,
                c_layout,
                c_layout,
                self.pref,
                heuristics.len() as i32,
                heuristics.as_mut_ptr(),
                &mut returned,
            );
            // CUBLAS_STATUS_NOT_SUPPORTED (no algo for this configuration) is
            // surfaced as an empty candidate list, not an error — callers
            // decide the fallback (FP4 NVFP4 → MXFP4) or the message.
            if status != lt_sys::cublasStatus_t::CUBLAS_STATUS_SUCCESS {
                return Ok(LtPlan {
                    desc,
                    a_layout,
                    b_layout,
                    c_layout,
                    heuristics: Vec::new(),
                });
            }
            heuristics.truncate(returned as usize);
            heuristics.retain(|r| r.state == lt_sys::cublasStatus_t::CUBLAS_STATUS_SUCCESS);
            Ok(LtPlan {
                desc,
                a_layout,
                b_layout,
                c_layout,
                heuristics,
            })
        }
    }

    /// One cublasLt matmul launch using candidate `algo_index` of the shape's
    /// heuristic list (the normal path pins `options.algo_index` or 0).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn launch(
        &mut self,
        stream: &Arc<CudaStream>,
        key: LtShapeKey,
        algo_index: usize,
        a: *const std::ffi::c_void,
        b: *const std::ffi::c_void,
        c: *mut std::ffi::c_void,
        scale_a: Option<*const std::ffi::c_void>,
        scale_b: Option<*const std::ffi::c_void>,
        alpha: *const std::ffi::c_void,
        beta: *const std::ffi::c_void,
    ) -> Result<(), BackendError> {
        if !self.plans.contains_key(&key) {
            let plan = self.build_plan(key)?;
            self.plans.insert(key, plan);
        }
        let plan = self.plans.get(&key).expect("plan was just built");
        if plan.heuristics.is_empty() {
            // Dtype-specific hint: FP4 needs a CUDA 12.8+ cublasLt; INT4's
            // CUDA_R_4I candidate presence simply depends on the driver
            // release (empirically absent on some current drivers).
            let hint = match key.dtype {
                LtDtype::Fp4Nvfp4 | LtDtype::Fp4Mxfp4 => " (FP4 needs CUDA 12.8+/R570+)",
                LtDtype::Int4 => {
                    " (this driver's cublasLt exposes no CUDA_R_4I/COMPUTE_32I candidate)"
                }
                _ => "",
            };
            return Err(BackendError::Unsupported(format!(
                "cublasLt returned no supported algo for {} size {}{}",
                key.dtype.name(),
                key.size,
                hint
            )));
        }
        if algo_index >= plan.heuristics.len() {
            return Err(BackendError::Other(format!(
                "pinned algo index {} out of range: {} returned {} candidate(s)",
                algo_index,
                key.dtype.name(),
                plan.heuristics.len()
            )));
        }
        let algo = plan.heuristics[algo_index].algo;
        let (desc, a_layout, b_layout, c_layout) =
            (plan.desc, plan.a_layout, plan.b_layout, plan.c_layout);
        // FP4 block scales are per-op device buffers: refresh the descriptor's
        // scale pointers before the launch. Safe within a lane (ops on one
        // lane are sequential and plans are per-lane). The scale-pointer
        // attributes need the CUDA 11.8+ header surface — the cuda11 build
        // never receives Some(_) (FP4 is cfg-excluded).
        #[cfg(not(feature = "cuda11"))]
        if let Some(sa) = scale_a {
            unsafe {
                lt_res::set_matmul_desc_attribute(
                    desc,
                    lt_sys::cublasLtMatmulDescAttributes_t::CUBLASLT_MATMUL_DESC_A_SCALE_POINTER,
                    (&sa) as *const _ as *const _,
                    std::mem::size_of::<*const std::ffi::c_void>(),
                )
                .map_err(|err| BackendError::Other(err.to_string()))?;
            }
        }
        #[cfg(not(feature = "cuda11"))]
        if let Some(sb) = scale_b {
            unsafe {
                lt_res::set_matmul_desc_attribute(
                    desc,
                    lt_sys::cublasLtMatmulDescAttributes_t::CUBLASLT_MATMUL_DESC_B_SCALE_POINTER,
                    (&sb) as *const _ as *const _,
                    std::mem::size_of::<*const std::ffi::c_void>(),
                )
                .map_err(|err| BackendError::Other(err.to_string()))?;
            }
        }
        #[cfg(feature = "cuda11")]
        let _ = (scale_a, scale_b);
        let (ws_ptr, _ws_sync) = self.workspace.device_ptr(stream);
        unsafe {
            lt_res::matmul(
                self.handle,
                desc,
                alpha,
                beta,
                a,
                a_layout,
                b,
                b_layout,
                c,
                c_layout,
                c,
                c_layout,
                &algo,
                ws_ptr as *mut _,
                self.workspace_size,
                stream.cu_stream() as *mut _,
            )
            .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        Ok(())
    }

    /// Workspace size in MiB (algo report display).
    pub(super) fn workspace_mib(&self) -> f64 {
        self.workspace_size as f64 / (1024.0 * 1024.0)
    }

    /// Human-readable identity of the `idx`-th heuristic candidate for a
    /// built plan (None when the plan/idx is missing).
    pub(super) fn algo_config_at(&self, key: LtShapeKey, idx: usize) -> Option<String> {
        let plan = self.plans.get(&key)?;
        let result = plan.heuristics.get(idx)?;
        Some(Self::algo_config_string(&result.algo))
    }

    /// Human-readable identity of one algo candidate (tile/stages/split-K/
    /// reduction/swizzle config attributes). cublasLt does not expose SASS
    /// kernel names through the public API — these attributes ARE the
    /// user-visible kernel identity.
    pub(super) fn algo_config_string(algo: &lt_sys::cublasLtMatmulAlgo_t) -> String {
        use lt_sys::cublasLtMatmulAlgoConfigAttributes_t as cfg;
        unsafe {
            let get = |attr: cfg| -> Option<u32> {
                let mut value: u32 = 0;
                let mut written: usize = 0;
                let status = lt_sys::cublasLtMatmulAlgoConfigGetAttribute(
                    algo,
                    attr,
                    (&mut value) as *mut u32 as *mut _,
                    std::mem::size_of::<u32>(),
                    &mut written,
                );
                if status == lt_sys::cublasStatus_t::CUBLAS_STATUS_SUCCESS && written > 0 {
                    Some(value)
                } else {
                    None
                }
            };
            format!(
                "id={} tile={} stages={} splitk={} red={} swz={}",
                get(cfg::CUBLASLT_ALGO_CONFIG_ID).unwrap_or(u32::MAX),
                get(cfg::CUBLASLT_ALGO_CONFIG_TILE_ID).unwrap_or(u32::MAX),
                get(cfg::CUBLASLT_ALGO_CONFIG_STAGES_ID).unwrap_or(u32::MAX),
                get(cfg::CUBLASLT_ALGO_CONFIG_SPLITK_NUM).unwrap_or(u32::MAX),
                get(cfg::CUBLASLT_ALGO_CONFIG_REDUCTION_SCHEME).unwrap_or(u32::MAX),
                get(cfg::CUBLASLT_ALGO_CONFIG_CTA_SWIZZLING).unwrap_or(u32::MAX),
            )
        }
    }

    fn options(&self) -> &LtOptions {
        &self.options
    }

    pub(super) fn set_options(&mut self, options: LtOptions) {
        self.options = options;
    }
}

impl Drop for LtLane {
    fn drop(&mut self) {
        unsafe {
            for plan in self.plans.values() {
                let _ = lt_res::destroy_matmul_desc(plan.desc);
                let _ = lt_res::destroy_matrix_layout(plan.a_layout);
                let _ = lt_res::destroy_matrix_layout(plan.b_layout);
                let _ = lt_res::destroy_matrix_layout(plan.c_layout);
            }
            let _ = lt_res::destroy_matmul_pref(self.pref);
            let _ = lt_res::destroy_handle(self.handle);
        }
    }
}

impl CudaBackend {
    pub(super) fn lt_for_lane(&self, lane: usize) -> &LtLane {
        match lane {
            0 => &self.lt,
            1 => &self.aux_lt[0],
            _ => &self.aux_lt[1],
        }
    }

    pub(super) fn lt_for_lane_mut(&mut self, lane: usize) -> &mut LtLane {
        match lane {
            0 => &mut self.lt,
            1 => &mut self.aux_lt[0],
            _ => &mut self.aux_lt[1],
        }
    }

    pub(super) fn lt_options(&self) -> &LtOptions {
        self.lt.options()
    }
}
