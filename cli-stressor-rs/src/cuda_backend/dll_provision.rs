//! CUDA runtime-library provisioning (`--cuda-dll-version` / `--cuda-dll-dir`
//! / `--cuda-dll-mirror`): resolve, download, verify, extract, and preload
//! the exact cublas/cublasLt/nvrtc/cudart binaries a run should use.
//!
//! Why: FP8/FP4 kernel availability is a property of the loaded
//! cublasLt64_*.dll, and Windows' DLL search order lets a stray years-old
//! copy (exe directory, PATH toolkit) shadow the driver/toolkit one —
//! observed in practice (cublasLt 12.0.2 shadowing a CUDA 13.5 driver;
//! cublasLt 12.8.4 shipping zero E5M2 candidates on Ada). Explicit version
//! selection + preloading by full path makes the library choice
//! deterministic and matrix-recordable.
//!
//! Channel: NVIDIA's official PyPI wheels (`nvidia-cublas-cu12/13`,
//! `nvidia-cuda-nvrtc-cu12/13`, `nvidia-cuda-runtime-cu12/13`) resolved via
//! the PEP 503 simple index (mirror-aware), sha256-verified against the
//! index fragments, extracted to a per-version cache (only the binaries the
//! stressor needs), and preloaded with `LoadLibraryExW(LOAD_WITH_ALTERED_
//! SEARCH_PATH)` before cudarc's dynamic loads — the preloaded module wins
//! cudarc's name-based lookup regardless of the system search order.

use std::path::{Path, PathBuf};

use cli_stressor_rs::BackendError;

/// User-facing provisioning configuration (`--cuda-dll-*` flags).
#[derive(Debug, Clone, Default)]
pub struct CudaDllProvision {
    /// `--cuda-dll-version <V>`: "12.8" (latest 12.8.*) or an exact wheel
    /// version ("12.8.4.1"). The major must match the build's CUDA feature.
    pub version: Option<String>,
    /// `--cuda-dll-mirror tuna|aliyun|<index-base>`: PyPI simple-index mirror
    /// (mainland-China friendly). Default: pypi.org with automatic file
    /// fallback to TUNA/Aliyun.
    pub mirror: Option<String>,
    /// `--cuda-dll-dir <DIR>`: skip download, preload DLLs from this directory.
    pub dir: Option<PathBuf>,
}

/// Builtin simple-index bases for the fallback chain (the file-download
/// fallback rewrites `files.pythonhosted.org/packages/<suffix>` onto these).
const MIRRORS: &[(&str, &str)] = &[
    ("tuna", "https://pypi.tuna.tsinghua.edu.cn"),
    ("aliyun", "https://mirrors.aliyun.com/pypi"),
];

const PYPI_INDEX: &str = "https://pypi.org/simple";
const PYPI_FILES: &str = "https://files.pythonhosted.org/packages";

/// Packages the stressor needs, per CUDA major (PyPI project names carry the
/// major suffix).
fn packages_for_major(major: u32) -> [String; 3] {
    [
        format!("nvidia-cublas-cu{major}"),
        format!("nvidia-cuda-nvrtc-cu{major}"),
        format!("nvidia-cuda-runtime-cu{major}"),
    ]
}

/// Parse a version string into numeric segments ("12.8.4.1" → [12, 8, 4, 1]);
/// non-numeric junk segments are dropped so wheel epochs never panic us.
pub(crate) fn parse_version_segments(v: &str) -> Vec<u32> {
    v.split('.')
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .collect()
}

