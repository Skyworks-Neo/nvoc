//! `nvoc-srv` — the control service binary.
//!
//! Windows: launched without arguments by the SCM (`dispatch`); launched by
//! a human, pass `--foreground` to run in the console.
//! Linux/macOS: always foreground (systemd unit: `srv/systemd/nvoc-srv.service`).

use clap::Parser;
use log::{error, info, warn};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "nvoc-srv",
    about = "NVOC closed-loop control service: fan PID thermal control"
)]
struct Cli {
    /// Run in the foreground (console) instead of the Windows service manager.
    #[arg(long)]
    foreground: bool,
    /// TOML config file (default: %PROGRAMDATA%\nvoc\nvoc-srv.toml or
    /// /etc/nvoc/nvoc-srv.toml).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Override the control tick interval (ms).
    #[arg(long)]
    interval_ms: Option<u64>,
    /// Override the PID target temperature (°C).
    #[arg(long)]
    target_c: Option<f32>,
    /// Override PID kp (fan % per °C of error).
    #[arg(long)]
    kp: Option<f32>,
    /// Override PID ki (fan % per °C·s of accumulated error).
    #[arg(long)]
    ki: Option<f32>,
    /// Override PID kd (fan % per °C/s of temperature slope).
    #[arg(long)]
    kd: Option<f32>,
    /// Override the PID feed-forward base duty (%).
    #[arg(long)]
    base_percent: Option<f32>,
    /// Override the HTTP control-plane port.
    #[arg(long)]
    port: Option<u16>,
}

fn main() {
    let cli = Cli::parse();

    #[cfg(windows)]
    if !cli.foreground {
        // SCM launch path: block on the dispatcher until the service stops.
        if let Err(e) = nvoc_srv::service::dispatch() {
            eprintln!("service dispatcher failed: {e}");
            std::process::exit(1);
        }
        return;
    }

    #[cfg(not(windows))]
    let _ = cli.foreground;

    run_foreground(cli);
}

fn load_config(path: &std::path::Path) -> nvoc_srv::RuntimeConfig {
    match nvoc_srv::config::load_file(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error ({}): {e}", path.display());
            std::process::exit(2);
        }
    }
}

fn apply_overrides(cfg: &mut nvoc_srv::RuntimeConfig, cli: &Cli) {
    if let Some(v) = cli.interval_ms {
        cfg.interval_ms = v;
    }
    if let Some(v) = cli.target_c {
        cfg.pid.target_c = v;
    }
    if let Some(v) = cli.kp {
        cfg.pid.kp = v;
    }
    if let Some(v) = cli.ki {
        cfg.pid.ki = v;
    }
    if let Some(v) = cli.kd {
        cfg.pid.kd = v;
    }
    if let Some(v) = cli.base_percent {
        cfg.pid.base_percent = v;
    }
    if let Some(v) = cli.port {
        cfg.port = v;
    }
}

fn run_foreground(cli: Cli) {
    let path = cli
        .config
        .clone()
        .unwrap_or_else(nvoc_srv::config::default_config_path);
    let mut cfg = load_config(&path);
    apply_overrides(&mut cfg, &cli);
    if let Err(e) = cfg.validate() {
        eprintln!("config error: {e}");
        std::process::exit(2);
    }

    match nvoc_srv::logging::init(true) {
        Ok(p) => info!("nvoc-srv foreground starting (log: {})", p.display()),
        Err(e) => {
            eprintln!("logging init failed: {e}");
            std::process::exit(1);
        }
    }
    info!(
        "control plane on 127.0.0.1:{} — config: {}",
        cfg.port,
        path.display()
    );

    let handles = nvoc_srv::runtime::setup(cfg);
    nvoc_srv::http::spawn_supervised(
        handles.config.clone(),
        handles.status.clone(),
        handles.cmd_tx.clone(),
    );

    let shutdown_tx = handles.shutdown_tx.clone();
    if ctrlc::set_handler(move || {
        let _ = shutdown_tx.send(());
    })
    .is_err()
    {
        warn!("ctrl-c handler unavailable; stop via HTTP POST /shutdown");
    }

    match nvoc_srv::runtime::run_control_loop(handles) {
        Ok(()) => info!("stopped cleanly; fans restored to driver control"),
        Err(e) => {
            error!("control loop failed: {e}");
            std::process::exit(1);
        }
    }
}
