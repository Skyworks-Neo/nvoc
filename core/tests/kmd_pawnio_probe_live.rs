#![cfg(windows)]
//! PawnIO 内核通道实机探针(只读;需要管理员终端 + 已安装 PawnIO 驱动)。
//!
//! 分三段,全部 `#[ignore]`(GPU/驱动相关测试按仓库约定默认跳过):
//!
//! 1. `probe_transport_with_official_signed_module`:用官方签名模块 Echo 验证
//!    传输层(设备直连 + LOAD/EXECUTE 载荷布局),这是对 g-helper 调用形态的
//!    独立对照——它过了,才说明本层协议常量没抄错;
//! 2. `probe_unsigned_module_signature_gate`:装载自研未签名 PhysMem 模块,给出
//!    实证官方版签名门在位(未签名 blob 被拒;2.2.0 的 -unrestricted 版与
//!    官方版代码逐字节相同,见 core/kmd/README.md——此路不通已裁决);
//! 3. `probe_kernel_walk_reads_nvlddmkm_header`:完整走查——模块列表取
//!    nvlddmkm 基址 → 低内存扫 low-stub → 找页表根 → 活体/磁盘头比对。签名门
//!    拒绝时打印诊断并跳过(不算失败)。
//!
//! 运行(管理员 PowerShell/cmd):
//! `cargo test -p nvoc-core --test kmd_pawnio_probe_live -- --ignored --nocapture`
//!
//! 环境变量覆盖:`NVOC_KMD_SIGNED_BLOB`(官方签名 blob,e.g. Echo.bin)、
//! `NVOC_KMD_PHYS_MODULE`(自研 PhysMem 未签名 blob)。

use std::path::{Path, PathBuf};

use nvoc_core::kmd::pagewalk::{
    PeFingerprint, discover_root, find_loaded_module, read_virtual, verify_live_header,
};
use nvoc_core::kmd::pawnio::{PawnIo, PawnIoError, PawnIoPhysMem};

/// 官方签名模块默认为仓库内 fixture(来自 PawnIO 官方模块包 0.2.11)。
fn signed_echo_blob() -> PathBuf {
    if let Some(path) = std::env::var_os("NVOC_KMD_SIGNED_BLOB") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("kmd")
        .join("Echo.bin")
}

/// 自研 PhysMem 模块 blob(未签名,`core/kmd/PhysMem.bin`)。
fn phys_module_blob() -> PathBuf {
    if let Some(path) = std::env::var_os("NVOC_KMD_PHYS_MODULE") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("kmd")
        .join("PhysMem.bin")
}

fn read_blob(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|err| panic!("读取模块 blob {} 失败: {err}", path.display()))
}

fn connect() -> PawnIo {
    match PawnIo::connect() {
        Ok(io) => io,
        Err(PawnIoError::NotInstalled) => panic!(
            "PawnIO 设备不存在(win32 2/3):先安装驱动(winget install namazso.PawnIO,或 PawnIO_setup.exe -install)"
        ),
        Err(PawnIoError::AccessDenied) => {
            panic!("打开 PawnIO 设备被拒(win32 5):必须用管理员终端跑,或安装时开启非管理员访问")
        }
        Err(err) => panic!("打开 PawnIO 设备失败: {err}"),
    }
}

fn read_file_head(path: &Path, len: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buffer = vec![0u8; len];
    file.read_exact(&mut buffer)?;
    Ok(buffer)
}

/// 传输层对照:官方签名 Echo 模块的 `ioctl_not` 往返。
#[test]
#[ignore = "requires PawnIO driver + elevated shell"]
fn probe_transport_with_official_signed_module() {
    let io = connect();
    let version = io.version().expect("IOCTL_PIO_VERSION 失败");
    println!(
        "PawnIO 驱动版本: {}.{}.{} (raw 0x{version:08X})",
        (version >> 16) & 0xFF,
        (version >> 8) & 0xFF,
        version & 0xFF
    );

    let blob_path = signed_echo_blob();
    let blob = read_blob(&blob_path);
    println!(
        "装载官方签名模块: {} ({} 字节)",
        blob_path.display(),
        blob.len()
    );
    io.load_module(&blob)
        .expect("官方签名模块装载失败:协议常量或 blob 载荷布局与本机 PawnIO 不符");

    let input = 0x0123_4567_89AB_CDEFu64;
    let out = io
        .execute("ioctl_not", &[input], 1)
        .expect("ioctl_not 执行失败(函数名/尺寸声明/输出回填有问题)");
    assert_eq!(out, vec![!input], "Echo 回读不是按位取反");
    println!(
        "传输层往返成功: ioctl_not(0x{input:016X}) -> 0x{:016X}",
        out[0]
    );
}

