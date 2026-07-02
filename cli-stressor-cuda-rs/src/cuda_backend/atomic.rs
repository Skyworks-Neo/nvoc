//! Atomic-operation stress path (custom NVRTC kernel) and its builder.

use std::sync::Arc;
use std::time::Instant;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use cudarc::driver::{CudaContext, CudaFunction, CudaModule, LaunchConfig, PushKernelArg};
use cudarc::nvrtc::compile_ptx;

use cli_stressor_cuda_rs::{BackendError, StreamMode};

use super::backend::CudaBackend;
use super::kernels::load_kernel;

const ATOMIC_SRC: &str = r#"
extern "C" __global__ void atomic_accum(const float* x, unsigned int n, unsigned int* out) {
    unsigned int idx = (unsigned int)(blockIdx.x * blockDim.x + threadIdx.x);
    if (idx < n) {
        unsigned int v = (idx & 31U) == 0U ? ((__float_as_uint(x[idx]) & 1U) + 1U) : 1U;
        atomicAdd(out, v);
    }
}
"#;

pub(super) fn build_atomic_kernel(
    ctx: &Arc<CudaContext>,
) -> Result<(Arc<CudaModule>, CudaFunction), BackendError> {
    let ptx = compile_ptx(ATOMIC_SRC).map_err(|err| BackendError::Other(err.to_string()))?;
    load_kernel(ctx, ptx, "atomic_accum")
}

impl CudaBackend {
    pub(super) fn run_atomic_path(
        &self,
        size: usize,
        warmup_iters: u32,
        burst_iters: u32,
        seed: u64,
        stream_mode: StreamMode,
    ) -> Result<f64, BackendError> {
        let atomic_fn = self
            .atomic_fn
            .as_ref()
            .ok_or_else(|| BackendError::Other("atomic kernel unavailable".to_string()))?;
        let n = (size * size) as u32;
        let lane_count = Self::lane_count(stream_mode);
        let mut rng = StdRng::seed_from_u64(seed);
        let mut xs = Vec::with_capacity(lane_count);
        let mut outs = Vec::with_capacity(lane_count);
        for lane in 0..lane_count {
            let stream = self.stream_for_lane(lane);
            let host: Vec<f32> = (0..n).map(|_| rng.random::<f32>()).collect();
            xs.push(
                stream
                    .clone_htod(&host)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
            outs.push(
                stream
                    .alloc_zeros::<u32>(1)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
        }
        let cfg = LaunchConfig::for_num_elems(n.max(1));

        for _ in 0..warmup_iters {
            for lane in 0..lane_count {
                let stream = self.stream_for_lane(lane);
                unsafe {
                    stream
                        .launch_builder(atomic_fn)
                        .arg(&xs[lane])
                        .arg(&n)
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
                        .launch_builder(atomic_fn)
                        .arg(&xs[lane])
                        .arg(&n)
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
