//! Integer ALU stress path — a custom NVRTC kernel that drives the integer
//! multiply-accumulate (and, on Pascal+, INT8 DP4A dot) units.
//!
//! Unlike the FP precisions, INT16/INT32 have no cuBLAS GEMM compute path, so
//! this kernel is the integer-stress mechanism for those widths. INT8 is also
//! usable here as a pure-ALU fallback (the faster INT8 path is the cuBLAS GEMM
//! in [`super::gemm`]).
//!
//! ## Generation compatibility
//! The kernel source is single, guarded by `__CUDA_ARCH__`: the DP4A
//! `__dp4a` intrinsic (INT8 4-way dot → INT32) is emitted only on SM ≥ 6.1
//! (Pascal GP100/GP10x); older/odd targets take the scalar int32 MAD path. The
//! PTX is compiled for a virtual `compute_<maj><min>` arch (capped at
//! `compute_90`) and the driver JIT-compiles it to the real GPU's SASS at module
//! load, so one build covers Pascal → Blackwell.

use std::sync::Arc;
use std::time::Instant;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use cudarc::driver::{CudaContext, CudaFunction, CudaModule, LaunchConfig, PushKernelArg};
use cudarc::nvrtc::{CompileOptions, compile_ptx_with_opts};

use cli_stressor_cuda_rs::{BackendError, DeviceInfo, PrecisionKind, PrecisionSpec, StreamMode};

use super::backend::CudaBackend;
use super::kernels::load_kernel;

const INTALU_SRC: &str = r#"
// Heavy, data-dependent integer MAD chain plus an INT8 DP4A dot on capable HW.
// `mode` selects the nominal operand width (8 / 16 / 32); the 8/16 modes add
// extra narrowing (sign-extend / xor) work to stress the smaller ALU paths.
extern "C" __global__ void int_alu_stress(
    const int* __restrict__ in, unsigned int n, unsigned int mode, int* __restrict__ out)
{
    unsigned int idx = blockIdx.x * blockDim.x + threadIdx.x;
    if (idx >= n) return;
    int a = in[idx];
    int b = in[(idx ^ 1u) < n ? (idx ^ 1u) : 0u];
    int acc = a;
    #pragma unroll 8
    for (int i = 0; i < 32; ++i) {
        acc = acc * b + a;                  // int32 multiply-accumulate
        b = b * 1664525 + 1013904223;       // LCG keeps the chain data-dependent
    }
#if __CUDA_ARCH__ >= 610
    // INT8 4-way dot product -> int32. Scalar __dp4a packs four 8-bit lanes out
    // of each 32-bit word; exercising the integer dot units (Pascal+).
    acc ^= __dp4a(a, b, 0);
#endif
    if (mode == 16u) {
        acc = (int)(short)acc ^ (int)(short)(acc >> 16);
    } else if (mode == 8u) {
        acc = (int)(signed char)acc
            ^ (int)(signed char)(acc >> 8)
            ^ (int)(signed char)(acc >> 16)
            ^ (int)(signed char)(acc >> 24);
    }
    out[idx] = acc;
}
"#;

/// Pick the NVRTC virtual-arch target for the device's compute capability.
///
/// Capped at `compute_90`: the CUDA 12.9 NVRTC bundled with cudarc's
/// `cuda-12090` feature may not yet recognize `compute_100` (Blackwell).
/// Targeting `compute_90` still works on newer GPUs because the driver
/// JIT-compiles the PTX down to the real architecture, and the
/// `__CUDA_ARCH__ >= 610` guard in the kernel re-selects the DP4A path at JIT
/// time.
fn nvrtc_arch_for(info: &DeviceInfo) -> String {
    let (maj, min) = info.compute_capability.unwrap_or((7, 5));
    let (tmaj, tmin) = if maj >= 9 { (9, 0) } else { (maj, min) };
    format!("compute_{}{}", tmaj, tmin)
}

pub(super) fn build_intalu_kernel(
    ctx: &Arc<CudaContext>,
    info: &DeviceInfo,
) -> Result<(Arc<CudaModule>, CudaFunction), BackendError> {
    let arch = nvrtc_arch_for(info);
    // `CompileOptions::arch` requires a `&'static str`. The string lives for the
    // process lifetime; this builder runs once per backend, so leaking is fine.
    let arch_static: &'static str = Box::leak(arch.into_boxed_str());
    let opts = CompileOptions {
        arch: Some(arch_static),
        ..Default::default()
    };
    let ptx = compile_ptx_with_opts(INTALU_SRC, opts)
        .map_err(|err| BackendError::Other(err.to_string()))?;
    load_kernel(ctx, ptx, "int_alu_stress")
}

impl CudaBackend {
    pub(super) fn run_intalu_path(
        &self,
        spec: &PrecisionSpec,
        size: usize,
        warmup_iters: u32,
        burst_iters: u32,
        seed: u64,
        stream_mode: StreamMode,
    ) -> Result<f64, BackendError> {
        let func = self.intalu_fn.as_ref().ok_or_else(|| {
            BackendError::Other("INT ALU kernel unavailable".to_string())
        })?;
        let n = (size * size) as u32;
        let lane_count = Self::lane_count(stream_mode);
        let mut rng = StdRng::seed_from_u64(seed);
        let mode: u32 = match spec.kind {
            PrecisionKind::INT8 => 8,
            PrecisionKind::INT16 => 16,
            PrecisionKind::INT32 => 32,
            // Other precisions shouldn't reach the intalu path; default to full
            // 32-bit integer MAD stress.
            _ => 32,
        };

        let mut xs = Vec::with_capacity(lane_count);
        let mut outs = Vec::with_capacity(lane_count);
        for lane in 0..lane_count {
            let stream = self.stream_for_lane(lane);
            // Seed with i32 data regardless of mode; the kernel narrows
            // internally for the 8/16-bit stress variants.
            let host: Vec<i32> = (0..n).map(|_| rng.random::<u32>() as i32).collect();
            xs.push(
                stream
                    .clone_htod(&host)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
            outs.push(
                stream
                    .alloc_zeros::<i32>(n as usize)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
        }
        let cfg = LaunchConfig::for_num_elems(n.max(1));

        for _ in 0..warmup_iters {
            for lane in 0..lane_count {
                let stream = self.stream_for_lane(lane);
                unsafe {
                    stream
                        .launch_builder(func)
                        .arg(&xs[lane])
                        .arg(&n)
                        .arg(&mode)
                        .arg(&mut outs[lane])
                        .launch(cfg)
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
                let stream = self.stream_for_lane(lane);
                unsafe {
                    stream
                        .launch_builder(func)
                        .arg(&xs[lane])
                        .arg(&n)
                        .arg(&mode)
                        .arg(&mut outs[lane])
                        .launch(cfg)
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
            }
        }
        for lane in 0..lane_count {
            self.stream_for_lane(lane)
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        Ok(op_start.elapsed().as_secs_f64())
    }
}
