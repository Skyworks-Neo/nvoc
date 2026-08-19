//! Device-side stress-buffer generation (opt-in via `--gpu-generate`).
//!
//! Instead of generating random data on the host and copying it H2D, a tiny
//! NVRTC kernel fills the device buffer directly with the same SplitMix64
//! stream the host helpers produce (`lib.rs::splitmix64`). This removes the
//! host RNG + PCIe upload from the critical path between bursts, giving more
//! wall-clock to actual stress work. Content differs from the host path
//! (kernel-local index arithmetic), which is fine: no validation path reads
//! stress buffers.

use std::sync::Arc;

use cudarc::driver::{CudaContext, CudaFunction, CudaModule};

use cudarc::nvrtc::compile_ptx;

use cli_stressor_cuda_rs::BackendError;

use super::backend::CudaBackend;

const GPU_FILL_SRC: &str = r#"
extern "C" __device__ unsigned long long splitmix64(unsigned long long* state) {
    *state += 0x9E3779B97F4A7C15ULL;
    unsigned long long z = *state;
    z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9ULL;
    z = (z ^ (z >> 27)) * 0x94D049BB133111EBULL;
    return z ^ (z >> 31);
}

extern "C" __global__ void fill_random_bytes(unsigned char* buf, unsigned long long n, unsigned long long seed) {
    unsigned long long stride = (unsigned long long)gridDim.x * blockDim.x;
    for (unsigned long long idx = (unsigned long long)blockIdx.x * blockDim.x + threadIdx.x; idx < n; idx += stride) {
        unsigned long long state = seed + idx * 0x9E3779B97F4A7C15ULL;
        buf[idx] = (unsigned char)(splitmix64(&state) >> 32);
    }
}

extern "C" __global__ void fill_random_f32(float* buf, unsigned long long n, unsigned long long seed) {
    unsigned long long stride = (unsigned long long)gridDim.x * blockDim.x;
    for (unsigned long long idx = (unsigned long long)blockIdx.x * blockDim.x + threadIdx.x; idx < n; idx += stride) {
        unsigned long long state = seed + idx * 0x9E3779B97F4A7C15ULL;
        unsigned long long x = splitmix64(&state);
        buf[idx] = (float)((x >> 40) & 0xFFFFFFULL) * (1.0f / 16777216.0f);
    }
}

extern "C" __global__ void fill_random_i32(int* buf, unsigned long long n, unsigned long long seed) {
    unsigned long long stride = (unsigned long long)gridDim.x * blockDim.x;
    for (unsigned long long idx = (unsigned long long)blockIdx.x * blockDim.x + threadIdx.x; idx < n; idx += stride) {
        unsigned long long state = seed + idx * 0x9E3779B97F4A7C15ULL;
        buf[idx] = (int)(splitmix64(&state) >> 32);
    }
}
"#;

pub(super) struct GpuFillKernels {
    pub(super) _module: Arc<CudaModule>,
    pub(super) bytes_fn: CudaFunction,
    pub(super) f32_fn: CudaFunction,
    pub(super) i32_fn: CudaFunction,
}

pub(super) fn build_gpu_fill_kernels(ctx: &Arc<CudaContext>) -> Result<GpuFillKernels, BackendError> {
    let ptx = compile_ptx(GPU_FILL_SRC).map_err(|err| BackendError::Other(err.to_string()))?;
    let module = ctx
        .load_module(ptx)
        .map_err(|err| BackendError::Other(err.to_string()))?;
    let bytes_fn = module
        .load_function("fill_random_bytes")
        .map_err(|err| BackendError::Other(err.to_string()))?;
    let f32_fn = module
        .load_function("fill_random_f32")
        .map_err(|err| BackendError::Other(err.to_string()))?;
    let i32_fn = module
        .load_function("fill_random_i32")
        .map_err(|err| BackendError::Other(err.to_string()))?;
    Ok(GpuFillKernels {
        _module: module,
        bytes_fn,
        f32_fn,
        i32_fn,
    })
}

impl CudaBackend {
    /// Opt in to device-side stress-buffer generation. Builds the NVRTC fill
    /// kernels eagerly so per-burst paths stay allocation-free.
    pub fn enable_gpu_generate(&mut self) -> Result<(), BackendError> {
        if self.gpu_fill.is_some() {
            self.gpu_generate = true;
            return Ok(());
        }
        let kernels = build_gpu_fill_kernels(&self._ctx)?;
        self.gpu_fill = Some(kernels);
        self.gpu_generate = true;
        Ok(())
    }

    pub(super) fn gpu_generate_enabled(&self) -> bool {
        self.gpu_generate
    }
}