/// Pick the best wheel (filename, url, sha256) from a PEP 503 simple-index
/// HTML body: matches the requested version prefix, the wanted platform tag,
/// and the highest parsed version. Returns None when nothing matches.
pub(crate) fn pick_wheel(
    index_html: &str,
    version_prefix: &str,
    want_windows: bool,
) -> Option<(String, String, String)> {
    let prefix_segments = parse_version_segments(version_prefix);
    let mut best: Option<(Vec<u32>, String, String, String)> = None;
    let mut rest = index_html;
    while let Some(pos) = rest.find("<a href=\"") {
        let after = &rest[pos + 9..];
        let (url, url_rest) = match after.split_once('"') {
            Some(v) => v,
            None => break,
        };
        // The checksum rides the href as a PEP 503 "#sha256=..." fragment.
        let hash = url
            .split_once("#sha256=")
            .map(|(_, h)| h.to_string())
            .unwrap_or_default();
        // Anchor text runs from the tag's closing '>' (there may be extra
        // attributes like data-requires-python between href and '>') to
        // '</a>'.
        let text = match url_rest
            .split_once('>')
            .and_then(|(_, t)| t.split_once("</a>"))
        {
            Some((t, _)) => t.to_string(),
            None => break,
        };
        // Advance past this anchor entirely.
        match rest[pos..].find("</a>") {
            Some(end) => rest = &rest[pos + end + 4..],
            None => break,
        }
        if !text.ends_with(".whl") {
            continue;
        }
        let platform_ok = if want_windows {
            text.contains("win_amd64")
        } else {
            text.contains("manylinux") && text.contains("x86_64")
        };
        if !platform_ok {
            continue;
        }
        // Wheel filename layout: {pkg_normalized}-{version}-py3-none-{tags}
        // — the version is the second dash-separated field (trimming the
        // "-py3..." tail, which would otherwise corrupt the segment parse).
        let version = text.split('-').nth(1).unwrap_or_default().to_string();
        let segs = parse_version_segments(&version);
        if segs.len() < 2 || segs[0] != prefix_segments[0] {
            continue;
        }
        // Requested "12.8" accepts 12.8.*; requested "12.8.4.1" accepts only
        // that exact version.
        let prefix_ok = if prefix_segments.len() == 2 && version_prefix.split('.').count() == 2 {
            segs[0] == prefix_segments[0] && segs[1] == prefix_segments[1]
        } else {
            segs == prefix_segments
        };
        if !prefix_ok {
            continue;
        }
        let better = best
            .as_ref()
            .map(|(bsegs, _, _, _)| segs > *bsegs)
            .unwrap_or(true);
        if better {
            best = Some((segs, text, url.to_string(), hash));
        }
    }
    best.map(|(_, text, url, hash)| (text, url, hash))
}

/// The binary names the stressor needs from the wheels, per platform.
/// All lowercase: the caller lowercases filenames before the prefix compare
/// (cublasLt64_ would never match its own lowercased name otherwise — that
/// bug silently dropped the single most important DLL from the cache).
pub(crate) fn wanted_binary_prefixes(windows: bool) -> &'static [&'static str] {
    if windows {
        &[
            "cublas64_",
            "cublaslt64_",
            "cudart64_",
            "nvrtc64_",
            "nvrtc-builtins64_",
        ]
    } else {
        &[
            "libcublas.so.",
            "libcublaslt.so.",
            "libcudart.so.",
            "libnvrtc.so.",
            "libnvrtc-builtins.so.",
        ]
    }
}

fn curl_to_file(url: &str, out: &Path) -> Result<(), BackendError> {
    let status = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--connect-timeout",
            "15",
            "--retry",
            "2",
            "-o",
            out.to_string_lossy().as_ref(),
            url,
        ])
        .status()
        .map_err(|err| BackendError::Other(format!("curl launch failed: {err}")))?;
    if !status.success() {
        return Err(BackendError::Other(format!(
            "curl download failed (exit {status:?}) for {url}"
        )));
    }
    Ok(())
}

