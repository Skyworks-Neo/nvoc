//! GEMM stress path — cuBLAS matrix multiply across the FP precisions.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::time::Instant;

use cudarc::cublas::{Gemm, GemmConfig, result as cublas_res, sys as cublas_sys};
use cudarc::driver::{DevicePtr, DevicePtrMut};
use half::{bf16, f16};

use cli_stressor_cuda_rs::{
    BackendError, PrecisionKind, PrecisionSpec, StreamMode,
    make_random_host_matrix,
};

use super::backend::CudaBackend;

pub(super) struct GemmPathConfig<'a> {
    pub spec: &'a PrecisionSpec,
    pub size: usize,
    pub warmup_iters: u32,
    pub burst_iters: u32,
    pub transpose_prob: f64,
    pub seed: u64,
    pub stream_mode: StreamMode,
}

impl CudaBackend {
    pub(super) fn run_gemm_path(&self, config: GemmPathConfig<'_>) -> Result<f64, BackendError> {
        let GemmPathConfig {
            spec,
            size,
            warmup_iters,
            burst_iters,
            transpose_prob,
            seed,
            stream_mode,
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
                Ok(op_start.elapsed().as_secs_f64())
            }
            PrecisionKind::FP16 => {
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
                Ok(op_start.elapsed().as_secs_f64())
            }
            PrecisionKind::FP32 | PrecisionKind::TF32 => {
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
                Ok(op_start.elapsed().as_secs_f64())
            }
            PrecisionKind::FP64 => {
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
                Ok(op_start.elapsed().as_secs_f64())
            }
            PrecisionKind::INT8 => {
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
            }
            PrecisionKind::FP8E4M3FN => Err(BackendError::Other(
                "GEMM path unsupported for FP8".to_string(),
            )),
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
}
