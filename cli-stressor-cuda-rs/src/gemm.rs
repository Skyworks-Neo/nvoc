use anyhow::{Context, Result};
use cudarc::cublas::sys as cublas_sys;
use cudarc::cublas::{CudaBlas, Gemm, GemmConfig};
use cudarc::driver::{CudaContext, CudaStream, DevicePtr};
use half::{bf16, f16};
use std::ptr::NonNull;
use std::sync::Arc;

pub use crate::abft::generate_matrix;

use cudarc::driver::sys as cuda_sys;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Precision {
    #[clap(name = "fp32")]
    Fp32,
    #[clap(name = "fp16")]
    Fp16,
    #[clap(name = "tf32")]
    Tf32,
    #[clap(name = "fp64")]
    Fp64,
    #[clap(name = "bf16")]
    Bf16,
}

impl Precision {
    pub fn as_str(&self) -> &'static str {
        match self {
            Precision::Fp32 => "FP32",
            Precision::Fp16 => "FP16",
            Precision::Tf32 => "TF32",
            Precision::Fp64 => "FP64",
            Precision::Bf16 => "BF16",
        }
    }

    pub fn is_deterministic(&self) -> bool {
        matches!(self, Precision::Fp32 | Precision::Fp16 | Precision::Fp64 | Precision::Bf16)
    }
}

pub struct CublasContext {
    pub ctx: Arc<CudaContext>,
    pub stream: Arc<CudaStream>,
    pub blas: CudaBlas,
}

impl CublasContext {
    pub fn new(device_index: u32) -> Result<Self> {
        unsafe {
            let res = cuda_sys::cuInit(0);
            if res as u32 != 0 {
                anyhow::bail!("cuInit failed: error code {}", res as u32);
            }
        }
        let ctx = CudaContext::new(device_index as usize)
            .context("failed to create CUDA context")?;
        let stream = ctx.default_stream();
        let blas = CudaBlas::new(stream.clone())
            .context("failed to create cuBLAS handle")?;
        Ok(Self { ctx, stream, blas })
    }

    pub fn synchronize(&self) -> Result<()> {
        self.stream
            .synchronize()
            .context("stream sync failed")?;
        Ok(())
    }
}

pub struct PinnedHost<T> {
    ptr: NonNull<T>,
    len: usize,
}

unsafe impl<T: Send> Send for PinnedHost<T> {}
unsafe impl<T: Send> Sync for PinnedHost<T> {}

impl<T> PinnedHost<T> {
    pub fn new(count: usize) -> Result<Self> {
        let mut ptr: *mut T = std::ptr::null_mut();
        let bytes = count * std::mem::size_of::<T>();
        let status = unsafe {
            cuda_sys::cuMemAllocHost_v2(
                (&mut ptr as *mut *mut T) as *mut *mut std::ffi::c_void,
                bytes,
            )
        };
        if status != cuda_sys::CUresult::CUDA_SUCCESS {
            anyhow::bail!("cuMemAllocHost failed: status={}", status as u32);
        }
        Ok(Self {
            ptr: NonNull::new(ptr).ok_or_else(|| anyhow::anyhow!("cuMemAllocHost returned null"))?,
            len: count,
        })
    }

    pub fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr()
    }

    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr.as_ptr()
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

