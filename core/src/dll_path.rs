//! NVML / NVAPI 用户态 DLL 的路径解析。
//!
//! 新驱动(R470+)把 `nvml.dll` 放进 System32(并在 PATH 上);老驱动(如
//! R391)只放在 `C:\Program Files\NVIDIA Corporation\NVSMI\`——不在任何搜索
//! 路径上,`LoadLibrary("nvml.dll")` 直接报 OS error 126(找不到模块)。
//!
//! 解析策略(NVML):
//! 1. 显式覆盖优先:env `NVOC_NVML_PATH`(CLI `--nvml-path` 启动期注入同名
//!    env),用户指哪打哪;
//! 2. 默认搜索路径(覆盖 System32/PATH 上的新驱动布局);
//! 3. init 失败 → 依次尝试候选绝对路径(System32 → NVSMI 老布局)。
//!
//! **用 init 成败代替版本判据**:一台机器只有一个 NVIDIA 内核驱动实例
//! (nvlddmkm.sys),nvml.dll 必须与它匹配。版本不匹配的 DLL 过不了 init,
//! 所以"哪个能 init 成功"就是"哪个与当前内核驱动匹配"——无需读驱动版本、
//! 无需代际判据。同机混插新/旧卡时,能枚举出哪些卡由内核驱动的支持面决定
//! (老内核驱动认不出新卡,反之亦然),与用户态 DLL 的选择无关。
//!
//! NVAPI(`nvapi64.dll`)始终在 System32,默认搜索即可命中;仅提供显式覆盖
//! (env `NVOC_NVAPI_PATH` / CLI `--nvapi-path`),覆盖时把该目录插入传统
//! DLL 搜索序(`SetDllDirectoryW`——nvapi-rs 用 `LoadLibraryA` 老式搜索,
//! `AddDllDirectory` 对它无效)。
//!
//! Linux 上两个覆盖的语义:
//! - NVML:nvml-wrapper 按 `lib_path` 直接 dlopen,文件路径开箱即用;目录
//!   则拼 `libnvidia-ml.so.1`。
//! - NVAPI:nvapi-rs 硬编码 `dlopen("libnvidia-api.so.1")`(SONAME,无路径),
//!   而进程内改 `LD_LIBRARY_PATH` 无效(glibc 启动期缓存搜索路径)。所以
//!   覆盖的实现是**预加载**:先按绝对路径 dlopen 用户指定的副本,glibc 把
//!   它的 DT_SONAME 登记进别名表,之后按 SONAME 的 dlopen 命中同一映射。
//!   探测按 SONAME 的 dlopen 是否命中(句柄相同)以便在 SONAME 不匹配时
//!   给出警告而不是静默回落系统库。

use std::path::{Path, PathBuf};

/// 显式 NVML 库路径的 env 变量名(CLI `--nvml-path` 注入同名 env)。
pub const NVML_PATH_ENV: &str = "NVOC_NVML_PATH";
/// 显式 NVAPI 库目录的 env 变量名(CLI `--nvapi-path` 注入同名 env;值为
/// nvapi64.dll 所在目录)。
pub const NVAPI_PATH_ENV: &str = "NVOC_NVAPI_PATH";

/// 旧驱动布局的 NVSMI 目录(64 位;32 位 DLL 同目录)。
#[cfg(windows)]
const NVSMI_DIR: &str = r"C:\Program Files\NVIDIA Corporation\NVSMI";

/// NVML 自动 fallback 的候选绝对路径,按新旧驱动布局排序:
/// System32(新驱动布局,显式列出以覆盖 PATH 被裁剪的场合)→ NVSMI(老驱动布局)。
/// Linux 上默认 SONAME 搜索(ldconfig)已覆盖驱动安装布局,无候选。
#[cfg(windows)]
fn nvml_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![Path::new(r"C:\Windows\System32\nvml.dll").to_path_buf()];
    candidates.push(Path::new(NVSMI_DIR).join("nvml.dll"));
    candidates
}

