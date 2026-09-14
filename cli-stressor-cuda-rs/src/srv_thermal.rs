//! Debug channel: opt-in thermal session (`--target-temp`) on the
//! nvoc-srv control plane, so the stressor alone can exercise closed-loop
//! fan control before the formal optimizer-side integration is trusted.
//!
//! Self-contained by design — this module does not share code with the
//! optimizer's session manager. Lifecycle: probe `/status` (adopt a healthy
//! srv; spawn `nvoc-srv --foreground` as a child when nothing listens;
//! hard-error on a port occupied by something else), wait for readiness,
//! push the setpoint, engage PID mode. [`exit`] routes every stressor
//! termination through teardown: `POST /restore`, and — only for a spawned
//! child — `POST /shutdown` + wait/kill. Debug-grade: a hard crash (TDR,
//! abort) leaves the spawned srv orphaned but safe (it keeps controlling
//! with its watchdog); the next `--target-temp` run reuses it, and the web
//! UI at `/` can always `/restore` or `/shutdown` it.

use clap::Args as _Args;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Child;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const IO_TIMEOUT: Duration = Duration::from_secs(3);
const READY_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy)]
struct Opts {
    target_c: f32,
    port: u16,
}

#[derive(Debug)]
struct Session {
    opts: Opts,
    spawned: bool,
    child: Option<Child>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn session_lock() -> std::sync::MutexGuard<'static, Option<Session>> {
    SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// CLI surface for the debug channel.
#[derive(_Args, Debug, Clone)]
pub struct ThermalArgs {
    /// Hold the GPU at this temperature via a local nvoc-srv fan control
    /// loop (debug: spawns/reuses 127.0.0.1 srv for the run). Ignored when
    /// running as an nvoc-auto-optimizer worker — the optimizer owns the
    /// session then.
    #[arg(long, value_name = "TEMP_C")]
    pub target_temp: Option<f32>,
    /// nvoc-srv control-plane port for --target-temp (default 14514).
    #[arg(long, value_name = "PORT")]
    pub srv_port: Option<u16>,
    /// nvoc-srv binary to spawn when no srv is running
    /// (default: next to this executable).
    #[arg(long, value_name = "PATH")]
    pub srv_exe: Option<String>,
}

/// Engage the thermal session when `--target-temp` is present. No-op
/// without it; warns and skips in optimizer-worker mode (the optimizer owns
/// the session there).
pub fn engage(args: &ThermalArgs, worker_mode: bool) {
    let Some(target_c) = args.target_temp else {
        return;
    };
    if worker_mode {
        eprintln!(
            "warning: --target-temp ignored in optimizer-worker mode; \
             the optimizer owns the thermal session"
        );
        return;
    }
    let opts = Opts {
        target_c,
        port: args.srv_port.unwrap_or(14514),
    };
    let client = Client { port: opts.port };
    let mut session = match client.probe_state() {
        Ok(Probe::Ready) => {
            eprintln!("nvoc-srv on port {0}: adopting for this run", opts.port);
            Session {
                opts,
                spawned: false,
                child: None,
            }
        }
        Ok(Probe::Starting) => {
            eprintln!("nvoc-srv on port {0} is starting; waiting", opts.port);
            Session {
                opts,
                spawned: false,
                child: None,
            }
        }
        Ok(Probe::Absent) => {
            let exe = args
                .srv_exe
                .clone()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::current_exe()
                        .unwrap_or_else(|_| std::path::PathBuf::from("nvoc-srv"))
                        .with_file_name("nvoc-srv.exe")
                });
            match std::process::Command::new(&exe)
                .arg("--foreground")
                .arg("--port")
                .arg(opts.port.to_string())
                .arg("--target-c")
                .arg(opts.target_c.to_string())
                .spawn()
            {
                Ok(child) => {
                    eprintln!(
                        "spawned {} --foreground --target-c {}",
                        exe.display(),
                        opts.target_c
                    );
                    Session {
                        opts,
                        spawned: true,
                        child: Some(child),
                    }
                }
                Err(e) => {
                    eprintln!(
                        "warning: cannot spawn {} ({e}); continuing WITHOUT thermal control \
                         (install nvoc-srv or pass --srv-exe)",
                        exe.display()
                    );
                    return;
                }
            }
        }
        Err(message) => {
            eprintln!(
                "warning: port {} is occupied by something that is not a healthy nvoc-srv \
                 ({message}); continuing WITHOUT thermal control",
                opts.port
            );
            return;
        }
    };

    // Wait for readiness, then engage.
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        match client.probe_state() {
            Ok(Probe::Ready) => break,
            _ if Instant::now() >= deadline => {
                eprintln!(
                    "warning: nvoc-srv never became ready; continuing WITHOUT thermal control"
                );
                if session.spawned {
                    let _ = client.post("/shutdown");
                    if let Some(child) = session.child.as_mut() {
                        let _ = child.wait();
                    }
                }
                return;
            }
            _ => std::thread::sleep(PROBE_INTERVAL),
        }
    }
    if client
        .post(&format!("/pid?target_c={}", opts.target_c))
        .and_then(|_| client.post("/mode?value=pid"))
        .is_err()
    {
        eprintln!(
            "warning: could not engage nvoc-srv PID mode; continuing WITHOUT thermal control"
        );
        return;
    }
    eprintln!(
        "thermal session active: nvoc-srv PID target {} °C",
        opts.target_c
    );
    *session_lock() = Some(session);

    // Best-effort teardown on Ctrl-C (hard crashes leave the spawned srv
    // orphaned-but-safe; see module docs).
    std::thread::Builder::new()
        .name("srv-thermal-ctrlc".into())
        .spawn(|| {
            if ctrlc::set_handler(|| exit(130)).is_err() {
                eprintln!("warning: ctrl-c teardown unavailable; use the web UI to restore");
            }
        })
        .ok();
}