impl PinnedHost<f32> {
    pub unsafe fn as_slice(&self) -> &[f32] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl PinnedHost<f64> {
    pub unsafe fn as_slice(&self) -> &[f64] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl<T> Drop for PinnedHost<T> {
    fn drop(&mut self) {
        unsafe {
            cuda_sys::cuMemFreeHost(self.ptr.as_ptr() as *mut std::ffi::c_void);
        }
    }
}

pub struct TrialDeviceBuffers {
    pub d_a: Option<cudarc::driver::CudaSlice<f32>>,
    pub d_a_half: Option<cudarc::driver::CudaSlice<f16>>,
    pub d_b: Option<cudarc::driver::CudaSlice<f32>>,
    pub d_b_half: Option<cudarc::driver::CudaSlice<f16>>,
    pub d_c: Option<cudarc::driver::CudaSlice<f32>>,
    pub d_c_half: Option<cudarc::driver::CudaSlice<f16>>,
    pub d_a_f64: Option<cudarc::driver::CudaSlice<f64>>,
    pub d_b_f64: Option<cudarc::driver::CudaSlice<f64>>,
    pub d_c_f64: Option<cudarc::driver::CudaSlice<f64>>,
    pub d_a_bf16: Option<cudarc::driver::CudaSlice<bf16>>,
    pub d_b_bf16: Option<cudarc::driver::CudaSlice<bf16>>,
    pub d_c_bf16: Option<cudarc::driver::CudaSlice<bf16>>,
}

pub struct TrialReusableState {
    pub pinned_c: PinnedHost<f32>,
    pub pinned_c_f64: Option<PinnedHost<f64>>,
    pub pinned_c_ext: Option<PinnedHost<f32>>,
    pub pinned_c_f64_ext: Option<PinnedHost<f64>>,
    pub device_bufs: TrialDeviceBuffers,
}

pub fn run_trial_gemm_fast(
    cu: &CublasContext,
    precision: Precision,
    n: usize,
    state: &mut TrialReusableState,
    extended: bool,
) -> Result<(f64, usize)> {
    use std::time::Instant;

    let t_start = Instant::now();
    let raw_stream = cu.stream.cu_stream();

    match precision {
        Precision::Fp32 | Precision::Tf32 => {
            if extended {
                let d_c = state.device_bufs.d_c.as_mut().expect("d_c");
                let pinned = state.pinned_c_ext.as_mut().expect("pinned_c_ext");
                cu.stream.memset_zeros(d_c).context("memset C_f")?;
                gemm_extended_fp32_device(&cu.blas, &cu.stream, n,
                    state.device_bufs.d_a.as_ref().expect("d_a"),
                    state.device_bufs.d_b.as_ref().expect("d_b"), d_c)?;
                cu.synchronize()?;
                let (d_ptr, _) = d_c.device_ptr(&cu.stream);
                chk(unsafe { cuda_sys::cuMemcpyDtoHAsync_v2(
                    pinned.as_mut_ptr() as _, d_ptr, ((n+1)*(n+1)*4) as _, raw_stream) })?;
                cu.synchronize()?;
                Ok((t_start.elapsed().as_secs_f64(), (n + 1) * (n + 1)))
            } else {
                let d_c = state.device_bufs.d_c.as_mut().expect("d_c");
                cu.stream.memset_zeros(d_c).context("memset C")?;
                gemm_fp32_device(&cu.blas, &cu.stream, n,
                    state.device_bufs.d_a.as_ref().expect("d_a"),
                    state.device_bufs.d_b.as_ref().expect("d_b"), d_c)?;
                cu.synchronize()?;
                let (d_ptr, _) = d_c.device_ptr(&cu.stream);
                chk(unsafe { cuda_sys::cuMemcpyDtoHAsync_v2(
                    state.pinned_c.as_mut_ptr() as _, d_ptr, (n*n*4) as _, raw_stream) })?;
                cu.synchronize()?;
                Ok((t_start.elapsed().as_secs_f64(), n * n))
            }
        }
        Precision::Fp16 => {
            if extended {
                let d_c = state.device_bufs.d_c_half.as_mut().expect("d_c_half");
                let pinned = state.pinned_c_ext.as_mut().expect("pinned_c_ext");
                cu.stream.memset_zeros(d_c).context("memset C_f")?;
                gemm_extended_fp16_device(&cu.blas, &cu.stream, n,
                    state.device_bufs.d_a_half.as_ref().expect("d_a_half"),
                    state.device_bufs.d_b_half.as_ref().expect("d_b_half"), d_c)?;
                cu.synchronize()?;
                let (d_ptr, _) = d_c.device_ptr(&cu.stream);
                chk(unsafe { cuda_sys::cuMemcpyDtoHAsync_v2(
                    pinned.as_mut_ptr() as _, d_ptr, ((n+1)*(n+1)*2) as _, raw_stream) })?;
                cu.synchronize()?;
                Ok((t_start.elapsed().as_secs_f64(), (n + 1) * (n + 1)))
            } else {
                let d_c = state.device_bufs.d_c_half.as_mut().expect("d_c_half");
                cu.stream.memset_zeros(d_c).context("memset C")?;
                gemm_fp16_device(&cu.blas, &cu.stream, n,
                    state.device_bufs.d_a_half.as_ref().expect("d_a_half"),
                    state.device_bufs.d_b_half.as_ref().expect("d_b_half"), d_c)?;
                cu.synchronize()?;
                let (d_ptr, _) = d_c.device_ptr(&cu.stream);
                chk(unsafe { cuda_sys::cuMemcpyDtoHAsync_v2(
                    state.pinned_c.as_mut_ptr() as _, d_ptr, (n*n*2) as _, raw_stream) })?;
                cu.synchronize()?;
                Ok((t_start.elapsed().as_secs_f64(), n * n))
            }
        }
        Precision::Fp64 => {
            if extended {
                let d_c = state.device_bufs.d_c_f64.as_mut().expect("d_c_f64");
                let pinned = state.pinned_c_f64_ext.as_mut().expect("pinned_c_f64_ext");
                cu.stream.memset_zeros(d_c).context("memset C_f")?;
                gemm_extended_fp64_device(&cu.blas, &cu.stream, n,
                    state.device_bufs.d_a_f64.as_ref().expect("d_a_f64"),
                    state.device_bufs.d_b_f64.as_ref().expect("d_b_f64"), d_c)?;
                cu.synchronize()?;
                let (d_ptr, _) = d_c.device_ptr(&cu.stream);
                chk(unsafe { cuda_sys::cuMemcpyDtoHAsync_v2(
                    pinned.as_mut_ptr() as _, d_ptr, ((n+1)*(n+1)*8) as _, raw_stream) })?;
                cu.synchronize()?;
                Ok((t_start.elapsed().as_secs_f64(), (n + 1) * (n + 1)))
            } else {
                let d_c = state.device_bufs.d_c_f64.as_mut().expect("d_c_f64");
                cu.stream.memset_zeros(d_c).context("memset C")?;
                gemm_fp64_device(&cu.blas, &cu.stream, n,
                    state.device_bufs.d_a_f64.as_ref().expect("d_a_f64"),
                    state.device_bufs.d_b_f64.as_ref().expect("d_b_f64"), d_c)?;
                cu.synchronize()?;
                let (d_ptr, _) = d_c.device_ptr(&cu.stream);
                chk(unsafe { cuda_sys::cuMemcpyDtoHAsync_v2(
                    state.pinned_c_f64.as_mut().expect("pinned_c_f64").as_mut_ptr() as _,
                    d_ptr, (n*n*8) as _, raw_stream) })?;
                cu.synchronize()?;
                Ok((t_start.elapsed().as_secs_f64(), n * n))
            }
        }
        Precision::Bf16 => {
            if extended {
                let d_c = state.device_bufs.d_c_bf16.as_mut().expect("d_c_bf16");
                let pinned = state.pinned_c_ext.as_mut().expect("pinned_c_ext");
                cu.stream.memset_zeros(d_c).context("memset C_f")?;
                gemm_extended_bf16_device(&cu.blas, &cu.stream, n,
                    state.device_bufs.d_a_bf16.as_ref().expect("d_a_bf16"),
                    state.device_bufs.d_b_bf16.as_ref().expect("d_b_bf16"), d_c)?;
                cu.synchronize()?;
                let (d_ptr, _) = d_c.device_ptr(&cu.stream);
                chk(unsafe { cuda_sys::cuMemcpyDtoHAsync_v2(
                    pinned.as_mut_ptr() as _, d_ptr, ((n+1)*(n+1)*2) as _, raw_stream) })?;
                cu.synchronize()?;
                Ok((t_start.elapsed().as_secs_f64(), (n + 1) * (n + 1)))
            } else {
                let d_c = state.device_bufs.d_c_bf16.as_mut().expect("d_c_bf16");
                cu.stream.memset_zeros(d_c).context("memset C")?;
                gemm_bf16_device(&cu.blas, &cu.stream, n,
                    state.device_bufs.d_a_bf16.as_ref().expect("d_a_bf16"),
                    state.device_bufs.d_b_bf16.as_ref().expect("d_b_bf16"), d_c)?;
                cu.synchronize()?;
                let (d_ptr, _) = d_c.device_ptr(&cu.stream);
                chk(unsafe { cuda_sys::cuMemcpyDtoHAsync_v2(
                    state.pinned_c.as_mut_ptr() as _, d_ptr, (n*n*2) as _, raw_stream) })?;
                cu.synchronize()?;
                Ok((t_start.elapsed().as_secs_f64(), n * n))
            }
        }
    }
}

fn chk(status: cuda_sys::CUresult) -> Result<()> {
    if status != cuda_sys::CUresult::CUDA_SUCCESS {
        anyhow::bail!("CUDA error: status={}", status as u32);
    }
    Ok(())
}

pub fn read_pinned_result(
    pinned: &PinnedHost<f32>,
    n: usize,
    precision: Precision,
) -> Vec<f32> {
    match precision {
        Precision::Fp16 | Precision::Bf16 => {
            let half_slice: &[u16] =
                unsafe { std::slice::from_raw_parts(pinned.as_ptr() as *const u16, n) };
            half_slice.iter().map(|&v| f16::from_bits(v).to_f32()).collect()
        }
        _ => {
            let slice = unsafe { pinned.as_slice() };
            slice[..n].to_vec()
        }
    }
}

pub fn read_pinned_f64(pinned: &PinnedHost<f64>, n: usize) -> Vec<f32> {
    let slice = unsafe { pinned.as_slice() };
    slice[..n].iter().map(|&v| v as f32).collect()
}

pub fn init_cublas_deterministic(blas: &CudaBlas) -> Result<()> {
    let status = unsafe {
        cublas_sys::cublasSetMathMode(*blas.handle(), cublas_sys::cublasMath_t::CUBLAS_PEDANTIC_MATH)
    };
    if status != cublas_sys::cublasStatus_t::CUBLAS_STATUS_SUCCESS {
        anyhow::bail!("cublasSetMathMode(CUBLAS_PEDANTIC_MATH) failed: {:?}", status);
    }
    let status = unsafe {
        cublas_sys::cublasSetAtomicsMode(
            *blas.handle(),
            cublas_sys::cublasAtomicsMode_t::CUBLAS_ATOMICS_NOT_ALLOWED,
        )
    };
    if status != cublas_sys::cublasStatus_t::CUBLAS_STATUS_SUCCESS {
        anyhow::bail!("cublasSetAtomicsMode(CUBLAS_ATOMICS_NOT_ALLOWED) failed: {:?}", status);
    }
    eprintln!("[init] cuBLAS set to deterministic mode (PEDANTIC_MATH + ATOMICS_NOT_ALLOWED)");
    Ok(())
}

pub fn init_cublas_tf32(blas: &CudaBlas) -> Result<()> {
    let status = unsafe {
        cublas_sys::cublasSetMathMode(*blas.handle(), cublas_sys::cublasMath_t::CUBLAS_TF32_TENSOR_OP_MATH)
    };
    if status != cublas_sys::cublasStatus_t::CUBLAS_STATUS_SUCCESS {
        anyhow::bail!("cublasSetMathMode(CUBLAS_TF32_TENSOR_OP_MATH) failed: {:?}", status);
    }
    eprintln!("[init] cuBLAS set to TF32 mode (TF32_TENSOR_OP_MATH)");
    Ok(())
}

pub fn gemm_fp32_device(
    blas: &CudaBlas, _stream: &Arc<CudaStream>, n: usize,
    d_a: &cudarc::driver::CudaSlice<f32>, d_b: &cudarc::driver::CudaSlice<f32>,
    d_c: &mut cudarc::driver::CudaSlice<f32>,
) -> Result<()> {
    let cfg = GemmConfig {
        transa: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        transb: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        m: n as i32, n: n as i32, k: n as i32,
        alpha: 1.0f32, lda: n as i32, ldb: n as i32, beta: 0.0f32, ldc: n as i32,
    };
    unsafe { blas.gemm(cfg, d_a, d_b, d_c).context("cublasSgemm failed")?; }
    Ok(())
}

pub fn gemm_fp16_device(
    blas: &CudaBlas, _stream: &Arc<CudaStream>, n: usize,
    d_a: &cudarc::driver::CudaSlice<f16>, d_b: &cudarc::driver::CudaSlice<f16>,
    d_c: &mut cudarc::driver::CudaSlice<f16>,
) -> Result<()> {
    let cfg = GemmConfig {
        transa: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        transb: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        m: n as i32, n: n as i32, k: n as i32,
        alpha: f16::from_f32(1.0), lda: n as i32, ldb: n as i32,
        beta: f16::from_f32(0.0), ldc: n as i32,
    };
    unsafe { blas.gemm(cfg, d_a, d_b, d_c).context("cublasHgemm failed")?; }
    Ok(())
}

pub fn gemm_extended_fp32_device(
    blas: &CudaBlas, _stream: &Arc<CudaStream>, n: usize,
    d_a_c: &cudarc::driver::CudaSlice<f32>, d_b_r: &cudarc::driver::CudaSlice<f32>,
    d_c_f: &mut cudarc::driver::CudaSlice<f32>,
) -> Result<()> {
    let m = (n + 1) as i32; let k = n as i32;
    let cfg = GemmConfig {
        transa: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        transb: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        m, n: m, k, alpha: 1.0f32, lda: n as i32, ldb: (n + 1) as i32,
        beta: 0.0f32, ldc: (n + 1) as i32,
    };
    unsafe { blas.gemm(cfg, d_a_c, d_b_r, d_c_f).context("cublasSgemm (extended) failed")?; }
    Ok(())
}

pub fn gemm_extended_fp16_device(
    blas: &CudaBlas, _stream: &Arc<CudaStream>, n: usize,
    d_a_c: &cudarc::driver::CudaSlice<f16>, d_b_r: &cudarc::driver::CudaSlice<f16>,
    d_c_f: &mut cudarc::driver::CudaSlice<f16>,
) -> Result<()> {
    let m = (n + 1) as i32; let k = n as i32;
    let cfg = GemmConfig {
        transa: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        transb: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        m, n: m, k, alpha: f16::from_f32(1.0),
        lda: n as i32, ldb: (n + 1) as i32,
        beta: f16::from_f32(0.0), ldc: (n + 1) as i32,
    };
    unsafe { blas.gemm(cfg, d_a_c, d_b_r, d_c_f).context("cublasHgemm (extended) failed")?; }
    Ok(())
}

pub fn gemm_fp64_device(
    blas: &CudaBlas, _stream: &Arc<CudaStream>, n: usize,
    d_a: &cudarc::driver::CudaSlice<f64>, d_b: &cudarc::driver::CudaSlice<f64>,
    d_c: &mut cudarc::driver::CudaSlice<f64>,
) -> Result<()> {
    let cfg = GemmConfig {
        transa: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        transb: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        m: n as i32, n: n as i32, k: n as i32,
        alpha: 1.0f64, lda: n as i32, ldb: n as i32, beta: 0.0f64, ldc: n as i32,
    };
    unsafe { blas.gemm(cfg, d_a, d_b, d_c).context("cublasDgemm failed")?; }
    Ok(())
}

pub fn gemm_bf16_device(
    blas: &CudaBlas, _stream: &Arc<CudaStream>, n: usize,
    d_a: &cudarc::driver::CudaSlice<bf16>, d_b: &cudarc::driver::CudaSlice<bf16>,
    d_c: &mut cudarc::driver::CudaSlice<bf16>,
) -> Result<()> {
    let cfg = GemmConfig {
        transa: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        transb: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        m: n as i32, n: n as i32, k: n as i32,
        alpha: bf16::from_f32(1.0), lda: n as i32, ldb: n as i32,
        beta: bf16::from_f32(0.0), ldc: n as i32,
    };
    unsafe { blas.gemm(cfg, d_a, d_b, d_c).context("cublasGemmEx(BF16) failed")?; }
    Ok(())
}

pub fn gemm_extended_fp64_device(
    blas: &CudaBlas, _stream: &Arc<CudaStream>, n: usize,
    d_a_c: &cudarc::driver::CudaSlice<f64>, d_b_r: &cudarc::driver::CudaSlice<f64>,
    d_c_f: &mut cudarc::driver::CudaSlice<f64>,
) -> Result<()> {
    let m = (n + 1) as i32; let k = n as i32;
    let cfg = GemmConfig {
        transa: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        transb: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        m, n: m, k, alpha: 1.0f64, lda: n as i32, ldb: (n + 1) as i32,
        beta: 0.0f64, ldc: (n + 1) as i32,
    };
    unsafe { blas.gemm(cfg, d_a_c, d_b_r, d_c_f).context("cublasDgemm (extended) failed")?; }
    Ok(())
}

pub fn gemm_extended_bf16_device(
    blas: &CudaBlas, _stream: &Arc<CudaStream>, n: usize,
    d_a_c: &cudarc::driver::CudaSlice<bf16>, d_b_r: &cudarc::driver::CudaSlice<bf16>,
    d_c_f: &mut cudarc::driver::CudaSlice<bf16>,
) -> Result<()> {
    let m = (n + 1) as i32; let k = n as i32;
    let cfg = GemmConfig {
        transa: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        transb: cublas_sys::cublasOperation_t::CUBLAS_OP_N,
        m, n: m, k, alpha: bf16::from_f32(1.0),
        lda: n as i32, ldb: (n + 1) as i32,
        beta: bf16::from_f32(0.0), ldc: (n + 1) as i32,
    };
    unsafe { blas.gemm(cfg, d_a_c, d_b_r, d_c_f).context("cublasGemmEx(BF16 extended) failed")?; }
    Ok(())
}

fn download_f32(stream: &Arc<CudaStream>, d: &cudarc::driver::CudaSlice<f32>) -> Result<Vec<f32>> {
    let host = stream.clone_dtoh(d).context("device to host copy failed")?;
    stream.synchronize().context("sync after dtoh failed")?;
    Ok(host)
}

fn download_f16(stream: &Arc<CudaStream>, d: &cudarc::driver::CudaSlice<f16>) -> Result<Vec<f32>> {
    let host: Vec<f16> = stream.clone_dtoh(d).context("device to host copy failed")?;
    stream.synchronize().context("sync after dtoh failed")?;
    Ok(host.into_iter().map(|v| v.to_f32()).collect())
}

fn download_f64(stream: &Arc<CudaStream>, d: &cudarc::driver::CudaSlice<f64>) -> Result<Vec<f32>> {
    let host: Vec<f64> = stream.clone_dtoh(d).context("device to host copy failed")?;
    stream.synchronize().context("sync after dtoh failed")?;
    Ok(host.into_iter().map(|v| v as f32).collect())
}

fn download_bf16(stream: &Arc<CudaStream>, d: &cudarc::driver::CudaSlice<bf16>) -> Result<Vec<f32>> {
    let host: Vec<bf16> = stream.clone_dtoh(d).context("device to host copy failed")?;
    stream.synchronize().context("sync after dtoh failed")?;
    Ok(host.into_iter().map(|v| v.to_f32()).collect())
}

pub fn preflight_reproducibility_check(
    cu: &CublasContext, precision: Precision, n: usize,
    a_host: &[f32], b_host: &[f32],
) -> Result<()> {
    let mut results: Vec<Vec<f32>> = Vec::new();
    for _run in 0..3 {
        let c_host = match precision {
            Precision::Fp32 | Precision::Tf32 => {
                let d_a = cu.stream.clone_htod(a_host).context("upload A failed")?;
                let d_b = cu.stream.clone_htod(b_host).context("upload B failed")?;
                let mut d_c = cu.stream.alloc_zeros::<f32>(n * n).context("alloc C failed")?;
                gemm_fp32_device(&cu.blas, &cu.stream, n, &d_a, &d_b, &mut d_c)?;
                cu.synchronize()?;
                download_f32(&cu.stream, &d_c)?
            }
            Precision::Fp16 => {
                let a_half: Vec<f16> = a_host.iter().map(|v| f16::from_f32(*v)).collect();
                let b_half: Vec<f16> = b_host.iter().map(|v| f16::from_f32(*v)).collect();
                let d_a = cu.stream.clone_htod(&a_half).context("upload A(half) failed")?;
                let d_b = cu.stream.clone_htod(&b_half).context("upload B(half) failed")?;
                let mut d_c = cu.stream.alloc_zeros::<f16>(n * n).context("alloc C(half) failed")?;
                gemm_fp16_device(&cu.blas, &cu.stream, n, &d_a, &d_b, &mut d_c)?;
                cu.synchronize()?;
                download_f16(&cu.stream, &d_c)?
            }
            Precision::Fp64 => {
                let a_f64: Vec<f64> = a_host.iter().map(|&v| v as f64).collect();
                let b_f64: Vec<f64> = b_host.iter().map(|&v| v as f64).collect();
                let d_a = cu.stream.clone_htod(&a_f64).context("upload A(f64) failed")?;
                let d_b = cu.stream.clone_htod(&b_f64).context("upload B(f64) failed")?;
                let mut d_c = cu.stream.alloc_zeros::<f64>(n * n).context("alloc C(f64) failed")?;
                gemm_fp64_device(&cu.blas, &cu.stream, n, &d_a, &d_b, &mut d_c)?;
                cu.synchronize()?;
                download_f64(&cu.stream, &d_c)?
            }
            Precision::Bf16 => {
                let a_bf16: Vec<bf16> = a_host.iter().map(|&v| bf16::from_f32(v)).collect();
                let b_bf16: Vec<bf16> = b_host.iter().map(|&v| bf16::from_f32(v)).collect();
                let d_a = cu.stream.clone_htod(&a_bf16).context("upload A(bf16) failed")?;
                let d_b = cu.stream.clone_htod(&b_bf16).context("upload B(bf16) failed")?;
                let mut d_c = cu.stream.alloc_zeros::<bf16>(n * n).context("alloc C(bf16) failed")?;
                gemm_bf16_device(&cu.blas, &cu.stream, n, &d_a, &d_b, &mut d_c)?;
                cu.synchronize()?;
                download_bf16(&cu.stream, &d_c)?
            }
        };
        results.push(c_host);
    }
    if precision.is_deterministic() {
        for i in 1..3 {
            for (j, (&a, &b)) in results[0].iter().zip(results[i].iter()).enumerate() {
                if a.to_bits() != b.to_bits() {
                    anyhow::bail!(
                        "Pre-flight FAILED: GEMM run 0 and run {} differ at element {} ({:.8e} vs {:.8e})",
                        i, j, a, b
                    );
                }
            }
        }
        eprintln!("[preflight] cuBLAS determinism verified (3/3 runs bit-identical) for {}", precision.as_str());
    } else {
        let mut max_variation = 0.0f32;
        for i in 1..3 {
            for (&a, &b) in results[0].iter().zip(results[i].iter()) {
                let diff = (a - b).abs();
                if diff > max_variation { max_variation = diff; }
            }
        }
        eprintln!("[preflight] TF32 mode: 3-run max variation = {:.6e}", max_variation);
    }
    Ok(())
}

pub fn run_trial_gemm(
    cu: &CublasContext, precision: Precision, n: usize,
    a_host: &[f32], b_host: &[f32],
    a_c_host: Option<&[f32]>, b_r_host: Option<&[f32]>,
    extended: bool,
) -> Result<(Vec<f32>, Option<Vec<f32>>, f64)> {
    use std::time::Instant;
    let t_start = Instant::now();
    match precision {
        Precision::Fp32 | Precision::Tf32 => {
            if extended {
                let a_c = a_c_host.expect("a_c required");
                let b_r = b_r_host.expect("b_r required");
                let d_a = cu.stream.clone_htod(a_c).context("upload A_c failed")?;
                let d_b = cu.stream.clone_htod(b_r).context("upload B_r failed")?;
                let mut d_c = cu.stream.alloc_zeros::<f32>((n+1)*(n+1)).context("alloc C_f failed")?;
                gemm_extended_fp32_device(&cu.blas, &cu.stream, n, &d_a, &d_b, &mut d_c)?;
                cu.synchronize()?;
                let c_host = download_f32(&cu.stream, &d_c)?;
                Ok((c_host, None, t_start.elapsed().as_secs_f64()))
            } else {
                let d_a = cu.stream.clone_htod(a_host).context("upload A failed")?;
                let d_b = cu.stream.clone_htod(b_host).context("upload B failed")?;
                let mut d_c = cu.stream.alloc_zeros::<f32>(n*n).context("alloc C failed")?;
                gemm_fp32_device(&cu.blas, &cu.stream, n, &d_a, &d_b, &mut d_c)?;
                cu.synchronize()?;
                let c_host = download_f32(&cu.stream, &d_c)?;
                Ok((c_host, None, t_start.elapsed().as_secs_f64()))
            }
        }
        Precision::Fp16 => {
            if extended {
                let a_c = a_c_host.expect("a_c required");
                let b_r = b_r_host.expect("b_r required");
                let a_c_half: Vec<f16> = a_c.iter().map(|v| f16::from_f32(*v)).collect();
                let b_r_half: Vec<f16> = b_r.iter().map(|v| f16::from_f32(*v)).collect();
                let d_a = cu.stream.clone_htod(&a_c_half).context("upload A_c(half) failed")?;
                let d_b = cu.stream.clone_htod(&b_r_half).context("upload B_r(half) failed")?;
                let mut d_c = cu.stream.alloc_zeros::<f16>((n+1)*(n+1)).context("alloc C_f(half) failed")?;
                gemm_extended_fp16_device(&cu.blas, &cu.stream, n, &d_a, &d_b, &mut d_c)?;
                cu.synchronize()?;
                let c_f_host = download_f16(&cu.stream, &d_c)?;
                Ok((c_f_host, None, t_start.elapsed().as_secs_f64()))
            } else {
                let a_half: Vec<f16> = a_host.iter().map(|v| f16::from_f32(*v)).collect();
                let b_half: Vec<f16> = b_host.iter().map(|v| f16::from_f32(*v)).collect();
                let d_a = cu.stream.clone_htod(&a_half).context("upload A(half) failed")?;
                let d_b = cu.stream.clone_htod(&b_half).context("upload B(half) failed")?;
                let mut d_c = cu.stream.alloc_zeros::<f16>(n*n).context("alloc C(half) failed")?;
                gemm_fp16_device(&cu.blas, &cu.stream, n, &d_a, &d_b, &mut d_c)?;
                cu.synchronize()?;
                let c_host = download_f16(&cu.stream, &d_c)?;
                Ok((c_host, None, t_start.elapsed().as_secs_f64()))
            }
        }
        Precision::Fp64 => {
            if extended {
                let a_c = a_c_host.expect("a_c required");
                let b_r = b_r_host.expect("b_r required");
                let a_c_f64: Vec<f64> = a_c.iter().map(|&v| v as f64).collect();
                let b_r_f64: Vec<f64> = b_r.iter().map(|&v| v as f64).collect();
                let d_a = cu.stream.clone_htod(&a_c_f64).context("upload A_c(f64) failed")?;
                let d_b = cu.stream.clone_htod(&b_r_f64).context("upload B_r(f64) failed")?;
                let mut d_c = cu.stream.alloc_zeros::<f64>((n+1)*(n+1)).context("alloc C_f(f64) failed")?;
                gemm_extended_fp64_device(&cu.blas, &cu.stream, n, &d_a, &d_b, &mut d_c)?;
                cu.synchronize()?;
                let c_f_host = download_f64(&cu.stream, &d_c)?;
                Ok((c_f_host, None, t_start.elapsed().as_secs_f64()))
            } else {
                let a_f64: Vec<f64> = a_host.iter().map(|&v| v as f64).collect();
                let b_f64: Vec<f64> = b_host.iter().map(|&v| v as f64).collect();
                let d_a = cu.stream.clone_htod(&a_f64).context("upload A(f64) failed")?;
                let d_b = cu.stream.clone_htod(&b_f64).context("upload B(f64) failed")?;
                let mut d_c = cu.stream.alloc_zeros::<f64>(n*n).context("alloc C(f64) failed")?;
                gemm_fp64_device(&cu.blas, &cu.stream, n, &d_a, &d_b, &mut d_c)?;
                cu.synchronize()?;
                let c_host = download_f64(&cu.stream, &d_c)?;
                Ok((c_host, None, t_start.elapsed().as_secs_f64()))
            }
        }
        Precision::Bf16 => {
            if extended {
                let a_c = a_c_host.expect("a_c required");
                let b_r = b_r_host.expect("b_r required");
                let a_c_bf16: Vec<bf16> = a_c.iter().map(|&v| bf16::from_f32(v)).collect();
                let b_r_bf16: Vec<bf16> = b_r.iter().map(|&v| bf16::from_f32(v)).collect();
                let d_a = cu.stream.clone_htod(&a_c_bf16).context("upload A_c(bf16) failed")?;
                let d_b = cu.stream.clone_htod(&b_r_bf16).context("upload B_r(bf16) failed")?;
                let mut d_c = cu.stream.alloc_zeros::<bf16>((n+1)*(n+1)).context("alloc C_f(bf16) failed")?;
                gemm_extended_bf16_device(&cu.blas, &cu.stream, n, &d_a, &d_b, &mut d_c)?;
                cu.synchronize()?;
                let c_f_host = download_bf16(&cu.stream, &d_c)?;
                Ok((c_f_host, None, t_start.elapsed().as_secs_f64()))
            } else {
                let a_bf16: Vec<bf16> = a_host.iter().map(|&v| bf16::from_f32(v)).collect();
                let b_bf16: Vec<bf16> = b_host.iter().map(|&v| bf16::from_f32(v)).collect();
                let d_a = cu.stream.clone_htod(&a_bf16).context("upload A(bf16) failed")?;
                let d_b = cu.stream.clone_htod(&b_bf16).context("upload B(bf16) failed")?;
                let mut d_c = cu.stream.alloc_zeros::<bf16>(n*n).context("alloc C(bf16) failed")?;
                gemm_bf16_device(&cu.blas, &cu.stream, n, &d_a, &d_b, &mut d_c)?;
                cu.synchronize()?;
                let c_host = download_bf16(&cu.stream, &d_c)?;
                Ok((c_host, None, t_start.elapsed().as_secs_f64()))
            }
        }
    }
}
