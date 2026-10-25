//! 内嵌 pmxdrv.sys 载荷钉死测试(非 live,无需管理员/GPU)。
//!
//! 钉住两件事:
//! 1. `include_bytes!` 进 nvoc-cli 的载荷 == 仓库内 Intel 2019 签名构建
//!    (SHA256 与 power.rs::PMXDRV_SYS_SHA256、kmd_pmxdrv_probe_live.rs
//!    头注三方一致,防 .sys 被静默调包);
//! 2. `extract_pmxdrv_sys()` 落盘内容逐字节 == 载荷(写-读回链路完整,
//!    SCM 装载吃的就是这份磁盘镜像)。
#![cfg(all(target_os = "windows", target_arch = "x86_64"))]

use nvoc_core::kmd::power::{PMXDRV_SYS, PMXDRV_SYS_SHA256, extract_pmxdrv_sys};
use sha2::{Digest, Sha256};

/// 载荷指纹必须钉死在 Intel 2019 签名构建上;长度同步钉住。
#[test]
fn embedded_payload_matches_pinned_sha256() {
    let digest = Sha256::digest(PMXDRV_SYS);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, PMXDRV_SYS_SHA256);
    assert_eq!(PMXDRV_SYS.len(), 43_632);
}

/// 释放路径写-读回一致;测试结束清理落盘副本。
#[test]
fn extraction_round_trips_payload() {
    let path = extract_pmxdrv_sys().expect("embedded payload extraction");
    let written = std::fs::read(&path).expect("readback of extracted payload");
    assert_eq!(written, PMXDRV_SYS);
    let _ = std::fs::remove_file(&path);
}