/// 未签名自研模块的差分:本机 edition 是否执行签名门。
#[test]
#[ignore = "requires PawnIO driver + elevated shell"]
fn probe_unsigned_module_signature_gate() {
    let io = connect();
    let blob_path = phys_module_blob();
    let blob = read_blob(&blob_path);
    println!(
        "装载自研未签名模块: {} ({} 字节, sig_len = {})",
        blob_path.display(),
        blob.len(),
        u32::from_le_bytes(blob[..4].try_into().unwrap())
    );
    match io.load_module(&blob) {
        Ok(()) => {
            println!("结果: 接受 → 本机 PawnIO 不校验签名,自研模块可用");
            // 顺手用第一个 ioctl 验证模块真的活着:读物理地址 0 的 8 字节。
            match io.execute("ioctl_read_phys_qword", &[0], 1) {
                Ok(words) => println!("ioctl_read_phys_qword(0) -> 0x{:016X}", words[0]),
                Err(err) => println!(
                    "ioctl_read_phys_qword(0) 失败: {err}(地址 0 可能是保留页,不视为模块问题)"
                ),
            }
        }
        Err(PawnIoError::Win32(code)) => println!(
            "结果: 被拒(win32 错误 {code})→ Official edition 的签名门生效:废弃 blob 需走\
             Unrestricted edition / 上游签名 / 自签分支(见 core/kmd/README.md)"
        ),
        Err(err) => println!("结果: 被拒,{err}"),
    }
}

/// 完整目标:经 PawnIO 走查 nvlddmkm 的内核虚拟地址空间(只读)。
#[test]
#[ignore = "requires PawnIO driver + elevated shell"]
fn probe_kernel_walk_reads_nvlddmkm_header() {
    let module = find_loaded_module("nvlddmkm.sys").expect("nvlddmkm.sys 不在系统模块列表");
    println!(
        "nvlddmkm: 基址 0x{:016X}, 磁盘 {}, SeDebugPrivilege={}",
        module.base,
        module.path.display(),
        module.debug_privilege
    );

    let disk_head = read_file_head(&module.path, 4096)
        .unwrap_or_else(|err| panic!("读磁盘映像头 {} 失败: {err}", module.path.display()));
    let fingerprint = PeFingerprint::from_image(&disk_head).expect("磁盘映像头不是有效 PE32+");
    println!(
        "磁盘指纹: timestamp=0x{:08X}, SizeOfImage=0x{:X}, 头前缀 {} 字节",
        fingerprint.timestamp,
        fingerprint.size_of_image,
        fingerprint.header_prefix.len()
    );

    let io = connect();
    let blob = read_blob(&phys_module_blob());
    if let Err(err) = io.load_module(&blob) {
        println!("PhysMem 模块装载失败: {err}");
        println!(
            "签名门拒绝(预期行为):内核通道已裁决改走 pmxdrv,此走查留给上游收编后复用(见 core/kmd/README.md);跳过。"
        );
        return;
    }

    let physical = PawnIoPhysMem::new(&io);
    let report = discover_root(&physical, module.base, &fingerprint);
    println!(
        "低内存扫描: {}/{} 页可读, low-stub {} 个, 候选 {} 个",
        report.readable_pages,
        report.scanned_pages,
        report.low_stubs.len(),
        report.candidates_tested
    );
    for event in &report.events {
        println!("  {event}");
    }
    let root = report
        .unique_root()
        .unwrap_or_else(|diag| panic!("页表根不唯一: {diag}"));
    println!("页表根: 0x{root:016X}");

    let check =
        verify_live_header(&physical, root, module.base, &fingerprint).expect("活体映像头读回失败");
    println!(
        "活体/磁盘比对: timestamp={} size={} prefix={}",
        check.timestamp_matches, check.size_matches, check.prefix_matches
    );
    assert!(check.all(), "活体映像头与磁盘不一致: {check:?}");

    let live = read_virtual(&physical, root, module.base, 64).expect("读活体映像头失败");
    println!("活体头 64 字节: {:02X?}", live);
    assert_eq!(&live[..2], b"MZ");
    println!("走查闭环成立: PawnIO 物理读 → 四级翻译 → nvlddmkm 活体映像可读");
}
