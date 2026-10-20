//! PMxDrv 传输层 live 探针(只读;需管理员 + 已加载 pmxdrv.sys)。
//!
//! 运行(elevated PowerShell):
//! ```text
//! sc create PMXDRV type= kernel start= demand binPath= <pmxdrv.sys 绝对路径>
//! sc start PMXDRV
//! cargo test -p nvoc-core --test kmd_pmxdrv_probe_live -- --ignored --nocapture
//! ```
//! pmxdrv.sys 取自 `reverse/xocd/pmxdrv.sys`(xOCD.exe 内嵌资源提取,Intel
//! 签名,SHA256 B1A8EE1222EEA5F199028D90B9B77C2ACF46D6D84A9E125403B2888C6F681C72);
//! 不入库(Intel 专有二进制),留在 reverse/ 车道。
#![cfg(windows)]

use nvoc_core::kmd::pagewalk::{
    PeFingerprint, discover_root, find_loaded_module, read_virtual, verify_live_header,
};
use nvoc_core::kmd::pmxdrv::{PmxDrv, PmxDrvPhysMem};
use std::io::Read;
use std::path::Path;

/// 阶段一:传输层冒烟 —— 打开设备、映射低内存一页、读回、反映射。
#[test]
#[ignore = "requires pmxdrv.sys service + elevated shell"]
fn probe_pmxdrv_transport_maps_low_memory() {
    let drv = match PmxDrv::connect() {
        Ok(drv) => drv,
        Err(err) => panic!("PMxDrv open failed: {err}"),
    };
    // IVT/BIOS 数据区(0x1000..0x2000)是恒在 RAM,任何 x64 机器可读。
    let va = drv
        .map_physical(0x1000, 1)
        .expect("failed to map physical 0x1000");
    println!("mapped: physical page 0x1000 -> user VA 0x{va:016X}");
    let window = unsafe { std::slice::from_raw_parts(va as *const u8, 4096) };
    let nonzero = window.iter().filter(|&&b| b != 0).count();
    println!("non-zero bytes in page: {nonzero}/4096");
    drv.unmap_physical(va).expect("unmap failed");
    println!("unmapped: transport map/read/unmap all pass");
}

/// 阶段二:完整走查 —— 定位 nvlddmkm → low-stub 扫描 → 页表走查 →
/// 活体/磁盘 PE 头比对(与 xOCD NvidiaKernelReader 同算法)。
#[test]
#[ignore = "requires pmxdrv.sys service + elevated shell"]
fn probe_kernel_walk_reads_nvlddmkm_header() {
    let module =
        find_loaded_module("nvlddmkm.sys").expect("nvlddmkm.sys not in system module list");
    println!(
        "nvlddmkm: base 0x{:016X}, image {}, SeDebugPrivilege={}",
        module.base,
        module.path.display(),
        module.debug_privilege
    );

    let disk_head = read_file_head(&module.path, 4096).unwrap_or_else(|err| {
        panic!(
            "failed to read disk image header {}: {err}",
            module.path.display()
        )
    });
    let fingerprint =
        PeFingerprint::from_image(&disk_head).expect("disk image header is not a valid PE32+");
    println!(
        "disk fingerprint: timestamp=0x{:08X}, SizeOfImage=0x{:X}, {}-byte header prefix",
        fingerprint.timestamp,
        fingerprint.size_of_image,
        fingerprint.header_prefix.len()
    );

    let drv = match PmxDrv::connect() {
        Ok(drv) => drv,
        Err(err) => panic!("PMxDrv open failed (run sc start PMXDRV first, elevated): {err}"),
    };
    let physical = PmxDrvPhysMem::new(&drv);
    let discovery = discover_root(&physical, module.base, &fingerprint);
    for event in &discovery.events {
        println!("  {event}");
    }
    println!(
        "discovery stats: low-stub {} pages / readable {}/{} / candidates {} / {} roots",
        discovery.low_stubs.len(),
        discovery.readable_pages,
        discovery.scanned_pages,
        discovery.candidates_tested,
        discovery.roots.len()
    );
    let root = discovery
        .unique_root()
        .expect("root not unique, walk aborted (see events above)");

    let check = verify_live_header(&physical, root, module.base, &fingerprint)
        .expect("failed to read back live image header");
    println!("live verification (root=0x{root:016X}): {check:?}");
    assert!(
        check.all(),
        "live image header disagrees with disk: {check:?}"
    );

    let live =
        read_virtual(&physical, root, module.base, 64).expect("failed to read live image header");
    assert_eq!(&live[..2], b"MZ", "live image missing MZ");
    println!(
        "live header 64 bytes: {}",
        live.iter().map(|b| format!("{b:02X}")).collect::<String>()
    );
}

fn read_file_head(path: &Path, len: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    std::fs::File::open(path)?.read_exact(&mut buf)?;
    Ok(buf)
}
