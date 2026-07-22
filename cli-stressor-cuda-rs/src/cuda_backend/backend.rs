//! [`CudaBackend`] definition, constructor, and the [`Backend`] trait impl that
//! dispatches to the per-workload submodules (`gemm`, `mem_ops`, `atomic`, …).

use std::sync::Arc;

use cudarc::cublas::{CudaBlas, Gemm, GemmConfig, sys as cublas_sys};
use cudarc::driver::sys as cuda_sys;
use cudarc::driver::{CudaContext, CudaFunction, CudaModule, CudaStream};
use half::{bf16, f16};

use cli_stressor_cuda_rs::{
    Backend, BackendError, DeviceInfo, HostMatrix, KernelPathRequest, KernelType, PrecisionKind,
    PrecisionSpec,
};

use super::atomic::build_atomic_kernel;
use super::device::query_device_info_for_index;
use super::gemm::GemmPathConfig;

#[cfg(feature = "vulkan")]
use cli_stressor_cuda_rs::PciBusAddress;

#[cfg(feature = "vulkan")]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CudaDeviceIdentity {
    pub uuid: [u8; 16],
    pub pci_bus: Option<PciBusAddress>,
}

pub struct CudaBackend {
    #[cfg_attr(not(feature = "vulkan"), allow(dead_code))]
    pub(super) device_index: u32,
    pub(super) _ctx: Arc<CudaContext>,
    pub(super) stream: Arc<CudaStream>,
    pub(super) aux_streams: Vec<Arc<CudaStream>>,
    pub(super) blas: CudaBlas,
    pub(super) aux_blas: Vec<CudaBlas>,
    pub(super) _atomic_module: Option<Arc<CudaModule>>,
    pub(super) atomic_fn: Option<CudaFunction>,
    pub(super) _intalu_module: Option<Arc<CudaModule>>,
    pub(super) intalu_fn: Option<CudaFunction>,
    pub(super) info: DeviceInfo,
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
            Err(err) => {
                println!(
                    "Warning: Atomic kernel build failed (possibly CUDA vs GPU mismatch): {}",
                    err
                );
                (None, None)
            }
        };
        // INT ALU kernel is arch-targeted (needs DeviceInfo), so query SM first.
        let info = query_device_info_for_index(gpu_index)?;
        let (intalu_module, intalu_fn) = match super::int_alu::build_intalu_kernel(&ctx, &info) {
            Ok((module, func)) => (Some(module), Some(func)),
            Err(err) => {
                println!(
                    "Warning: INT ALU kernel build failed (INT stress path disabled): {}",
                    err
                );
                (None, None)
            }
        };
        Ok(Self {
            device_index: gpu_index,
            _ctx: ctx,
            stream,
            aux_streams: vec![stream2, stream3],
            blas,
            aux_blas: vec![blas2, blas3],
            _atomic_module: atomic_module,
            atomic_fn,
            _intalu_module: intalu_module,
            intalu_fn,
            info,
        })
    }

    #[cfg(feature = "vulkan")]
    pub fn device_identity(&self) -> Result<CudaDeviceIdentity, BackendError> {
        let uuid = super::device::query_cuda_device_uuid(self.device_index)?;
        let pci_bus = super::device::query_cuda_device_pci_bus_address(self.device_index)?;
        Ok(CudaDeviceIdentity { uuid, pci_bus })
    }
}

impl Backend for CudaBackend {
    type Matrix = CudaMatrix;
    type Output = CudaOutput;

    fn device_info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn supports_precision(&self, spec: &PrecisionSpec) -> Result<(), String> {
        self.info.supports_precision(spec)
    }

    fn supports_kernel(&self, kind: KernelType) -> bool {
        match kind {
            KernelType::Atomic => self.atomic_fn.is_some(),
            KernelType::IntAlu => self.intalu_fn.is_some(),
            _ => true,
        }
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
            // INT precisions are exercised through the dedicated `gemm` (INT8)
            // and `intalu` kernel paths, not the FP validation/upload pipeline.
            PrecisionKind::INT8 | PrecisionKind::INT16 | PrecisionKind::INT32 => Err(
                BackendError::Other("INT precision has no FP upload_matrix path".to_string()),
            ),
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
            KernelType::IntAlu => {
                self.run_intalu_path(spec, size, warmup_iters, burst_iters, seed, stream_mode)
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

// The per-workload `run_*_path` / `stream_for_lane` / `blas_for_lane` /
// `lane_count` methods are defined in the sibling submodules (`gemm`,
// `mem_ops`, `atomic`, `lanes`) as additional `impl CudaBackend` blocks. They
// share the struct via the `pub(super)` fields/helpers above.
