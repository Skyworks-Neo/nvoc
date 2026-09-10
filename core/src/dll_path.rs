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
//! 3. init 失败 → 依次尝试候选绝对路径(WOA DriverStore ARM64EC → System32
//!    → NVSMI 老布局)。
//!
//! WOA(Windows on ARM64)布局(616.00 WOA 包 nv_surface_woa.inf 实证):
//! `System32\nvml.dll` 是纯 ARM64 的 `nvml_loader.dll` 改名副本(INF 行
//! `nvml.dll,nvml_loader.dll,,0x00004000`),x64 进程加载直接报
//! ERROR_BAD_EXE_FORMAT(193);x64 可用的 NVML 以 ARM64EC 形式
//! `nvml_arm64ec.dll` 只落在 DriverStore FileRepository。x64 构建在 WOA 上
//! 要么依赖下方候选探测,要么显式 `NVOC_NVML_PATH`(CLI `--nvml-path`,值为
//! 完整 DLL 路径)指向 `...\FileRepository\nv_surface_woa.inf_*\nvml_arm64ec.dll`;
//! ARM64 原生构建走 System32 副本,不受影响。
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

use std::path::{Path, PathBuf};

/// 显式 NVML 库路径的 env 变量名(CLI `--nvml-path` 注入同名 env)。
pub const NVML_PATH_ENV: &str = "NVOC_NVML_PATH";
/// 显式 NVAPI 库目录的 env 变量名(CLI `--nvapi-path` 注入同名 env;值为
/// nvapi64.dll 所在目录)。
pub const NVAPI_PATH_ENV: &str = "NVOC_NVAPI_PATH";

/// 旧驱动布局的 NVSMI 目录(64 位;32 位 DLL 同目录)。
const NVSMI_DIR: &str = r"C:\Program Files\NVIDIA Corporation\NVSMI";

/// NVML 自动 fallback 的候选绝对路径,按新旧驱动布局排序:
/// WOA DriverStore 的 ARM64EC NVML(x64 进程在 WOA 上唯一可用的 NVML;非 WOA
/// 环境为空列表,零成本)→ System32(新驱动布局,显式列出以覆盖 PATH 被裁剪的
/// 场合)→ NVSMI(老驱动布局)。init_nvml 逐个尝试,哪个 init 成功用哪个,候选
/// 顺序只影响探测开销、不影响正确性。
fn nvml_candidates() -> Vec<PathBuf> {
    let mut candidates = woa_driverstore_candidates();
    candidates.push(Path::new(r"C:\Windows\System32\nvml.dll").to_path_buf());
    candidates.push(Path::new(NVSMI_DIR).join("nvml.dll"));
    candidates
}

/// WOA DriverStore 里的 `nvml_arm64ec.dll`(ARM64EC,x64 与 ARM64 进程均可
/// 加载)候选。616+ WOA 驱动 INF 只把纯 ARM64 的 nvml_loader.dll 改名成
/// System32\nvml.dll,ARM64EC 版本留在 FileRepository 的 `nv*.inf_*` 目录;
/// 逐目录探测,目录名带 hash 无版本序,命中多个时由 init_nvml 择优。
#[cfg(windows)]
fn woa_driverstore_candidates() -> Vec<PathBuf> {
    let store = Path::new(r"C:\Windows\System32\DriverStore\FileRepository");
    let Ok(entries) = std::fs::read_dir(store) else {
        return Vec::new();
    };
    let mut hits: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|dir| {
            dir.is_dir()
                && dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("nv") && name.contains(".inf_"))
        })
        .map(|dir| dir.join("nvml_arm64ec.dll"))
        .filter(|candidate| candidate.is_file())
        .collect();
    hits.sort();
    hits
}

#[cfg(not(windows))]
fn woa_driverstore_candidates() -> Vec<PathBuf> {
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

/// NVAPI 首次调用前的准备:若设置了显式覆盖目录(env `NVOC_NVAPI_PATH`),
/// 把它插入传统 DLL 搜索序,使后续 `LoadLibraryA("nvapi64.dll")` 命中该目录。
/// 非覆盖场景是零成本 no-op。
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
    #[cfg(not(windows))]
    let _ = override_path(NVAPI_PATH_ENV);
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

    #[test]
    fn nvml_candidates_cover_system32_and_nvspmi() {
        let candidates = nvml_candidates();
        // WOA 机器上前面会多出 DriverStore 的 nvml_arm64ec.dll 候选,末两位固定。
        let n = candidates.len();
        assert!(n >= 2);
        assert!(candidates[n - 2].ends_with(r"System32\nvml.dll"));
        assert!(candidates[n - 1].ends_with(r"NVSMI\nvml.dll"));
    }
}
