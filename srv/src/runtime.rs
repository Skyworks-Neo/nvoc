//! The control loop: one OS thread ticking every `interval_ms`, plus an
//! independent heartbeat watchdog.
//!
//! The legacy service drove this from a compio async runtime; the workload
//! is timer + two channels, which a plain thread with `recv_deadline`
//! expresses without the async machinery — and the synchronous design lets
//! the watchdog share the backend via `Arc<Mutex<_>>` so a hung loop still
//! gets its fans un-pinned.

use crate::backend::{GPU_INDEX_MAX, NvapiBackend};
use crate::config::RuntimeConfig;
use crate::controller::{ControlBackend as _, GpuController};
use crate::pid::PidController;
use flume::{Receiver, Sender};
use log::{error, info};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub type SharedConfig = Arc<Mutex<RuntimeConfig>>;
pub type SharedStatus = Arc<Mutex<Vec<crate::controller::GpuControlStatus>>>;
pub type SharedHeartbeat = Arc<Mutex<Instant>>;

/// Imperative commands from the HTTP plane that are not pure config edits.
#[derive(Debug, Clone)]
pub enum ServiceCmd {
    /// Legacy `/oc_global`: one-shot P0 graphics clock delta.
    SetOcGlobal { gpu_index: usize, delta_khz: i32 },
    /// Graceful loop exit (fans restored first).
    Shutdown,
}

/// Everything a service/foreground entrypoint needs to wire the loop up.
pub struct LoopHandles {
    pub config: SharedConfig,
    pub status: SharedStatus,
    pub heartbeat: SharedHeartbeat,
    pub cmd_tx: Sender<ServiceCmd>,
    pub cmd_rx: Receiver<ServiceCmd>,
    pub shutdown_tx: Sender<()>,
    pub shutdown_rx: Receiver<()>,
}

pub fn setup(config: RuntimeConfig) -> LoopHandles {
    let (cmd_tx, cmd_rx) = flume::unbounded();
    let (shutdown_tx, shutdown_rx) = flume::unbounded();
    LoopHandles {
        config: Arc::new(Mutex::new(config)),
        status: Arc::new(Mutex::new(Vec::new())),
        heartbeat: Arc::new(Mutex::new(Instant::now())),
        cmd_tx,
        cmd_rx,
        shutdown_tx,
        shutdown_rx,
    }
}

/// Mutex access that survives a panic in any other user of the shared state
/// (a long-lived service must not brick its control plane on one poison).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn spawn_watchdog(
    backend: Arc<Mutex<NvapiBackend>>,
    config: SharedConfig,
    heartbeat: SharedHeartbeat,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("nvoc-watchdog".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(5));
                let stale_for = lock(&heartbeat).elapsed();
                let timeout = Duration::from_secs(lock(&config).watchdog_timeout_s);
                if stale_for <= timeout {
                    continue;
                }
                error!(
                    "watchdog: control-loop heartbeat stale for {stale_for:?} (> {timeout:?}); \
                 restoring fan control to driver"
                );
                let mut backend_guard = lock(&backend);
                for i in 0..backend_guard.gpu_count() {
                    if let Err(e) = backend_guard.restore_fan_auto(i) {
                        error!("watchdog: GPU {i} restore failed: {e}");
                    }
                }
            }
        })
        .expect("spawn watchdog thread")
}

/// Blocking control loop. Returns when shutdown is signalled (SCM stop,
/// Ctrl-C, `/shutdown`) or discovery fails; fans are restored to driver
/// control on every exit path that reaches the loop.
pub fn run_control_loop(handles: LoopHandles) -> Result<(), String> {
    let backend = NvapiBackend::discover()?;
    let discovered = backend.gpu_count();
    let backend = Arc::new(Mutex::new(backend));

    let cfg0 = lock(&handles.config).clone();
    let selected = cfg0.selected_indices(discovered);
    let mut controllers: Vec<GpuController> = selected
        .iter()
        .map(|_| GpuController::new(PidController::from_params(&cfg0.pid)))
        .collect();
    info!(
        "control loop: {} of {discovered} GPU(s) under control, mode {:?}, interval {} ms",
        selected.len(),
        cfg0.mode,
        cfg0.interval_ms
    );
    *lock(&handles.heartbeat) = Instant::now();
    spawn_watchdog(
        backend.clone(),
        handles.config.clone(),
        handles.heartbeat.clone(),
    );

    let LoopHandles {
        config,
        status,
        heartbeat,
        cmd_rx,
        shutdown_rx,
        ..
    } = handles;

    loop {
        let interval = Duration::from_millis(lock(&config).interval_ms);
        let deadline = Instant::now() + interval;
        let mut stop = false;

        while Instant::now() < deadline {
            match shutdown_rx.try_recv() {
                Ok(()) | Err(flume::TryRecvError::Disconnected) => {
                    stop = true;
                    break;
                }
                Err(flume::TryRecvError::Empty) => {}
            }
            match cmd_rx.recv_deadline(deadline) {
                Ok(ServiceCmd::Shutdown) => {
                    stop = true;
                    break;
                }
                Ok(cmd) => handle_cmd(cmd, &backend),
                // HTTP plane gone: keep controlling; the watchdog owns failsafe.
                Err(flume::RecvTimeoutError::Disconnected) => {}
                Err(flume::RecvTimeoutError::Timeout) => break,
            }
        }
        if stop {
            break;
        }

        let cfg = lock(&config).clone();
        let dt_s = cfg.interval_ms as f32 / 1000.0;
        let mut backend_guard = lock(&backend);
        let mut snapshot = Vec::with_capacity(selected.len());
        for (slot, &gpu_index) in selected.iter().enumerate() {
            let name = backend_guard.name(gpu_index).into_owned();
            snapshot.push(controllers[slot].tick(
                gpu_index,
                &name,
                &cfg,
                &mut *backend_guard,
                dt_s,
            ));
        }
        drop(backend_guard);
        *lock(&status) = snapshot;
        *lock(&heartbeat) = Instant::now();
    }

    // Every graceful exit hands the fans back to the driver.
    {
        let mut backend_guard = lock(&backend);
        for (slot, &gpu_index) in selected.iter().enumerate() {
            controllers[slot].restore(gpu_index, &mut *backend_guard);
        }
    }
    *lock(&heartbeat) = Instant::now();
    info!("control loop stopped; fans restored to driver control");
    Ok(())
}

fn handle_cmd(cmd: ServiceCmd, backend: &Mutex<NvapiBackend>) {
    let ServiceCmd::SetOcGlobal {
        gpu_index,
        delta_khz,
    } = cmd
    else {
        return; // Shutdown is handled by the loop before dispatch
    };
    if gpu_index > GPU_INDEX_MAX {
        error!("OC command rejected: gpu index {gpu_index} > {GPU_INDEX_MAX}");
        return;
    }
    let mut backend_guard = lock(backend);
    if gpu_index >= backend_guard.gpu_count() {
        error!(
            "OC command rejected: GPU {gpu_index} out of range (system has {})",
            backend_guard.gpu_count()
        );
        return;
    }
    match backend_guard.set_oc_global(gpu_index, delta_khz) {
        Ok(()) => info!("GPU {gpu_index}: OC delta {delta_khz} kHz applied"),
        Err(e) => error!("{e}"),
    }
}
