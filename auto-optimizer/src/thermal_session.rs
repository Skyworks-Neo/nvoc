//! Stress-run thermal session: a `--target-temp`-requested fan PID control
//! session on the nvoc-srv control plane, owned by the optimizer process for
//! the whole scan (it outlives every short-lived stressor round).
//!
//! Lifecycle (probe-first):
//! 1. [`ensure`] at scan start — if a healthy srv answers, adopt it
//!    (restore-only at teardown); if nothing listens, spawn
//!    `nvoc-srv --foreground` as a child, wait for readiness, then push
//!    the setpoint and switch to PID mode. A port occupied by something
//!    that is *not* srv is a hard error (spawning would only add a blind
//!    second instance).
//! 2. [`finish`] from `cleanup_autoscan_exit` (both success and error
//!    paths) — `POST /restore` hands the fan back to the driver; a spawned
//!    child is shut down, a pre-existing service is left running.

use crate::srv_client::{ProbeState, SrvClient};
use anyhow::{Result, anyhow};
use clap::ArgMatches;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

const PROBE_INTERVAL: Duration = Duration::from_millis(250);
/// Wait for the control loop after spawning (or after adopting an srv that
/// was still in its discovery window).
const READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Graceful shutdown window for a spawned child before we kill it.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Default)]
struct SessionState {
    opts: Option<SessionOpts>,
    child: Mutex<Option<Child>>,
    /// True when *we* spawned the srv process (vs. adopting a pre-existing
    /// one): only then is it ours to shut down.
    spawned: bool,
}

#[derive(Debug, Clone, Copy)]
struct SessionOpts {
    port: u16,
}

static SESSION: Mutex<SessionState> = Mutex::new(SessionState {
    opts: None,
    child: Mutex::new(None),
    spawned: false,
});

fn session_lock() -> MutexGuard<'static, SessionState> {
    SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Read the `--target-temp` / `--srv-port` / `--srv-exe` group and engage
/// the thermal session. No-op when `--target-temp` is absent (opt-in).
pub fn ensure(matches: &ArgMatches) -> Result<()> {
    let Some(raw) = matches.get_one::<String>("target_temp") else {
        return Ok(());
    };
    let target_c: f32 = raw
        .parse()
        .map_err(|_| anyhow!("--target-temp must be a number (°C), got {raw:?}"))?;
    if !(30.0..=110.0).contains(&target_c) {
        return Err(anyhow!("--target-temp must be 30–110 °C, got {target_c}"));
    }
    // 定义侧是 value_parser!(u16)（arg_help.rs），读取必须同为 u16——
    // 用 String downcast 会在真的传了 --srv-port 时 panic（缺省时
    // get_one 返回 None 不触碰值类型，所以默认端口路径测不出来）。
    let port = matches
        .get_one::<u16>("srv_port")
        .copied()
        .unwrap_or(crate::srv_client::DEFAULT_PORT);

    let mut state = session_lock();
    if state.opts.is_some() {
        return Ok(()); // already ensured (nested optimize workflow)
    }

    let client = SrvClient::new(port);
    state.opts = Some(SessionOpts { port });
    match client.probe_state() {
        Ok(ProbeState::Ready) => {
            // Adopt the pre-existing control plane; restore-only teardown.
            eprintln!("nvoc-srv on port {port}: adopting for the scan session");
            state.spawned = false;
        }
        Ok(ProbeState::Starting) => {
            // srv is up but its control loop is still discovering; adopt and
            // wait for readiness below.
            eprintln!("nvoc-srv on port {port} is starting; waiting for readiness");
            state.spawned = false;
        }
        Ok(ProbeState::Absent) => {
            // Nothing listens: spawn a foreground srv as our child.
            let exe = matches
                .get_one::<String>("srv_exe")
                .map(PathBuf::from)
                .unwrap_or_else(default_srv_exe);
            let mut command = std::process::Command::new(&exe);
            command
                .arg("--foreground")
                .arg("--port")
                .arg(port.to_string())
                .arg("--target-c")
                .arg(target_c.to_string());
            let child = command.spawn().map_err(|e| {
                state.opts = None; // nothing engaged — no teardown needed
                anyhow!(
                    "cannot spawn {} ({e}); install nvoc-srv or pass --srv-exe",
                    exe.display()
                )
            })?;
            eprintln!(
                "no nvoc-srv on port {port}; spawned {} --foreground --target-c {target_c}",
                exe.display()
            );
            *state.child.lock().expect("session child mutex") = Some(child);
            state.spawned = true;
        }
        Err(probe_err) => {
            state.opts = None;
            return Err(anyhow!(
                "port {port} is occupied by something that is not a healthy nvoc-srv \
                 ({probe_err}); free the port or pass --srv-port"
            ));
        }
    }

    // Wait for the control loop to be live — covers both the discovery
    // window of an adopted srv and the startup of our spawned child.
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        match client.probe_state() {
            Ok(ProbeState::Ready) => break,
            _ if Instant::now() >= deadline => {
                let msg = if state.spawned {
                    "spawned nvoc-srv did not become ready in time"
                } else {
                    "adopted nvoc-srv never reported a live control loop"
                };
                return Err(ensure_failure(state, &client, msg));
            }
            _ => std::thread::sleep(PROBE_INTERVAL),
        }
    }

    // Push the setpoint and engage the loop. Mode last: it flips on with
    // the target already in place.
    if let Err(e) = client
        .post(&format!("/pid?target_c={target_c}"))
        .and_then(|_| client.post("/mode?value=pid"))
    {
        return Err(ensure_failure(state, &client, &e.to_string()));
    }
    eprintln!("thermal session active: nvoc-srv PID target {target_c} °C");
    Ok(())
}

