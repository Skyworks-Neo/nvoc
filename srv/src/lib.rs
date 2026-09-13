//! NVOC-SRV: closed-loop GPU control service.
//!
//! Platform-neutral core:
//! - [`config`]: layered runtime configuration (defaults → TOML file → CLI
//!   flags → HTTP mutations)
//! - [`pid`]: pure PID controller (no GPU dependency, unit tested)
//! - [`controller`]: per-GPU mode state machine + failsafes around the PID
//! - [`backend`]: NVAPI-backed [`ControlBackend`] with NVML fallback
//! - [`runtime`]: single-threaded control loop + heartbeat watchdog
//! - [`http`]: loopback-only HTTP control plane
//! - [`logging`]: rotating file sink (+console tee in foreground)
//!
//! Windows-only SCM integration lives in [`service`]; on Linux the same
//! binary runs in foreground mode under the systemd unit in `srv/systemd/`.

pub mod backend;
pub mod config;
pub mod controller;
pub mod http;
pub mod logging;
pub mod pid;
pub mod runtime;
#[cfg(windows)]
pub mod service;

pub use config::{
    ControlMode, FreqParams, GpuSelection, LoopKind, PidParams, RuntimeConfig, SensorKind,
};
pub use controller::{ControlBackend, GpuControlStatus, SensorBundle};
