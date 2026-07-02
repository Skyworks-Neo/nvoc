//! CUDA backend for the stress tester.
//!
//! This module root only re-exports the public API; the implementation is split
//! by workload across the `cuda_backend/` folder:
//!
//! - [`backend`]: `CudaBackend` struct, constructor, and the `Backend` trait
//!   impl (the dispatcher that fans out to the workload modules).
//! - [`lanes`]: multi-stream lane selection (`stream_for_lane` / `blas_for_lane`).
//! - [`gemm`]: cuBLAS GEMM stress path (FP precisions).
//! - [`mem_ops`]: memcpy / memset / sgeam (transpose & elementwise) / reduction.
//! - [`atomic`]: atomic-operation stress kernel (custom NVRTC).
//! - [`kernels`]: shared NVRTC module-loading helpers.
//! - [`device`]: device enumeration and selection (UUID / PCI / sorted index).

mod atomic;
mod backend;
mod device;
mod gemm;
mod int_alu;
mod kernels;
mod lanes;
mod mem_ops;

// `CudaMatrix` / `CudaOutput` are the `Backend::Matrix` / `Backend::Output`
// associated types; re-exported as part of the backend's public surface even
// though nothing in this crate names them directly.
#[allow(unused_imports)]
pub use backend::{CudaBackend, CudaMatrix, CudaOutput};
#[cfg(feature = "vulkan")]
#[allow(unused_imports)]
pub use backend::CudaDeviceIdentity;

pub use device::{
    enumerate_cuda_devices, resolve_device_index_by_pci_bus,
    resolve_device_index_by_sorted_index, resolve_device_index_by_uuid,
};