fn curl_to_string(url: &str) -> Result<String, BackendError> {
    let output = std::process::Command::new("curl")
        .args(["-fsSL", "--connect-timeout", "15", "--retry", "2", url])
        .output()
        .map_err(|err| BackendError::Other(format!("curl launch failed: {err}")))?;
    if !output.status.success() {
        return Err(BackendError::Other(format!(
            "curl fetch failed (exit {:?}) for {url}",
            output.status
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|err| BackendError::Other(format!("index is not utf-8: {err}")))
}

fn sha256_hex_file(path: &Path) -> Result<String, BackendError> {
    use sha2::{Digest, Sha256};
    let data = std::fs::read(path)
        .map_err(|err| BackendError::Other(format!("cannot read {}: {err}", path.display())))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Rewrite a files.pythonhosted.org file URL onto a mirror's packages root
/// (TUNA/Aliyun mirror the identical /packages/ suffix path).
fn mirror_file_url(url: &str, mirror_packages_base: &str) -> String {
    if let Some(suffix) = url.strip_prefix(PYPI_FILES) {
        format!("{mirror_packages_base}{suffix}")
    } else {
        url.to_string()
    }
}

fn download_with_fallback(
    url: &str,
    out: &Path,
    mirror: Option<&str>,
) -> Result<String, BackendError> {
    // Attempt order: the resolved URL first, then the builtin CN mirrors for
    // files.pythonhosted.org content (mainland-China fallback).
    let mut sources: Vec<String> = vec![url.to_string()];
    if mirror.is_none() && url.starts_with(PYPI_FILES) {
        for (_, base) in MIRRORS {
            sources.push(mirror_file_url(url, &format!("{base}/packages")));
        }
    }
    let mut last_err = None;
    for src in &sources {
        let host = src.split('/').nth(2).unwrap_or(src).to_string();
        println!("[CUDA] dll provision: downloading from {host} ...");
        match curl_to_file(src, out) {
            Ok(()) => return Ok(host),
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err.unwrap_or_else(|| BackendError::Other("no download source".to_string())))
}

fn extract_wheel(wheel: &Path, into: &Path) -> Result<(), BackendError> {
    let status = std::process::Command::new("tar")
        .args([
            "-xf",
            wheel.to_string_lossy().as_ref(),
            "-C",
            into.to_string_lossy().as_ref(),
        ])
        .status();
    if status.as_ref().is_ok_and(|s| s.success()) {
        return Ok(());
    }
    // GNU tar (Linux default) cannot read zip; try unzip, then python3.
    let unzip = std::process::Command::new("unzip")
        .args([
            "-o",
            wheel.to_string_lossy().as_ref(),
            "-d",
            into.to_string_lossy().as_ref(),
        ])
        .status();
    if unzip.is_ok_and(|s| s.success()) {
        return Ok(());
    }
    let py = std::process::Command::new("python3")
        .args([
            "-m",
            "zipfile",
            "-e",
            wheel.to_string_lossy().as_ref(),
            into.to_string_lossy().as_ref(),
        ])
        .status();
    if py.is_ok_and(|s| s.success()) {
        return Ok(());
    }
    Err(BackendError::Other(
        "wheel extraction failed (tar/unzip/python3 all unavailable or failed)".to_string(),
    ))
}

fn collect_binaries(root: &Path, windows: bool, out_dir: &Path) -> Result<usize, BackendError> {
    let wanted = wanted_binary_prefixes(windows);
    let mut copied = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|err| BackendError::Other(format!("read_dir {}: {err}", dir.display())))?;
        for entry in entries {
            let entry = entry.map_err(|err| BackendError::Other(err.to_string()))?;
            let path = entry.path();
            let meta = entry
                .metadata()
                .or_else(|_| std::fs::metadata(&path))
                .map_err(|err| {
                    BackendError::Other(format!("metadata {}: {err}", path.display()))
                })?;
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let name_lower = name.to_ascii_lowercase();
            let is_wanted = if windows {
                name_lower.ends_with(".dll") && wanted.iter().any(|p| name_lower.starts_with(p))
            } else {
                name_lower.contains(".so") && wanted.iter().any(|p| name_lower.starts_with(p))
            };
            if !is_wanted {
                continue;
            }
            // fs::copy follows symlinks so real .so files land in the cache.
            let target = out_dir.join(&name);
            std::fs::copy(&path, &target)
                .map_err(|err| BackendError::Other(format!("copy {}: {err}", path.display())))?;
            copied += 1;
        }
    }
    Ok(copied)
}

fn cache_root(requested: &str) -> Result<PathBuf, BackendError> {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|_| PathBuf::from("."));
    let dir = base.join("nvoc").join("cuda-dlls").join(requested);
    std::fs::create_dir_all(&dir)
        .map_err(|err| BackendError::Other(format!("cache dir {}: {err}", dir.display())))?;
    Ok(dir)
}

/// Build major must match the compile-time CUDA feature (cudarc's candidate
/// names and bindings come from exactly one generation).
fn build_cuda_major() -> u32 {
    if cfg!(feature = "cuda13") {
        13
    } else if cfg!(feature = "cuda11") {
        11
    } else {
        12
    }
}

fn preload_dir(dir: &Path, windows: bool) -> Result<usize, BackendError> {
    if !windows {
        // Linux: cudarc's dlopen resolves via LD_LIBRARY_PATH — no preload
        // here; the caller prints the setup prompt instead.
        return Ok(0);
    }
    use windows_sys::Win32::System::LibraryLoader::{
        LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW,
    };
    let mut loaded = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let entries = std::fs::read_dir(dir)
        .map_err(|err| BackendError::Other(format!("read_dir {}: {err}", dir.display())))?;
    for entry in entries {
        let entry = entry.map_err(|err| BackendError::Other(err.to_string()))?;
        let path = entry.path();
        if path.extension().map(|e| e != "dll").unwrap_or(true) {
            continue;
        }
        let wide: Vec<u16> = path
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            LoadLibraryExW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                LOAD_WITH_ALTERED_SEARCH_PATH,
            )
        };
        if handle.is_null() {
            failures.push(path.to_string_lossy().to_string());
        } else {
            loaded += 1;
            // Intentionally leaked: the module must stay registered for the
            // process lifetime so cudarc's name-based lookup hits it.
        }
    }
    if loaded == 0 {
        return Err(BackendError::Other(format!(
            "no DLL could be preloaded from {}: {failures:?}",
            dir.display()
        )));
    }
    Ok(loaded)
}

