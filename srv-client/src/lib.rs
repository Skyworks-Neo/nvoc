//! Client for the **resident** nvoc-srv control plane.
//!
//! nvoc-srv runs as a service and owns the GPU control loop; consumers (the
//! auto-optimizer, the CUDA stressor) connect to it, register a session, and
//! declare a control [`session::Session`] claim instead of spawning their own
//! srv process. This crate is that client: a dependency-free loopback HTTP
//! transport plus the registration/claim/heartbeat lifecycle.

pub mod http;
pub mod session;

pub use http::{DEFAULT_PORT, ProbeState, SrvClient};
pub use session::Session;

use std::time::{Duration, Instant};

/// Priority band for the CUDA stressor's debug thermal channel.
pub const PRIORITY_STRESSOR: i32 = 10;
/// Priority band for the auto-optimizer's `--target-temp` scan. Sits **above**
/// the MCP agent (30): the scan holds a temperature-critical exclusive session
/// for its entire duration and cannot tolerate being preempted mid-measurement,
/// so an agent's advisory request yields to it. Still below the desktop GUI/TUI
/// (40) and the human console, so a person preempts the scan.
pub const PRIORITY_OPTIMIZER: i32 = 35;
/// Priority band for the desktop GUI/TUI acting as a consumer. Above the MCP
/// agent so the human's explicit operation wins, below the reserved console so
/// a web-console human action still preempts.
pub const PRIORITY_DESKTOP: i32 = 40;

/// Ensure a resident srv is answering on `port`, starting the installed
/// service if nothing is listening. Never spawns a per-client child — the srv
/// is a shared, long-lived service.
pub fn ensure_resident(port: u16) -> Result<(), String> {
    let client = SrvClient::new(port);
    match client.probe_state() {
        Ok(ProbeState::Ready) => Ok(()),
        Ok(ProbeState::Starting) => {
            log::info!("nvoc-srv on port {port} is starting; waiting for readiness");
            wait_ready(&client, Duration::from_secs(15))
        }
        Ok(ProbeState::Absent) => {
            log::info!("no nvoc-srv on port {port}; starting the installed service");
            start_service()?;
            wait_ready(&client, Duration::from_secs(20))
        }
        Err(e) => Err(format!(
            "port {port} is occupied by something that is not a healthy nvoc-srv: {}",
            e.message
        )),
    }
}

/// Poll `/status` until the control loop reports GPUs, or the deadline passes.
fn wait_ready(client: &SrvClient, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        match client.probe_state() {
            Ok(ProbeState::Ready) => return Ok(()),
            Ok(ProbeState::Starting) => {}
            Ok(ProbeState::Absent) => {}
            Err(e) => {
                return Err(format!(
                    "nvoc-srv became unhealthy while waiting: {}",
                    e.message
                ));
            }
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "nvoc-srv did not become ready within {timeout:?}; check the service logs"
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Start the installed nvoc-srv service. Requires the service to already be
/// installed (Windows: `nvoc-srv-ctl install`; Linux: the systemd unit) and,
/// on Windows, an elevated caller.
#[cfg(windows)]
fn start_service() -> Result<(), String> {
    run_service_command("net", &["start", "nvoc_service"]).map_err(|e| {
        format!(
            "could not start the nvoc_service Windows service: {e}; \
             install it with `nvoc-srv-ctl install` and start it (needs elevation)"
        )
    })
}

#[cfg(not(windows))]
fn start_service() -> Result<(), String> {
    run_service_command("systemctl", &["start", "nvoc-srv"]).map_err(|e| {
        format!(
            "could not start the nvoc-srv systemd unit: {e}; \
             install srv/systemd/nvoc-srv.service and `systemctl daemon-reload`"
        )
    })
}

fn run_service_command(program: &str, args: &[&str]) -> Result<(), String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program} could not be run: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(format!(
            "{program} {} exited {}: {}{}",
            args.join(" "),
            output.status,
            stdout.trim(),
            stderr.trim()
        ))
    }
}
