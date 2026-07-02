//! Shared NVRTC / module-loading helpers for the custom CUDA stress kernels.

use std::sync::Arc;

use cudarc::driver::{CudaContext, CudaFunction, CudaModule};
use cudarc::nvrtc::Ptx;

use cli_stressor_cuda_rs::BackendError;

/// JIT-compile `ptx` (a compiled PTX image) into a module on `ctx` and look up
/// the entry point `name`. The returned `Arc<CudaModule>` must be held alive for
/// as long as the `CudaFunction` is used.
pub(super) fn load_kernel(
    ctx: &Arc<CudaContext>,
    ptx: Ptx,
    name: &str,
) -> Result<(Arc<CudaModule>, CudaFunction), BackendError> {
    let module = ctx
        .load_module(ptx)
        .map_err(|err| BackendError::Other(err.to_string()))?;
    let func = module
        .load_function(name)
        .map_err(|err| BackendError::Other(err.to_string()))?;
    Ok((module, func))
}