/// Common failure path: the session is already recorded in the global
/// state, so `finish()` still restores fans; surface the error.
fn ensure_failure(
    mut state: MutexGuard<'static, SessionState>,
    client: &SrvClient,
    msg: &str,
) -> anyhow::Error {
    let _ = client.post("/restore");
    if state.spawned {
        kill_child(&mut state);
    }
    state.opts = None;
    anyhow!("thermal session failed: {msg}")
}

/// Teardown from `cleanup_autoscan_exit`: hand the fan back to the driver
/// and (only for a spawned child) stop the service. Best-effort — cleanup
/// must never mask the scan result.
pub fn finish() {
    let mut state = session_lock();
    let Some(opts) = state.opts.take() else {
        return;
    };
    let client = SrvClient::new(opts.port);
    if let Err(e) = client.post("/restore") {
        eprintln!("warning: thermal session restore failed: {e}");
    }
    if state.spawned {
        let requested = client.post("/shutdown");
        let exited = requested.is_ok() && wait_child_exit(&mut state, SHUTDOWN_TIMEOUT);
        if !exited {
            eprintln!("warning: spawned nvoc-srv did not exit cleanly; killing");
            kill_child(&mut state);
        }
        eprintln!("thermal session closed (spawned nvoc-srv stopped)");
    } else {
        eprintln!("thermal session closed (pre-existing nvoc-srv restored to auto)");
    }
}

fn wait_child_exit(state: &mut SessionState, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let mut guard = state.child.lock().expect("session child mutex");
        let Some(child) = guard.as_mut() else {
            return true;
        };
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => {
                drop(guard);
                std::thread::sleep(PROBE_INTERVAL);
            }
            _ => return false,
        }
    }
}

fn kill_child(state: &mut SessionState) {
    if let Some(child) = state.child.lock().expect("session child mutex").as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    *state.child.lock().expect("session child mutex") = None;
}

/// The optimizer ships alongside `nvoc-srv.exe` (same convention as
/// nvoc-srv-ctl's sibling-binary install).
fn default_srv_exe() -> PathBuf {
    std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("nvoc-srv"))
        .with_file_name("nvoc-srv.exe")
}

/// Test seam: reset the global session between unit tests.
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *session_lock() = SessionState::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_without_opt_in_is_noop() {
        reset_for_tests();
        let app = clap::Command::new("t").arg(
            clap::Arg::new("target_temp")
                .long("target-temp")
                .num_args(1),
        );
        let matches = app.try_get_matches_from(["t"]).expect("parse");
        ensure(&matches).expect("noop");
        finish(); // must also be a safe noop
    }

    #[test]
    fn target_temp_range_is_validated_before_any_side_effect() {
        reset_for_tests();
        let app = clap::Command::new("t").arg(
            clap::Arg::new("target_temp")
                .long("target-temp")
                .num_args(1),
        );
        let matches = app
            .try_get_matches_from(["t", "--target-temp", "200"])
            .expect("parse");
        let err = ensure(&matches).unwrap_err().to_string();
        assert!(err.contains("30–110"), "{err}");
        assert!(session_lock().opts.is_none(), "no session may be recorded");
    }

    #[test]
    fn real_cli_definition_matches_u16_srv_port_access() {
        // 钉死"定义 == 读取"的类型契约：arg_help.rs 用 value_parser!(u16)
        // 定义 --srv-port，本模块读取也必须是 get_one::<u16>。T-B 现场故障
        // （Mismatch between definition and access of `srv_port`）的回归点：
        // 任何一侧改类型，这条测试都会在 downcast 处直接 panic。
        reset_for_tests();
        let cmd = crate::arg_help::get_arguments();
        let matches = cmd
            .try_get_matches_from([
                "nvoc-auto-optimizer",
                "optimize",
                "--target-temp",
                "70",
                "--srv-port",
                "14515",
            ])
            .expect("real CLI definition must parse --target-temp/--srv-port");
        let sub = matches
            .subcommand_matches("optimize")
            .expect("optimize subcommand");
        assert_eq!(sub.get_one::<u16>("srv_port"), Some(&14515));
    }

    #[test]
    fn unhealthy_port_is_a_hard_error_not_a_spawn() {
        reset_for_tests();
        // Occupy the port with a plain TCP listener: not a srv.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let app = clap::Command::new("t")
            .arg(
                clap::Arg::new("target_temp")
                    .long("target-temp")
                    .num_args(1),
            )
            // 与 arg_help.rs 的真实定义保持一致：value_parser!(u16)。
            // 回归点：若读取侧退回 get_one::<String>，传了 --srv-port 的
            // 这条用例会在 downcast 处 panic，直接复现 T-B 现场故障。
            .arg(
                clap::Arg::new("srv_port")
                    .long("srv-port")
                    .num_args(1)
                    .value_parser(clap::value_parser!(u16)),
            );
        let matches = app
            .try_get_matches_from(["t", "--target-temp", "70", "--srv-port", &port.to_string()])
            .expect("parse");
        let err = ensure(&matches).unwrap_err().to_string();
        assert!(err.contains("occupied"), "{err}");
        let state = session_lock();
        assert!(state.opts.is_none());
        assert!(!state.spawned);
        assert!(state.child.lock().expect("child mutex").is_none());
    }
}
