use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=OGKM");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos")
        || env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("x86_64")
    {
        return;
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let ogkm = env::var_os("OGKM")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("../../reference/open-gpu-kernel-modules-610.57.04"));
    let sdk = ogkm.join("src/common/sdk/nvidia/inc");
    assert!(
        sdk.is_dir(),
        "macOS RM backend requires OGKM 610.57.04 headers; set OGKM to the source tree"
    );
    let probe = root.join("../probe-macos-rm/src");
    let files = [
        root.join("native/macos_rm.c"),
        probe.join("nvx_xnu.c"),
        probe.join("nvx_rm.c"),
    ];
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    println!(
        "cargo:rerun-if-changed={}",
        root.join("native/rusd_read.h").display()
    );
    for header in ["nvx_xnu.h", "nvx_rm.h", "nvx_defs.h"] {
        println!("cargo:rerun-if-changed={}", probe.join(header).display());
    }
    println!("cargo:rerun-if-changed={}", sdk.display());
    let unix = ogkm.join("src/nvidia/arch/nvalloc/unix/include");
    println!("cargo:rerun-if-changed={}", unix.display());
    cc::Build::new()
        .files(files)
        .include(&probe)
        .include(sdk)
        .include(unix)
        .flag("-std=c11")
        .compile("nvoc_macos_rm");
    println!("cargo:rustc-link-lib=framework=IOKit");
    println!("cargo:rustc-link-lib=framework=CoreFoundation");
}