#[cfg(not(windows))]
fn nvml_candidates() -> Vec<PathBuf> {
    Vec::new()
}

/// 解析"当前实际加载的 nvml.dll"的候选路径(不 init、不验证可加载性)。
/// 顺序与 [`init_nvml`] 一致:env 覆盖 → System32 → NVSMI。供需要按同一路径
/// 裸调 NVML C 符号的调用方使用(Windows LoadLibrary 对同一路径复用已加载
/// 模块,不会双重加载)。
pub fn resolved_nvml_path() -> Option<PathBuf> {
    if let Some(path) = override_path(NVML_PATH_ENV) {
        return Some(path);
    }
    nvml_candidates()
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// 读取显式覆盖路径(env),空串视为未设置。
fn override_path(env: &str) -> Option<PathBuf> {
    std::env::var(env)
        .ok()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
}

/// 初始化 NVML,按 env 覆盖 → 默认搜索路径 → 候选绝对路径的顺序。
///
/// 返回默认路径的原始错误当所有候选都失败时,保持 `Nvml::init()` 旧签名
/// 的调用方错误信息不变。
pub fn init_nvml() -> Result<nvml_wrapper::Nvml, nvml_wrapper::error::NvmlError> {
    // 1) 显式覆盖(env / CLI)。指了路径就只信它——失败直接报错,让用户
    //    看到真实原因而不是静默落到别的 DLL。
    if let Some(path) = override_path(NVML_PATH_ENV) {
        // Linux 上也接受目录(拼默认 SONAME);Windows 沿用文件路径语义。
        #[cfg(unix)]
        let path = resolve_lib_file(&path, "libnvidia-ml.so.1");
        return nvml_wrapper::Nvml::builder()
            .lib_path(path.as_os_str())
            .init();
    }

    // 2) 默认搜索路径(新驱动布局 + PATH)。
    let default_err = match nvml_wrapper::Nvml::init() {
        Ok(nvml) => return Ok(nvml),
        Err(err) => err,
    };

    // 3) 候选绝对路径(老驱动布局等)。第一个 init 成功的胜出。
    for candidate in nvml_candidates() {
        if !candidate.is_file() {
            continue;
        }
        if let Ok(nvml) = nvml_wrapper::Nvml::builder()
            .lib_path(candidate.as_os_str())
            .init()
        {
            return Ok(nvml);
        }
    }

    Err(default_err)
}

/// NVAPI 首次调用前的准备:若设置了显式覆盖(env `NVOC_NVAPI_PATH`),让后续
/// 的库加载命中用户指定的副本。Windows 把覆盖目录插入传统 DLL 搜索序(使
/// `LoadLibraryA("nvapi64.dll")` 命中);Linux 预加载覆盖文件使 SONAME 查找
/// 命中(见模块文档)。非覆盖场景是零成本 no-op。
pub fn prepare_nvapi() {
    #[cfg(windows)]
    if let Some(path) = override_path(NVAPI_PATH_ENV) {
        let dir = if path.is_file() {
            // 允许直接指到 DLL 文件——取其父目录。
            match path.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
                _ => return,
            }
        } else {
            path
        };
        set_dll_directory(&dir);
    }
    #[cfg(unix)]
    if let Some(path) = override_path(NVAPI_PATH_ENV) {
        let file = resolve_lib_file(&path, "libnvidia-api.so.1");
        preload_for_soname(&file, "libnvidia-api.so.1");
    }
}

/// 把覆盖路径解析成具体库文件:目录拼 `<default_file>`,文件原样。
#[cfg(unix)]
fn resolve_lib_file(path: &Path, default_file: &str) -> PathBuf {
    if path.is_dir() {
        path.join(default_file)
    } else {
        path.to_path_buf()
    }
}

