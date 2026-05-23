use cli_stressor_cuda_rs::PciBusAddress;
use cli_stressor_cuda_rs::{
    Backend, BackendError, CudaDeviceEnumInfo, DeviceInfo, HostMatrix, KernelPathRequest,
    KernelType, PrecisionKind, PrecisionSpec, StreamMode, make_random_host_matrix,
};
use cudarc::cublas::{Asum, AsumConfig, CudaBlas, Gemm, GemmConfig, sys as cublas_sys};
use cudarc::driver::sys as cuda_sys;
use cudarc::driver::{
    CudaContext, CudaFunction, CudaModule, CudaStream, DevicePtr, DevicePtrMut, LaunchConfig,
    PushKernelArg,
};
use cudarc::nvrtc::compile_ptx;
use half::{bf16, f16};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::ffi::CStr;
use std::sync::Arc;
use std::time::Instant;

#[cfg(feature = "vulkan")]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CudaDeviceIdentity {
    pub uuid: [u8; 16],
    pub pci_bus: Option<PciBusAddress>,
}

struct GemmPathConfig<'a> {
    spec: &'a PrecisionSpec,
    size: usize,
    warmup_iters: u32,
    burst_iters: u32,
    transpose_prob: f64,
    seed: u64,
    stream_mode: StreamMode,
}

pub struct CudaBackend {
    #[cfg_attr(not(feature = "vulkan"), allow(dead_code))]
    device_index: u32,
    _ctx: Arc<CudaContext>,
    stream: Arc<CudaStream>,
    aux_streams: Vec<Arc<CudaStream>>,
    blas: CudaBlas,
    aux_blas: Vec<CudaBlas>,
    _atomic_module: Option<Arc<CudaModule>>,
    atomic_fn: Option<CudaFunction>,
    info: DeviceInfo,
}

pub enum CudaMatrix {
    BF16 {
        data: cudarc::driver::CudaSlice<bf16>,
        size: usize,
    },
    F16 {
        data: cudarc::driver::CudaSlice<f16>,
        size: usize,
    },
    F32 {
        data: cudarc::driver::CudaSlice<f32>,
        size: usize,
    },
    F64 {
        data: cudarc::driver::CudaSlice<f64>,
        size: usize,
    },
}

pub enum CudaOutput {
    BF16 {
        data: cudarc::driver::CudaSlice<bf16>,
    },
    F16 {
        data: cudarc::driver::CudaSlice<f16>,
    },
    F32 {
        data: cudarc::driver::CudaSlice<f32>,
    },
    F64 {
        data: cudarc::driver::CudaSlice<f64>,
    },
}

impl CudaBackend {
    #[allow(dead_code)]
    pub fn new() -> Result<Self, BackendError> {
        Self::new_with_device(0)
    }

    pub fn new_with_device(gpu_index: u32) -> Result<Self, BackendError> {
        unsafe {
            let res = cuda_sys::cuInit(0);
            if res as u32 != 0 {
                return Err(BackendError::Other(format!(
                    "cuInit failed: error code {}",
                    res as u32
                )));
            }
        }
        let ctx = CudaContext::new(gpu_index as usize)
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let stream = ctx.default_stream();
        let stream2 = stream
            .fork()
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let stream3 = stream
            .fork()
            .map_err(|err| BackendError::Other(err.to_string()))?;
        let blas =
            CudaBlas::new(stream.clone()).map_err(|err| BackendError::Other(err.to_string()))?;
        let blas2 =
            CudaBlas::new(stream2.clone()).map_err(|err| BackendError::Other(err.to_string()))?;
        let blas3 =
            CudaBlas::new(stream3.clone()).map_err(|err| BackendError::Other(err.to_string()))?;
        let (atomic_module, atomic_fn) = match build_atomic_kernel(&ctx) {
            Ok((module, func)) => (Some(module), Some(func)),
            Err(_) => (None, None),
        };
        let info = query_device_info_for_index(gpu_index)?;
        Ok(Self {
            device_index: gpu_index,
            _ctx: ctx,
            stream,
            aux_streams: vec![stream2, stream3],
            blas,
            aux_blas: vec![blas2, blas3],
            _atomic_module: atomic_module,
            atomic_fn,
            info,
        })
    }

