//! Foreground host for local deployment/debugging; the Windows service embeds
//! exactly the same Host. This binary never launches a scan without an API request.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let install = std::env::current_exe()?.parent().unwrap().to_path_buf();
    let root = std::env::var_os("NVOC_SCAN_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| install.join("scan-data"));
    let host = nvoc_srv::scan_api::Host::start(
        root,
        install.join("nvoc-auto-optimizer.exe"),
        std::env::var("NVOC_SCAN_MANUAL_TOKEN")?,
        std::env::var("NVOC_SCAN_AUTOMATION_TOKEN")?,
    )?;
    eprintln!("Scan API listening on 127.0.0.1:14515; press Enter to stop and clean up scans.");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    host.stop();
    Ok(())
}
#[cfg(not(windows))]
fn main() {
    eprintln!("Hosted optimizer processes currently require Windows.");
}
