//! Memory-bandwidth and element-wise stress paths (memcpy / memset / sgeam /
//! reduction), all driven through cuBLAS or the driver memory primitives.

use rand::rngs::StdRng;
use rand::{RngExt, SeedableRng};
use std::time::Instant;

use cudarc::cublas::{Asum, AsumConfig, sys as cublas_sys};
use cudarc::driver::{DevicePtr, DevicePtrMut, PushKernelArg};

use cli_stressor_cuda_rs::{
    BackendError, PrecisionKind, PrecisionSpec, RNG_TILE_BYTES, StreamMode, fill_random_bytes,
    make_random_host_matrix,
};

use super::backend::CudaBackend;

impl CudaBackend {
    pub(super) fn run_memcpy_path(
        &self,
        spec: &PrecisionSpec,
        size: usize,
        warmup_iters: u32,
        burst_iters: u32,
        seed: u64,
        stream_mode: StreamMode,
    ) -> Result<f64, BackendError> {
        let elem_size = match spec.kind {
            PrecisionKind::BF16 | PrecisionKind::FP16 => 2usize,
            PrecisionKind::FP32 | PrecisionKind::TF32 => 4usize,
            PrecisionKind::FP64 => 8usize,
            PrecisionKind::FP8E4M3FN => 1usize,
            PrecisionKind::INT8 => 1usize,
            PrecisionKind::INT16 => 2usize,
            PrecisionKind::INT32 => 4usize,
        };
        let bytes = size * size * elem_size;
        let lane_count = Self::lane_count(stream_mode);
        let mut srcs = Vec::with_capacity(lane_count);
        let mut dsts = Vec::with_capacity(lane_count);
        if self.gpu_generate_enabled() {
            // Device-side generation: no host RNG, no H2D copy. Content is
            // kernel-generated splitmix data; memcpy never inspects it.
            let kernels = self
                .gpu_fill
                .as_ref()
                .ok_or_else(|| BackendError::Other("gpu fill kernels unavailable".into()))?;
            let n = bytes as u64;
            for lane in 0..lane_count {
                let stream = self.stream_for_lane(lane);
                let src = stream
                    .alloc_zeros::<u8>(bytes)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                unsafe {
                    stream
                        .launch_builder(&kernels.bytes_fn)
                        .arg(&src)
                        .arg(&n)
                        .arg(&(seed.wrapping_add(lane as u64)))
                        .launch(cudarc::driver::LaunchConfig::for_num_elems(
                            bytes.min(u32::MAX as usize) as u32,
                        ))
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                srcs.push(src);
                dsts.push(
                    stream
                        .alloc_zeros::<u8>(bytes)
                        .map_err(|err| BackendError::Other(err.to_string()))?,
                );
            }
        } else {
            // Host path, tile + device-side repeat: only RNG_TILE_BYTES cross
            // PCIe (pageable staging cost scales with bytes); the bulk fill
            // runs D2D on the copy engine at VRAM bandwidth. The tile is
            // shared by all lanes — memcpy never inspects the source.
            let tile_len = RNG_TILE_BYTES.min(bytes);
            let mut tile = vec![0u8; tile_len];
            fill_random_bytes(&mut tile, seed);
            for lane in 0..lane_count {
                let stream = self.stream_for_lane(lane);
                let tile_dev = stream
                    .clone_htod(&tile)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                let mut src = stream
                    .alloc_zeros::<u8>(bytes)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                let mut offset = 0;
                while offset < bytes {
                    let len = tile_len.min(bytes - offset);
                    stream
                        .memcpy_dtod(
                            &tile_dev.slice(0..len),
                            &mut src.slice_mut(offset..offset + len),
                        )
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                    offset += len;
                }
                srcs.push(src);
                dsts.push(
                    stream
                        .alloc_zeros::<u8>(bytes)
                        .map_err(|err| BackendError::Other(err.to_string()))?,
                );
            }
        }
        for _ in 0..warmup_iters {
            for lane in 0..lane_count {
                let stream = self.stream_for_lane(lane);
                stream
                    .memcpy_dtod(&srcs[lane], &mut dsts[lane])
                    .map_err(|err| BackendError::Other(err.to_string()))?;
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
                stream
                    .memcpy_dtod(&srcs[lane], &mut dsts[lane])
                    .map_err(|err| BackendError::Other(err.to_string()))?;
            }
        }
        for lane in 0..lane_count {
            self.stream_for_lane(lane)
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        Ok(op_start.elapsed().as_secs_f64())
    }

    pub(super) fn run_memset_path(
        &self,
        spec: &PrecisionSpec,
        size: usize,
        warmup_iters: u32,
        burst_iters: u32,
        stream_mode: StreamMode,
    ) -> Result<f64, BackendError> {
        let elem_size = match spec.kind {
            PrecisionKind::BF16 | PrecisionKind::FP16 => 2usize,
            PrecisionKind::FP32 | PrecisionKind::TF32 => 4usize,
            PrecisionKind::FP64 => 8usize,
            PrecisionKind::FP8E4M3FN => 1usize,
            PrecisionKind::INT8 => 1usize,
            PrecisionKind::INT16 => 2usize,
            PrecisionKind::INT32 => 4usize,
        };
        let bytes = size * size * elem_size;
        let lane_count = Self::lane_count(stream_mode);
        let mut bufs = Vec::with_capacity(lane_count);
        for lane in 0..lane_count {
            let stream = self.stream_for_lane(lane);
            bufs.push(
                stream
                    .alloc_zeros::<u8>(bytes)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
        }
        for _ in 0..warmup_iters {
            for (lane, buf) in bufs.iter_mut().enumerate().take(lane_count) {
                self.stream_for_lane(lane)
                    .memset_zeros(buf)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
            }
        }
        for lane in 0..lane_count {
            self.stream_for_lane(lane)
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }

        let op_start = Instant::now();
        for _ in 0..burst_iters {
            for (lane, buf) in bufs.iter_mut().enumerate().take(lane_count) {
                self.stream_for_lane(lane)
                    .memset_zeros(buf)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
            }
        }
        for lane in 0..lane_count {
            self.stream_for_lane(lane)
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        Ok(op_start.elapsed().as_secs_f64())
    }

    pub(super) fn run_sgeam_path(
        &self,
        size: usize,
        warmup_iters: u32,
        burst_iters: u32,
        transpose: bool,
        seed: u64,
        stream_mode: StreamMode,
    ) -> Result<f64, BackendError> {
        let lane_count = Self::lane_count(stream_mode);
        let mut rng = StdRng::seed_from_u64(seed);
        let mut a_devs = Vec::with_capacity(lane_count);
        let mut b_devs = Vec::with_capacity(lane_count);
        let mut c_devs = Vec::with_capacity(lane_count);
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
            c_devs.push(
                stream
                    .alloc_zeros::<f32>(size * size)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
        }
        let n = size as i32;
        let transa = if transpose {
            cublas_sys::cublasOperation_t::CUBLAS_OP_T
        } else {
            cublas_sys::cublasOperation_t::CUBLAS_OP_N
        };
        let transb = cublas_sys::cublasOperation_t::CUBLAS_OP_N;
        let alpha = 1.0f32;
        let beta = if transpose { 0.0f32 } else { 1.0f32 };

        for _ in 0..warmup_iters {
            for lane in 0..lane_count {
                let stream = self.stream_for_lane(lane);
                let blas = self.blas_for_lane(lane);
                let (a_ptr, _a_sync) = a_devs[lane].device_ptr(stream);
                let (b_ptr, _b_sync) = b_devs[lane].device_ptr(stream);
                let (c_ptr, _c_sync) = c_devs[lane].device_ptr_mut(stream);
                let status = unsafe {
                    cublas_sys::cublasSgeam(
                        *blas.handle(),
                        transa,
                        transb,
                        n,
                        n,
                        &alpha as *const f32,
                        a_ptr as *const f32,
                        n,
                        &beta as *const f32,
                        b_ptr as *const f32,
                        n,
                        c_ptr as *mut f32,
                        n,
                    )
                };
                if status != cublas_sys::cublasStatus_t::CUBLAS_STATUS_SUCCESS {
                    return Err(BackendError::Other(format!(
                        "cublasSgeam failed: {:?}",
                        status
                    )));
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
                let blas = self.blas_for_lane(lane);
                let (a_ptr, _a_sync) = a_devs[lane].device_ptr(stream);
                let (b_ptr, _b_sync) = b_devs[lane].device_ptr(stream);
                let (c_ptr, _c_sync) = c_devs[lane].device_ptr_mut(stream);
                let status = unsafe {
                    cublas_sys::cublasSgeam(
                        *blas.handle(),
                        transa,
                        transb,
                        n,
                        n,
                        &alpha as *const f32,
                        a_ptr as *const f32,
                        n,
                        &beta as *const f32,
                        b_ptr as *const f32,
                        n,
                        c_ptr as *mut f32,
                        n,
                    )
                };
                if status != cublas_sys::cublasStatus_t::CUBLAS_STATUS_SUCCESS {
                    return Err(BackendError::Other(format!(
                        "cublasSgeam failed: {:?}",
                        status
                    )));
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

    pub(super) fn run_reduction_path(
        &self,
        size: usize,
        warmup_iters: u32,
        burst_iters: u32,
        seed: u64,
        stream_mode: StreamMode,
    ) -> Result<f64, BackendError> {
        let lane_count = Self::lane_count(stream_mode);
        let mut rng = StdRng::seed_from_u64(seed);
        let mut xs = Vec::with_capacity(lane_count);
        for lane in 0..lane_count {
            let stream = self.stream_for_lane(lane);
            let x_host = make_random_host_matrix(size, rng.random::<u64>());
            xs.push(
                stream
                    .clone_htod(&x_host.data)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
        }
        let cfg = AsumConfig {
            n: (size * size) as i32,
            incx: 1,
        };
        let mut outs = vec![0.0f32; lane_count];
        for _ in 0..warmup_iters {
            for lane in 0..lane_count {
                let blas = self.blas_for_lane(lane);
                unsafe {
                    blas.asum(cfg, &xs[lane], &mut outs[lane])
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
                    blas.asum(cfg, &xs[lane], &mut outs[lane])
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