    #[cfg(feature = "vulkan")]
    pub fn device_identity(&self) -> Result<CudaDeviceIdentity, BackendError> {
        let uuid = query_cuda_device_uuid(self.device_index)?;
        let pci_bus = query_cuda_device_pci_bus_address(self.device_index)?;
        Ok(CudaDeviceIdentity { uuid, pci_bus })
    }

    fn lane_count(stream_mode: StreamMode) -> usize {
        stream_mode.stream_count().clamp(1, 3)
    }

    fn stream_for_lane(&self, lane: usize) -> &Arc<CudaStream> {
        match lane {
            0 => &self.stream,
            1 => &self.aux_streams[0],
            _ => &self.aux_streams[1],
        }
    }

    fn blas_for_lane(&self, lane: usize) -> &CudaBlas {
        match lane {
            0 => &self.blas,
            1 => &self.aux_blas[0],
            _ => &self.aux_blas[1],
        }
    }

    fn run_gemm_path(&self, config: GemmPathConfig<'_>) -> Result<f64, BackendError> {
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
                for _ in 0..warmup_iters {
                    for lane in 0..lane_count {
                        let stream = self.stream_for_lane(lane);
                        let blas = self.blas_for_lane(lane);
                        let mut c = stream
                            .alloc_zeros::<bf16>(size * size)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c)
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
                        let blas = self.blas_for_lane(lane);
                        let mut c = stream
                            .alloc_zeros::<bf16>(size * size)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c)
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
                for _ in 0..warmup_iters {
                    for lane in 0..lane_count {
                        let stream = self.stream_for_lane(lane);
                        let blas = self.blas_for_lane(lane);
                        let mut c = stream
                            .alloc_zeros::<f16>(size * size)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c)
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
                        let blas = self.blas_for_lane(lane);
                        let mut c = stream
                            .alloc_zeros::<f16>(size * size)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c)
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
                for _ in 0..warmup_iters {
                    for lane in 0..lane_count {
                        let stream = self.stream_for_lane(lane);
                        let blas = self.blas_for_lane(lane);
                        let mut c = stream
                            .alloc_zeros::<f32>(size * size)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c)
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
                        let blas = self.blas_for_lane(lane);
                        let mut c = stream
                            .alloc_zeros::<f32>(size * size)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c)
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
                for _ in 0..warmup_iters {
                    for lane in 0..lane_count {
                        let stream = self.stream_for_lane(lane);
                        let blas = self.blas_for_lane(lane);
                        let mut c = stream
                            .alloc_zeros::<f64>(size * size)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c)
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
                        let blas = self.blas_for_lane(lane);
                        let mut c = stream
                            .alloc_zeros::<f64>(size * size)
                            .map_err(|err| BackendError::Other(err.to_string()))?;
                        unsafe {
                            blas.gemm(cfg, &a_devs[lane], &b_devs[lane], &mut c)
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
            PrecisionKind::FP8E4M3FN => Err(BackendError::Other(
                "GEMM path unsupported for FP8".to_string(),
            )),
        }
    }

    fn run_memcpy_path(
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
        };
        let bytes = size * size * elem_size;
        let mut rng = StdRng::seed_from_u64(seed);
        let lane_count = Self::lane_count(stream_mode);
        let mut srcs = Vec::with_capacity(lane_count);
        let mut dsts = Vec::with_capacity(lane_count);
        for lane in 0..lane_count {
            let stream = self.stream_for_lane(lane);
            let host: Vec<u8> = (0..bytes).map(|_| rng.random::<u8>()).collect();
            srcs.push(
                stream
                    .clone_htod(&host)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
            dsts.push(
                stream
                    .alloc_zeros::<u8>(bytes)
                    .map_err(|err| BackendError::Other(err.to_string()))?,
            );
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

    fn run_memset_path(
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

    fn run_sgeam_path(
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

    fn run_reduction_path(
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

    fn run_atomic_path(
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

impl Backend for CudaBackend {
    type Matrix = CudaMatrix;
    type Output = CudaOutput;

    fn device_info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn supports_precision(&self, spec: &PrecisionSpec) -> Result<(), String> {
        let cc = self.info.compute_capability;
        match spec.kind {
            PrecisionKind::FP8E4M3FN => {
                return Err("FP8 not implemented in this build (cuBLASLt required)".to_string());
            }
            PrecisionKind::BF16 => {
                if let Some((major, minor)) = cc
                    && major < 8
                {
                    return Err(format!(
                        "BF16 requires SM80+, current SM{}.{}",
                        major, minor
                    ));
                }
            }
            PrecisionKind::TF32 => {
                if let Some((major, minor)) = cc
                    && major < 8
                {
                    return Err(format!(
                        "TF32 requires SM80+, current SM{}.{}",
                        major, minor
                    ));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn set_tf32(&mut self, enabled: Option<bool>) -> Result<(), BackendError> {
        if enabled.is_none() {
            return Ok(());
        }
        let mode = if enabled == Some(true) {
            cublas_sys::cublasMath_t::CUBLAS_TF32_TENSOR_OP_MATH
        } else {
            cublas_sys::cublasMath_t::CUBLAS_DEFAULT_MATH
        };
        for blas in std::iter::once(&self.blas).chain(self.aux_blas.iter()) {
            let status = unsafe { cublas_sys::cublasSetMathMode(*blas.handle(), mode) };
            if status != cublas_sys::cublasStatus_t::CUBLAS_STATUS_SUCCESS {
                return Err(BackendError::Other(format!(
                    "cublasSetMathMode failed: {:?}",
                    status
                )));
            }
        }
        Ok(())
    }

    fn upload_matrix(
        &self,
        host: &HostMatrix,
        spec: &PrecisionSpec,
    ) -> Result<Self::Matrix, BackendError> {
        match spec.kind {
            PrecisionKind::BF16 => {
                let data: Vec<bf16> = host.data.iter().map(|v| bf16::from_f32(*v)).collect();
                let dev = self
                    .stream
                    .clone_htod(&data)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                Ok(CudaMatrix::BF16 {
                    data: dev,
                    size: host.size,
                })
            }
            PrecisionKind::FP16 => {
                let data: Vec<f16> = host.data.iter().map(|v| f16::from_f32(*v)).collect();
                let dev = self
                    .stream
                    .clone_htod(&data)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                Ok(CudaMatrix::F16 {
                    data: dev,
                    size: host.size,
                })
            }
            PrecisionKind::FP32 | PrecisionKind::TF32 => {
                let dev = self
                    .stream
                    .clone_htod(&host.data)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                Ok(CudaMatrix::F32 {
                    data: dev,
                    size: host.size,
                })
            }
            PrecisionKind::FP64 => {
                let data: Vec<f64> = host.data.iter().map(|v| *v as f64).collect();
                let dev = self
                    .stream
                    .clone_htod(&data)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                Ok(CudaMatrix::F64 {
                    data: dev,
                    size: host.size,
                })
            }
            PrecisionKind::FP8E4M3FN => Err(BackendError::Other(
                "precision not supported in CUDA backend".to_string(),
            )),
        }
    }

    fn gemm(
        &mut self,
        a: &Self::Matrix,
        b: &Self::Matrix,
        transpose_a: bool,
        transpose_b: bool,
    ) -> Result<Self::Output, BackendError> {
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

        match (a, b) {
            (CudaMatrix::BF16 { data: a, size }, CudaMatrix::BF16 { data: b, .. }) => {
                let mut c = self
                    .stream
                    .alloc_zeros::<bf16>(size * size)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                let cfg = GemmConfig {
                    transa: op_a,
                    transb: op_b,
                    m: *size as i32,
                    n: *size as i32,
                    k: *size as i32,
                    alpha: bf16::from_f32(1.0),
                    lda: *size as i32,
                    ldb: *size as i32,
                    beta: bf16::from_f32(0.0),
                    ldc: *size as i32,
                };
                unsafe {
                    self.blas
                        .gemm(cfg, a, b, &mut c)
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                Ok(CudaOutput::BF16 { data: c })
            }
            (CudaMatrix::F16 { data: a, size }, CudaMatrix::F16 { data: b, .. }) => {
                let mut c = self
                    .stream
                    .alloc_zeros::<f16>(size * size)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                let cfg = GemmConfig {
                    transa: op_a,
                    transb: op_b,
                    m: *size as i32,
                    n: *size as i32,
                    k: *size as i32,
                    alpha: f16::from_f32(1.0),
                    lda: *size as i32,
                    ldb: *size as i32,
                    beta: f16::from_f32(0.0),
                    ldc: *size as i32,
                };
                unsafe {
                    self.blas
                        .gemm(cfg, a, b, &mut c)
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                Ok(CudaOutput::F16 { data: c })
            }
            (CudaMatrix::F32 { data: a, size }, CudaMatrix::F32 { data: b, .. }) => {
                let mut c = self
                    .stream
                    .alloc_zeros::<f32>(size * size)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                let cfg = GemmConfig {
                    transa: op_a,
                    transb: op_b,
                    m: *size as i32,
                    n: *size as i32,
                    k: *size as i32,
                    alpha: 1.0f32,
                    lda: *size as i32,
                    ldb: *size as i32,
                    beta: 0.0f32,
                    ldc: *size as i32,
                };
                unsafe {
                    self.blas
                        .gemm(cfg, a, b, &mut c)
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                Ok(CudaOutput::F32 { data: c })
            }
            (CudaMatrix::F64 { data: a, size }, CudaMatrix::F64 { data: b, .. }) => {
                let mut c = self
                    .stream
                    .alloc_zeros::<f64>(size * size)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                let cfg = GemmConfig {
                    transa: op_a,
                    transb: op_b,
                    m: *size as i32,
                    n: *size as i32,
                    k: *size as i32,
                    alpha: 1.0f64,
                    lda: *size as i32,
                    ldb: *size as i32,
                    beta: 0.0f64,
                    ldc: *size as i32,
                };
                unsafe {
                    self.blas
                        .gemm(cfg, a, b, &mut c)
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                }
                Ok(CudaOutput::F64 { data: c })
            }
            _ => Err(BackendError::Other(
                "mismatched matrix precision".to_string(),
            )),
        }
    }

    fn output_to_f32(&self, output: &Self::Output) -> Result<Vec<f32>, BackendError> {
        match output {
            CudaOutput::BF16 { data, .. } => {
                let host: Vec<bf16> = self
                    .stream
                    .clone_dtoh(data)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                self.stream
                    .synchronize()
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                Ok(host.into_iter().map(|v| v.to_f32()).collect())
            }
            CudaOutput::F16 { data, .. } => {
                let host: Vec<f16> = self
                    .stream
                    .clone_dtoh(data)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                self.stream
                    .synchronize()
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                Ok(host.into_iter().map(|v| v.to_f32()).collect())
            }
            CudaOutput::F32 { data, .. } => self
                .stream
                .clone_dtoh(data)
                .map_err(|err| BackendError::Other(err.to_string()))
                .and_then(|host| {
                    self.stream
                        .synchronize()
                        .map_err(|err| BackendError::Other(err.to_string()))?;
                    Ok(host)
                }),
            CudaOutput::F64 { data, .. } => {
                let host: Vec<f64> = self
                    .stream
                    .clone_dtoh(data)
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                self.stream
                    .synchronize()
                    .map_err(|err| BackendError::Other(err.to_string()))?;
                Ok(host.into_iter().map(|v| v as f32).collect())
            }
        }
    }

    fn run_kernel_path(&mut self, request: KernelPathRequest<'_>) -> Result<f64, BackendError> {
        let KernelPathRequest {
            spec,
            kind,
            size,
            warmup_iters,
            burst_iters,
            transpose_prob,
            seed,
            stream_mode,
        } = request;

        match kind {
            KernelType::Gemm => self.run_gemm_path(GemmPathConfig {
                spec,
                size,
                warmup_iters,
                burst_iters,
                transpose_prob,
                seed,
                stream_mode,
            }),
            KernelType::Memcpy => {
                self.run_memcpy_path(spec, size, warmup_iters, burst_iters, seed, stream_mode)
            }
            KernelType::Memset => {
                self.run_memset_path(spec, size, warmup_iters, burst_iters, stream_mode)
            }
            KernelType::Transpose => {
                self.run_sgeam_path(size, warmup_iters, burst_iters, true, seed, stream_mode)
            }
            KernelType::Elementwise => {
                self.run_sgeam_path(size, warmup_iters, burst_iters, false, seed, stream_mode)
            }
            KernelType::Reduction => {
                self.run_reduction_path(size, warmup_iters, burst_iters, seed, stream_mode)
            }
            KernelType::Atomic => {
                self.run_atomic_path(size, warmup_iters, burst_iters, seed, stream_mode)
            }
        }
    }

    fn synchronize(&self) -> Result<(), BackendError> {
        self.stream
            .synchronize()
            .map_err(|err| BackendError::Other(err.to_string()))?;
        for stream in &self.aux_streams {
            stream
                .synchronize()
                .map_err(|err| BackendError::Other(err.to_string()))?;
        }
        Ok(())
    }

    fn empty_cache(&self) -> Result<(), BackendError> {
        Ok(())
    }
}

fn query_device_info_for_index(device_index: u32) -> Result<DeviceInfo, BackendError> {
    let mut device = 0;
    unsafe {
        let res = cuda_sys::cuDeviceGet(&mut device, device_index as i32);
        if res as u32 != 0 {
            return Err(BackendError::Other(format!(
                "cuDeviceGet failed for device {}: error code {}",
                device_index, res as u32
            )));
        }
    }

    let mut name_buf = [0i8; 128];
    unsafe {
        cuda_sys::cuDeviceGetName(name_buf.as_mut_ptr(), name_buf.len() as i32, device);
    }
    let name = unsafe { CStr::from_ptr(name_buf.as_ptr()) }
        .to_string_lossy()
        .trim_end_matches('\0')
        .to_string();

    let mut total_mem = 0usize;
    unsafe {
        cuda_sys::cuDeviceTotalMem_v2(&mut total_mem as *mut usize, device);
    }
    let total_mem_gb = if total_mem > 0 {
        Some(total_mem as f64 / 1024.0 / 1024.0 / 1024.0)
    } else {
        None
    };

    let mut major = 0i32;
    let mut minor = 0i32;
    unsafe {
        cuda_sys::cuDeviceGetAttribute(
            &mut major,
            cuda_sys::CUdevice_attribute::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR,
            device,
        );
        cuda_sys::cuDeviceGetAttribute(
            &mut minor,
            cuda_sys::CUdevice_attribute::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR,
            device,
        );
    }

    Ok(DeviceInfo {
        name,
        total_mem_gb,
        compute_capability: Some((major, minor)),
    })
}

pub fn enumerate_cuda_devices() -> Result<Vec<CudaDeviceEnumInfo>, BackendError> {
    unsafe {
        let res = cuda_sys::cuInit(0);
        if res as u32 != 0 {
            return Err(BackendError::Other(format!(
                "cuInit failed: error code {}",
                res as u32
            )));
        }
    }

    let mut device_count = 0i32;
    unsafe {
        let res = cuda_sys::cuDeviceGetCount(&mut device_count);
        if res as u32 != 0 {
            return Err(BackendError::Other(format!(
                "cuDeviceGetCount failed: error code {}",
                res as u32
            )));
        }
    }

    let mut devices = Vec::new();
    for idx in 0..device_count {
        let device_index = idx as u32;
        let mut device = 0i32;
        unsafe {
            cuda_sys::cuDeviceGet(&mut device, idx);
        }

        let mut name_buf = [0i8; 128];
        unsafe {
            cuda_sys::cuDeviceGetName(name_buf.as_mut_ptr(), name_buf.len() as i32, device);
        }
        let device_name = unsafe { CStr::from_ptr(name_buf.as_ptr()) }
            .to_string_lossy()
            .trim_end_matches('\0')
            .to_string();

        let uuid = fetch_device_uuid(device)?;
        let pci_bus = fetch_device_pci_bus(device)?;

        let mut total_mem = 0usize;
        unsafe {
            cuda_sys::cuDeviceTotalMem_v2(&mut total_mem as *mut usize, device);
        }
        let total_mem_gb = if total_mem > 0 {
            Some(total_mem as f64 / 1024.0 / 1024.0 / 1024.0)
        } else {
            None
        };

        let mut major = 0i32;
        let mut minor = 0i32;
        unsafe {
            cuda_sys::cuDeviceGetAttribute(
                &mut major,
                cuda_sys::CUdevice_attribute::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR,
                device,
            );
            cuda_sys::cuDeviceGetAttribute(
                &mut minor,
                cuda_sys::CUdevice_attribute::CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR,
                device,
            );
        }
        let compute_capability = Some((major, minor));

        devices.push(CudaDeviceEnumInfo {
            device_index,
            device_name,
            uuid,
            pci_bus,
            compute_capability,
            total_mem_gb,
        });
    }

    Ok(devices)
}

pub fn resolve_device_index_by_uuid(target_uuid: [u8; 16]) -> Result<u32, String> {
    let devices =
        enumerate_cuda_devices().map_err(|e| format!("failed to enumerate devices: {e}"))?;
    for dev in &devices {
        if dev.uuid == target_uuid {
            println!(
                "[CUDA] Selected device index {} by UUID match: {}",
                dev.device_index, dev.device_name
            );
            return Ok(dev.device_index);
        }
    }
    Err(format!(
        "no CUDA device found with UUID {}",
        format_uuid_hex(&target_uuid)
    ))
}

pub fn resolve_device_index_by_pci_bus(target_pci: PciBusAddress) -> Result<u32, String> {
    let devices =
        enumerate_cuda_devices().map_err(|e| format!("failed to enumerate devices: {e}"))?;
    for dev in &devices {
        if let Some(pci) = dev.pci_bus
            && pci == target_pci
        {
            println!(
                "[CUDA] Selected device index {} by PCI match: {}",
                dev.device_index, dev.device_name
            );
            return Ok(dev.device_index);
        }
    }
    Err(format!(
        "no CUDA device found with PCI {}",
        format_pci_address(&target_pci)
    ))
}

pub fn resolve_device_index_by_sorted_index(sorted_index: u32) -> Result<u32, String> {
    let mut devices =
        enumerate_cuda_devices().map_err(|e| format!("failed to enumerate devices: {e}"))?;

    // Sort by PCI bus address
    devices.sort_by(|a, b| match (a.pci_bus, b.pci_bus) {
        (Some(pci_a), Some(pci_b)) => (pci_a.domain, pci_a.bus, pci_a.device, pci_a.function)
            .cmp(&(pci_b.domain, pci_b.bus, pci_b.device, pci_b.function)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });

    if sorted_index >= devices.len() as u32 {
        return Err(format!(
            "sorted index {} out of range (available: 0-{})",
            sorted_index,
            devices.len() - 1
        ));
    }

    let dev = &devices[sorted_index as usize];
    println!(
        "[CUDA] Selected device index {} (sorted position {}) by index: {}",
        dev.device_index, sorted_index, dev.device_name
    );
    Ok(dev.device_index)
}

fn format_uuid_hex(uuid: &[u8; 16]) -> String {
    uuid.iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join("")
}

fn format_pci_address(pci: &PciBusAddress) -> String {
    format!(
        "{:04X}:{:02X}:{:02X}.{}",
        pci.domain, pci.bus, pci.device, pci.function
    )
}

fn fetch_device_uuid(device: i32) -> Result<[u8; 16], BackendError> {
    let mut raw_uuid = std::mem::MaybeUninit::<cuda_sys::CUuuid>::zeroed();
    unsafe {
        let res = cuda_sys::cuDeviceGetUuid_v2(raw_uuid.as_mut_ptr(), device);
        if res as u32 != 0 {
            // Return zero UUID if not supported instead of failing
            return Ok([0u8; 16]);
        }
        let raw_uuid = raw_uuid.assume_init();
        let uuid: [u8; 16] = std::mem::transmute(raw_uuid);
        Ok(uuid)
    }
}

fn fetch_device_pci_bus(device: i32) -> Result<Option<PciBusAddress>, BackendError> {
    let mut buf = [0i8; 32];
    unsafe {
        let res = cuda_sys::cuDeviceGetPCIBusId(buf.as_mut_ptr(), buf.len() as i32, device);
        if res as u32 != 0 {
            return Ok(None);
        }
    }

    let pci_bus_id = unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .trim()
        .to_string();
    if pci_bus_id.is_empty() {
        return Ok(None);
    }

    parse_cuda_pci_bus_id(&pci_bus_id)
        .map(Some)
        .map_err(BackendError::Other)
}

#[cfg(feature = "vulkan")]
fn query_cuda_device_uuid(device_index: u32) -> Result<[u8; 16], BackendError> {
    let mut device = 0;
    unsafe {
        let res = cuda_sys::cuDeviceGet(&mut device, device_index as i32);
        if res as u32 != 0 {
            return Err(BackendError::Other(format!(
                "cuDeviceGet failed: error code {}",
                res as u32
            )));
        }
    }

    let mut raw_uuid = std::mem::MaybeUninit::<cuda_sys::CUuuid>::zeroed();
    unsafe {
        let res = cuda_sys::cuDeviceGetUuid_v2(raw_uuid.as_mut_ptr(), device);
        if res as u32 != 0 {
            return Err(BackendError::Other(format!(
                "cuDeviceGetUuid_v2 failed: error code {}",
                res as u32
            )));
        }
        let raw_uuid = raw_uuid.assume_init();
        let uuid: [u8; 16] = std::mem::transmute(raw_uuid);
        Ok(uuid)
    }
}

#[cfg(feature = "vulkan")]
fn query_cuda_device_pci_bus_address(
    device_index: u32,
) -> Result<Option<PciBusAddress>, BackendError> {
    let mut device = 0;
    unsafe {
        let res = cuda_sys::cuDeviceGet(&mut device, device_index as i32);
        if res as u32 != 0 {
            return Err(BackendError::Other(format!(
                "cuDeviceGet failed: error code {}",
                res as u32
            )));
        }
    }

    let mut buf = [0i8; 32];
    unsafe {
        let res = cuda_sys::cuDeviceGetPCIBusId(buf.as_mut_ptr(), buf.len() as i32, device);
        if res as u32 != 0 {
            return Ok(None);
        }
    }

    let pci_bus_id = unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .trim()
        .to_string();
    if pci_bus_id.is_empty() {
        return Ok(None);
    }

    parse_cuda_pci_bus_id(&pci_bus_id)
        .map(Some)
        .map_err(BackendError::Other)
}

fn parse_cuda_pci_bus_id(raw: &str) -> Result<PciBusAddress, String> {
    let raw = raw.trim();
    let (domain_raw, rest) = raw
        .split_once(':')
        .ok_or_else(|| format!("invalid CUDA PCI bus id: {raw}"))?;
    let (bus_raw, rest) = rest
        .split_once(':')
        .ok_or_else(|| format!("invalid CUDA PCI bus id: {raw}"))?;
    let (device_raw, function_raw) = rest
        .split_once('.')
        .ok_or_else(|| format!("invalid CUDA PCI bus id: {raw}"))?;

    let domain = u32::from_str_radix(domain_raw, 16)
        .map_err(|_| format!("invalid PCI domain in bus id: {domain_raw}"))?;
    let bus = u32::from_str_radix(bus_raw, 16)
        .map_err(|_| format!("invalid PCI bus in bus id: {bus_raw}"))?;
    let device = u32::from_str_radix(device_raw, 16)
        .map_err(|_| format!("invalid PCI device in bus id: {device_raw}"))?;
    let function = u32::from_str_radix(function_raw, 16)
        .map_err(|_| format!("invalid PCI function in bus id: {function_raw}"))?;

    Ok(PciBusAddress {
        domain,
        bus,
        device,
        function,
    })
}

fn build_atomic_kernel(
    ctx: &Arc<CudaContext>,
) -> Result<(Arc<CudaModule>, CudaFunction), BackendError> {
    let src = r#"
extern "C" __global__ void atomic_accum(const float* x, unsigned int n, unsigned int* out) {
    unsigned int idx = (unsigned int)(blockIdx.x * blockDim.x + threadIdx.x);
    if (idx < n) {
        unsigned int v = (idx & 31U) == 0U ? ((__float_as_uint(x[idx]) & 1U) + 1U) : 1U;
        atomicAdd(out, v);
    }
}
"#;
    let ptx = compile_ptx(src).map_err(|err| BackendError::Other(err.to_string()))?;
    let module = ctx
        .load_module(ptx)
        .map_err(|err| BackendError::Other(err.to_string()))?;
    let func = module
        .load_function("atomic_accum")
        .map_err(|err| BackendError::Other(err.to_string()))?;
    Ok((module, func))
}