/// 预加载 `file`,使后续按 `soname` 的 dlopen 命中这份副本。
///
/// glibc 在映射对象时把 DT_SONAME 登记进该对象的别名表,再按 SONAME dlopen
/// 直接复用已有映射——这是 Linux 上改变 nvapi-rs 加载来源的唯一入口(它
/// 硬编码 `dlopen("libnvidia-api.so.1")`,而进程内改 `LD_LIBRARY_PATH` 无效)。
/// 预加载的引用计数有意泄漏到进程退出:提前 dlclose 会连带注销 SONAME 别名,
/// nvapi-rs 首次调用时会静默回落到系统库。SONAME 不命中时打警告——用户
/// 指定的副本不会被用上,静默回落比报错更难排查。
#[cfg(unix)]
fn preload_for_soname(file: &Path, soname: &str) {
    use libloading::os::unix::{Library, RTLD_LAZY, RTLD_LOCAL};

    // 与 nvapi-rs 的 dlopen 旗标一致(RTLD_LAZY|RTLD_LOCAL),保证复用同一映射。
    let flags = RTLD_LAZY | RTLD_LOCAL;
    // SAFETY: dlopen 加载 ELF 共享库;失败由 Err 分支携带 dlerror 报告。
    let by_path = match unsafe { Library::open(Some(file), flags) } {
        Ok(lib) => lib.into_raw(),
        Err(error) => {
            eprintln!(
                "warning: --nvapi-path: dlopen({}) failed: {error}; falling back to the system {soname}",
                file.display()
            );
            return;
        }
    };
    match unsafe { Library::open(Some(Path::new(soname)), flags) } {
        Ok(lib) => {
            let by_soname = lib.into_raw();
            if by_soname != by_path {
                eprintln!(
                    "warning: --nvapi-path: {} does not carry SONAME {soname}; NVAPI will load the system copy",
                    file.display()
                );
            }
            drop(unsafe { Library::from_raw(by_soname) });
        }
        Err(error) => eprintln!(
            "warning: --nvapi-path: dlopen(\"{soname}\") still fails after preloading {} ({error}); the override will not take effect",
            file.display()
        ),
    }
}

/// 把 `dir` 插入传统 DLL 搜索序(nvapi-rs 的 `LoadLibraryA` 走老式搜索,
/// 只有 `SetDllDirectory` 对它生效;`AddDllDirectory` 只影响
/// `LOAD_LIBRARY_SEARCH_*` 语义)。
#[cfg(windows)]
fn set_dll_directory(dir: &Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::LibraryLoader::SetDllDirectoryW;

    let wide: Vec<u16> = dir
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // 一次性启动期设置;失败(路径不存在等)静默——保持默认搜索行为。
    unsafe {
        SetDllDirectoryW(wide.as_ptr());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_path_reads_env_and_ignores_empty() {
        // 设env再清;用唯一变量名避免并发测试互踩。
        let key = "NVOC_TEST_OVERRIDE_PATH";
        unsafe { std::env::set_var(key, "C:\\some\\nvml.dll") };
        assert_eq!(
            override_path(key),
            Some(PathBuf::from("C:\\some\\nvml.dll"))
        );
        unsafe { std::env::set_var(key, "   ") };
        assert_eq!(override_path(key), None);
        unsafe { std::env::remove_var(key) };
        assert_eq!(override_path(key), None);
    }

    #[cfg(windows)]
    #[test]
    fn nvml_candidates_cover_system32_and_nvspmi() {
        let candidates = nvml_candidates();
        assert_eq!(candidates.len(), 2);
        assert!(candidates[0].ends_with(r"System32\nvml.dll"));
        assert!(candidates[1].ends_with(r"NVSMI\nvml.dll"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_lib_file_joins_soname_for_directories_only() {
        let dir = std::env::temp_dir();
        assert_eq!(
            resolve_lib_file(&dir, "libnvidia-api.so.1"),
            dir.join("libnvidia-api.so.1")
        );
        // 非目录(含不存在的路径)原样返回,错误留给 dlopen/dlerror 报。
        let file = dir.join("some-libnvidia-api.so.1");
        assert_eq!(resolve_lib_file(&file, "libnvidia-api.so.1"), file);
    }
}
