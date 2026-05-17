use cli_stressor_cuda_rs::{
    Backend, BackendError, DeviceInfo, HostMatrix, PrecisionKind, PrecisionSpec,
};
use cudarc::cublas::{CudaBlas, Gemm, GemmConfig, sys as cublas_sys};
use cudarc::driver::sys as cuda_sys;
use cudarc::driver::{CudaContext, CudaStream};
use half::{bf16, f16};
use std::ffi::CStr;
use std::sync::Arc;

pub struct CudaBackend {
    stream: Arc<CudaStream>,
    blas: CudaBlas,
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
    pub fn new() -> Result<Self, BackendError> {
        unsafe {
            let res = cuda_sys::cuInit(0);
            if res as u32 != 0 {
                return Err(BackendError::Other(format!(
                    "cuInit failed: error code {}",
                    res as u32
                )));
            }
        }
        let ctx = CudaContext::new(0).map_err(|err| BackendError::Other(err.to_string()))?;
        let stream = ctx.default_stream();
        let blas =
            CudaBlas::new(stream.clone()).map_err(|err| BackendError::Other(err.to_string()))?;
        let info = query_device_info()?;
        Ok(Self { stream, blas, info })
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
        let status = unsafe { cublas_sys::cublasSetMathMode(*self.blas.handle(), mode) };
        if status == cublas_sys::cublasStatus_t::CUBLAS_STATUS_SUCCESS {
            Ok(())
        } else {
            Err(BackendError::Other(format!(
                "cublasSetMathMode failed: {:?}",
                status
            )))
        }
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

    fn synchronize(&self) -> Result<(), BackendError> {
        self.stream
            .synchronize()
            .map_err(|err| BackendError::Other(err.to_string()))
    }

    fn empty_cache(&self) -> Result<(), BackendError> {
        Ok(())
    }
}

fn query_device_info() -> Result<DeviceInfo, BackendError> {
    let mut device = 0;
    unsafe {
        cuda_sys::cuDeviceGet(&mut device, 0);
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