/// Termination funnel: best-effort teardown, then exit. Safe to call when
/// no session exists and from any thread.
pub fn exit(code: i32) -> ! {
    teardown();
    std::process::exit(code);
}

/// Teardown without exiting: restore fan control; stop a spawned child.
pub fn teardown() {
    let mut guard = session_lock();
    let Some(mut session) = guard.take() else {
        return;
    };
    let client = Client {
        port: session.opts.port,
    };
    if let Err(e) = client.post("/restore") {
        eprintln!("warning: thermal teardown restore failed: {e}");
    }
    if session.spawned {
        let requested = client.post("/shutdown");
        let exited = requested.is_ok_and(|_| {
            let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
            loop {
                match session.child.as_mut().map(|c| c.try_wait()) {
                    None | Some(Ok(Some(_))) => return true,
                    Some(Ok(None)) if Instant::now() < deadline => {
                        std::thread::sleep(PROBE_INTERVAL);
                    }
                    _ => return false,
                }
            }
        });
        if !exited {
            eprintln!("warning: spawned nvoc-srv did not exit in time; killing");
            if let Some(child) = session.child.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        eprintln!("thermal session closed (spawned nvoc-srv stopped)");
    } else {
        eprintln!("thermal session closed (pre-existing nvoc-srv restored to auto)");
    }
}

// ---------------------------------------------------------------------------
// Minimal loopback HTTP client (independent of the optimizer's copy).
// ---------------------------------------------------------------------------

struct Client {
    port: u16,
}

enum Probe {
    Ready,
    Starting,
    Absent,
}

impl Client {
    fn post(&self, path_and_query: &str) -> Result<String, String> {
        let (code, body) = self.request("POST", path_and_query)?;
        if code < 400 {
            Ok(body)
        } else {
            Err(format!("srv returned {code}: {}", body.trim()))
        }
    }

    fn probe_state(&self) -> Result<Probe, String> {
        match self.request("GET", "/status") {
            Err(message) if message.contains("connection refused") => Ok(Probe::Absent),
            Err(message) => Err(message),
            Ok((code, body)) => {
                if code >= 400 {
                    return Err(format!("srv returned {code}: {body}"));
                }
                let ok = serde_json::from_str::<serde_json::Value>(&body)
                    .map_err(|e| format!("srv /status is not JSON: {e}"))?;
                let gpus = ok
                    .get("gpus")
                    .and_then(|g| g.as_array())
                    .ok_or_else(|| "srv /status missing 'gpus'".to_string())?;
                Ok(if gpus.is_empty() {
                    Probe::Starting
                } else {
                    Probe::Ready
                })
            }
        }
    }

    fn request(&self, method: &str, path_and_query: &str) -> Result<(u16, String), String> {
        let csrf = if method == "POST" {
            "X-Requested-With: XMLHttpRequest\r\n"
        } else {
            ""
        };
        let request = format!(
            "{method} {path_and_query} HTTP/1.1\r\n\
             Host: 127.0.0.1:{0}\r\n\
             {csrf}\
             Connection: close\r\n\
             Content-Length: 0\r\n\
             \r\n",
            self.port
        );
        let mut stream =
            TcpStream::connect(("127.0.0.1", self.port)).map_err(|e| format!("connect: {e}"))?;
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .map_err(|e| format!("timeout: {e}"))?;
        stream
            .write_all(request.as_bytes())
            .map_err(|e| format!("write: {e}"))?;
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .map_err(|e| format!("read: {e}"))?;
        let raw = String::from_utf8_lossy(&buf).into_owned();
        let (head, body) = raw
            .split_once("\r\n\r\n")
            .ok_or_else(|| "response has no header/body separator".to_string())?;
        let status_line = head.lines().next().unwrap_or_default();
        let code: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("no status line: {status_line:?}"))?;
        Ok((code, body.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(target: Option<f32>) -> ThermalArgs {
        ThermalArgs {
            target_temp: target,
            srv_port: None,
            srv_exe: None,
        }
    }

    #[test]
    fn no_target_temp_is_a_noop() {
        *session_lock() = None;
        engage(&args(None), false);
        engage(&args(None), true);
        assert!(session_lock().is_none(), "no session may be recorded");
        teardown(); // must also be a safe noop
    }

    #[test]
    fn worker_mode_ignores_target_temp() {
        *session_lock() = None;
        // Worker mode must not spawn/probe even with a setpoint: the
        // optimizer owns the session there. (Port 1 is never a srv; if the
        // guard regresses, the probe fails fast as Absent → but engage must
        // return before any of that.)
        engage(&args(Some(70.0)), true);
        assert!(session_lock().is_none(), "worker mode must not engage");
    }

    #[test]
    fn exit_without_session_is_safe() {
        *session_lock() = None;
        teardown();
    }
}