fn ensure_downloaded(requested: &str, mirror: Option<&str>) -> Result<PathBuf, BackendError> {
    let major = parse_version_segments(requested)
        .first()
        .copied()
        .ok_or_else(|| BackendError::Other(format!("invalid --cuda-dll-version: {requested}")))?;
    let build_major = build_cuda_major();
    if major != build_major {
        return Err(BackendError::Other(format!(
            "--cuda-dll-version {requested} does not match this build's CUDA generation (CUDA {build_major} headers): rebuild with --features cuda{major} (e.g. cargo build --no-default-features --features cuda{major})"
        )));
    }
    let dir = cache_root(requested)?;
    let marker = dir.join(".complete");
    if marker.exists() {
        println!(
            "[CUDA] dll provision: {requested} already cached in {}",
            dir.display()
        );
        return Ok(dir);
    }
    let windows = cfg!(windows);
    let index_base = match mirror {
        Some("tuna") => "https://pypi.tuna.tsinghua.edu.cn/simple".to_string(),
        Some("aliyun") => "https://mirrors.aliyun.com/pypi/simple".to_string(),
        Some(custom) if custom.starts_with("http") => format!("{custom}/simple"),
        Some(other) => {
            return Err(BackendError::Other(format!(
                "--cuda-dll-mirror: unknown mirror {other} (use tuna, aliyun, or an index base URL)"
            )));
        }
        None => PYPI_INDEX.to_string(),
    };
    let tmp = dir.join("tmp");
    std::fs::create_dir_all(&tmp).map_err(|err| BackendError::Other(format!("tmp dir: {err}")))?;
    let mut resolved: Vec<String> = Vec::new();
    for pkg in packages_for_major(major) {
        let index_url = format!("{index_base}/{pkg}/");
        println!("[CUDA] dll provision: resolving {pkg} from {index_url}");
        let index_html = curl_to_string(&index_url)?;
        let (filename, url, sha) =
            pick_wheel(&index_html, requested, windows).ok_or_else(|| {
                BackendError::Other(format!(
                    "no {pkg} wheel matching version {requested} for this platform on {index_url}"
                ))
            })?;
        println!("[CUDA] dll provision: {pkg} -> {filename}");
        let wheel_path = tmp.join(&filename);
        let source = download_with_fallback(&url, &wheel_path, mirror)?;
        let got = sha256_hex_file(&wheel_path)?;
        if !got.eq_ignore_ascii_case(&sha) {
            return Err(BackendError::Other(format!(
                "sha256 mismatch for {filename}: expected {sha}, got {got}"
            )));
        }
        println!("[CUDA] dll provision: {filename} sha256 OK (from {source})");
        let copied =
            extract_wheel(&wheel_path, &tmp).and_then(|_| collect_binaries(&tmp, windows, &dir))?;
        if copied == 0 {
            return Err(BackendError::Other(format!(
                "no wanted binaries found inside {filename}"
            )));
        }
        resolved.push(format!("{pkg} {filename}"));
        let _ = std::fs::remove_file(&wheel_path);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::write(
        &marker,
        format!("requested={requested}\n{}", resolved.join("\n")),
    )
    .map_err(|err| BackendError::Other(err.to_string()))?;
    Ok(dir)
}

/// Entry point called before any CUDA API use: provisions (if a version is
/// requested) and preloads (Windows) the DLLs, returning the directory they
/// came from (None = system libraries, behavior unchanged).
pub fn prepare(provision: &CudaDllProvision) -> Result<Option<PathBuf>, BackendError> {
    let windows = cfg!(windows);
    let dir = match (&provision.dir, &provision.version) {
        (Some(d), _) => {
            preload_dir(d, windows)?;
            Some(d.clone())
        }
        (None, Some(v)) => {
            let d = ensure_downloaded(v, provision.mirror.as_deref())?;
            let loaded = preload_dir(&d, windows)?;
            if windows {
                println!(
                    "[CUDA] dll provision: preloaded {loaded} DLL(s) from {}",
                    d.display()
                );
            } else {
                println!(
                    "[CUDA] dll provision: libraries cached in {} - run with LD_LIBRARY_PATH=\"{}:$LD_LIBRARY_PATH\"",
                    d.display(),
                    d.display()
                );
            }
            Some(d)
        }
        (None, None) => None,
    };
    Ok(dir)
}

#[cfg(all(test, feature = "cuda"))]
mod tests {
    use super::*;

    #[test]
    fn parse_version_segments_orders_correctly() {
        assert_eq!(parse_version_segments("12.8.4.1"), vec![12, 8, 4, 1]);
        assert!(parse_version_segments("12.9.3.13") > parse_version_segments("12.9.3.4"));
        assert_eq!(parse_version_segments("12.8"), vec![12, 8]);
    }

    #[test]
    fn pick_wheel_matches_prefix_and_platform() {
        let html = r#"
        <a href="https://files.pythonhosted.org/packages/aa/nvidia_cublas_cu12-12.8.4.1-py3-none-win_amd64.whl#sha256=aaaa">nvidia_cublas_cu12-12.8.4.1-py3-none-win_amd64.whl</a><br/>
        <a href="https://files.pythonhosted.org/packages/bb/nvidia_cublas_cu12-12.9.3.13-py3-none-win_amd64.whl#sha256=bbbb">nvidia_cublas_cu12-12.9.3.13-py3-none-win_amd64.whl</a><br/>
        <a href="https://files.pythonhosted.org/packages/cc/nvidia_cublas_cu12-12.9.3.13-py3-none-manylinux_2_27_x86_64.whl#sha256=cccc">nvidia_cublas_cu12-12.9.3.13-py3-none-manylinux_2_27_x86_64.whl</a><br/>
        <a href="https://files.pythonhosted.org/packages/dd/nvidia_cublas_cu12-12.8.9.9-py3-none-win_amd64.whl#sha256=dddd" data-requires-python="&gt;=3">nvidia_cublas_cu12-12.8.9.9-py3-none-win_amd64.whl</a><br/>
        "#;
        // "12.8" prefix accepts the highest 12.8.*.
        let (name, _, _) = pick_wheel(html, "12.8", true).expect("12.8 match");
        assert_eq!(name, "nvidia_cublas_cu12-12.8.9.9-py3-none-win_amd64.whl");
        // Exact version match.
        let (name, _, _) = pick_wheel(html, "12.9.3.13", true).expect("exact match");
        assert_eq!(name, "nvidia_cublas_cu12-12.9.3.13-py3-none-win_amd64.whl");
        // Linux pick ignores win wheels.
        let (name, _, _) = pick_wheel(html, "12.9.3.13", false).expect("linux match");
        assert!(name.contains("manylinux"));
        // No 13.x here: prefix 13.0 misses.
        assert!(pick_wheel(html, "13.0", true).is_none());
    }

    #[test]
    fn wanted_prefixes_cover_the_stressor_binaries() {
        // Regression: prefixes must be lowercase so the lowercased filename
        // compare never drops cublasLt64_12.dll (the preloading bug that made
        // the cache useless while the run still "worked" on a shadowed DLL).
        for p in wanted_binary_prefixes(true) {
            assert_eq!(p.to_ascii_lowercase(), *p, "{p} must be lowercase");
        }
        let lt = "cublaslt64_12.dll";
        assert!(
            wanted_binary_prefixes(true)
                .iter()
                .any(|p| lt.starts_with(p))
        );
        let lt_linux = "libcublasLt.so.12".to_ascii_lowercase();
        assert!(
            wanted_binary_prefixes(false)
                .iter()
                .any(|p| lt_linux.starts_with(p))
        );
    }

    #[test]
    fn mirror_rewrite_keeps_the_packages_suffix() {
        let url = "https://files.pythonhosted.org/packages/aa/bb/nvidia_cublas_cu12-12.8.4.1-py3-none-win_amd64.whl";
        let rewritten = mirror_file_url(url, "https://pypi.tuna.tsinghua.edu.cn/packages");
        assert!(rewritten.starts_with("https://pypi.tuna.tsinghua.edu.cn/packages/"));
        assert!(rewritten.ends_with("nvidia_cublas_cu12-12.8.4.1-py3-none-win_amd64.whl"));
    }
}
