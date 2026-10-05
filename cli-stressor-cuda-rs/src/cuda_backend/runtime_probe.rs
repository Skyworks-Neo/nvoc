//! Non-panicking probes for the CUDA shared libraries cudarc loads at runtime.
//!
//! On cudarc's `dynamic-loading` path every library accessor panics
//! (`panic_no_lib_found`) when a library cannot be loaded. The generated
//! `is_culib_present()` helpers run the exact same candidate search but return
//! `false` instead, so a missing runtime can be reported as a clean error
//! before any panicking accessor runs.

use std::path::{Path, PathBuf};

/// Availability of the CUDA libraries the stress backend touches, probed
/// against the process-visible DLL search path.
#[derive(Debug, Clone, Copy)]
pub struct CudaRuntimeLibs {
    /// Driver library (`nvcuda.dll`) — required for enumeration and context
    /// creation.
    pub driver: bool,
    /// cuBLAS — required; every GEMM path constructs a `CudaBlas` handle.
    pub cublas: bool,
    /// NVRTC (plus its builtins at compile time) — optional; gates the
    /// NVRTC-compiled kernels (atomic, INT ALU, verify engine, gpu fill).
    pub nvrtc: bool,
}

/// DLL name the cuBLAS error text should point at, per build generation.
#[cfg(feature = "cuda12")]
const CUBLAS_DLL: &str = "cublas64_12.dll";
#[cfg(feature = "cuda11")]
const CUBLAS_DLL: &str = "cublas64_11.dll";

const DRIVER_MISSING: &str = "CUDA driver library not found (nvcuda.dll) - install or update the \
                              NVIDIA display driver";

impl CudaRuntimeLibs {
    /// Probe the runtime libraries. Non-panicking (unlike the cudarc
    /// accessors, which abort on a missing library).
    pub fn probe() -> Self {
        unsafe {
            Self {
                driver: cudarc::driver::sys::is_culib_present(),
                cublas: cudarc::cublas::sys::is_culib_present(),
                nvrtc: cudarc::nvrtc::sys::is_culib_present(),
            }
        }
    }

    /// `Err` when the driver library is missing (device enumeration works but
    /// nothing else does).
    pub fn require_driver(&self) -> Result<(), String> {
        if self.driver {
            Ok(())
        } else {
            Err(DRIVER_MISSING.into())
        }
    }

    /// `Err` naming the first library the CUDA stress run cannot work
    /// without (driver, then cuBLAS).
    pub fn require_stress_libs(&self) -> Result<(), String> {
        self.require_driver()?;
        if !self.cublas {
            return Err(format!(
                "cuBLAS runtime library not found ({CUBLAS_DLL}) - install the CUDA runtime, \
                 point --cuda-path at the directory holding it, or place the runtime DLLs \
                 next to this executable"
            ));
        }
        Ok(())
    }

    /// Preload the CUDA runtime libraries found in `dir` so that later
    /// name-only loads — cudarc's accessors and NVRTC's builtins lookup —
    /// resolve against them (no DLLs need to sit next to the executable).
    ///
    /// `dir` may be the library directory itself or a CUDA toolkit root; its
    /// `bin/`, `lib64/`, `lib/` and Debian multiarch subdirectories are probed
    /// in that order. Handles are intentionally kept for the process lifetime
    /// (unloading would defeat the point). Returns the directory the libraries
    /// were actually loaded from plus the loaded file names.
    pub fn preload_from_dir(dir: &Path) -> Result<(PathBuf, Vec<String>), String> {
        let Some((root, files)) = probe_library_dir(dir) else {
            return Err(format!(
                "no CUDA runtime libraries found under {} (also probed its bin/, lib64/, \
                 lib/ subdirectories)",
                dir.display()
            ));
        };
        let mut pending: Vec<PathBuf> = files.into_iter().map(|(_, path)| path).collect();
        let total = pending.len();
        let mut loaded: Vec<String> = Vec::new();
        // Two passes: a library whose dependencies live in the same directory
        // may only load once those are resident.
        for pass in 0..2 {
            let mut retry: Vec<PathBuf> = Vec::new();
            for path in std::mem::take(&mut pending) {
                match load_keepalive(&path) {
                    Ok(()) => loaded.push(
                        path.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    ),
                    Err(err) => {
                        if pass == 0 {
                            retry.push(path);
                        } else {
                            eprintln!(
                                "Warning: --cuda-path: failed to preload {}: {}",
                                path.display(),
                                err
                            );
                        }
                    }
                }
            }
            pending = retry;
            if pending.is_empty() {
                break;
            }
        }
        if loaded.is_empty() {
            return Err(format!(
                "--cuda-path {}: none of the {} CUDA libraries there could be loaded",
                root.display(),
                total
            ));
        }
        Ok((root, loaded))
    }
}

/// Rank of a file that should be preloaded, in dependency order (cudart
/// first, NVRTC last). `None` for anything that is not a CUDA runtime library
/// this binary uses.
fn cuda_library_rank(name: &str) -> Option<u8> {
    let lower = name.to_ascii_lowercase();
    if !(lower.ends_with(".dll") || lower.contains(".so")) {
        return None;
    }
    if lower.contains("cudart") {
        Some(0)
    } else if lower.contains("cublaslt") {
        Some(1)
    } else if lower.contains("cublas") {
        Some(2)
    } else if lower.contains("nvrtc-builtins") {
        Some(3)
    } else if lower.contains("nvrtc") {
        Some(4)
    } else {
        None
    }
}

/// Locate the directory that actually holds the CUDA libraries and list its
/// candidates, ranked for loading. `None` when the path and its toolkit-style
/// subdirectories are all empty of candidates.
fn probe_library_dir(dir: &Path) -> Option<(PathBuf, Vec<(u8, PathBuf)>)> {
    let mut roots: Vec<PathBuf> = vec![dir.to_path_buf()];
    for sub in ["bin", "lib64", "lib", "targets/x86_64-linux/lib"] {
        roots.push(dir.join(sub));
    }
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut found: Vec<(u8, PathBuf)> = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_name()?.to_string_lossy();
                cuda_library_rank(&name).map(|rank| (rank, path))
            })
            .collect();
        if !found.is_empty() {
            found.sort_by_key(|(rank, _)| *rank);
            return Some((root, found));
        }
    }
    None
}

/// Load a library so its module stays resident for the process lifetime.
#[cfg(windows)]
fn load_keepalive(path: &Path) -> Result<(), String> {
    use libloading::os::windows::{LOAD_WITH_ALTERED_SEARCH_PATH, Library};
    // Altered search path lets the DLL resolve ITS dependencies (e.g. cublasLt
    // for cuBLAS) from the same directory instead of the default search order.
    let library =
        unsafe { Library::load_with_flags(path.to_path_buf(), LOAD_WITH_ALTERED_SEARCH_PATH) }
            .map_err(|err| err.to_string())?;
    // Deliberate leak: the loader must keep the module alive for the whole run.
    std::mem::forget(library);
    Ok(())
}

/// Load a library so its module stays resident for the process lifetime.
#[cfg(unix)]
fn load_keepalive(path: &Path) -> Result<(), String> {
    // dlopen with an absolute path: the loaded object's SONAME then satisfies
    // the name-only dlopen calls cudarc makes later on.
    let library =
        unsafe { libloading::Library::new(path.to_path_buf()) }.map_err(|err| err.to_string())?;
    // Deliberate leak: the loader must keep the mapped object alive for the
    // whole run (unloading would also undo the SONAME registration).
    std::mem::forget(library);
    Ok(())
}
