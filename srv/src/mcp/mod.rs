//! MCP control-plane endpoint: nvoc-srv's control API exposed as Model
//! Context Protocol tools, so an AI agent can act as a first-class consumer.
//!
//! The transport is **streamable HTTP on a loopback port** (never stdio — srv
//! is a resident service with no per-client child to attach a stdio server to).
//! `rmcp` is async-only, so the listener runs on its own dedicated tokio
//! runtime inside one supervised std thread ([`spawn_supervised`]), mirroring
//! [`crate::http::spawn_supervised`]. The async runtime never touches the
//! synchronous control loop; it reaches shared state only through the existing
//! `Arc<Mutex<…>>` handles, with no mutex guard held across an `.await`.
//!
//! **Identity.** An agent is *not* privileged: control tools bind the calling
//! MCP session to an ordinary [`crate::session`] consumer in the
//! [`crate::session::PRIORITY_MCP`] band (below the auto-optimizer's
//! temperature-critical scan and the desktop GUI/TUI, below the human console).
//! The claim is arbitrated by the same registry and bounded by
//! the same lease, so an agent that dies cannot leave a fan pinned — its lease
//! lapses and the control loop restores `auto`.

use crate::http::ServerState;
use crate::runtime::SharedConfig;
use crate::session;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

mod handler;
mod serve;

/// How often a bound MCP session renews its nvoc lease.
const HEARTBEAT: Duration = Duration::from_secs(10);
/// Idle window: an MCP session that issues no tool call for this long is
/// released and its nvoc session closed. Matches the consumer lease so an
/// abandoned agent frees control on the same timescale as any other consumer.
const IDLE_RELEASE: Duration = session::LEASE_TTL;

/// Spawn the MCP listener on a dedicated thread with its own tokio runtime.
/// Supervised like the HTTP plane: on a clean exit the runtime is rebuilt and
/// the listener restarted (release builds abort on panic — SCM/systemd failure
/// actions are the backstop there, as for the HTTP plane).
pub fn spawn_supervised(state: Arc<ServerState>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("nvoc-mcp".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("nvoc-mcp-rt")
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("mcp: failed to build tokio runtime: {e}");
                    return;
                }
            };
            loop {
                rt.block_on(serve::serve_once(state.clone()));
                log::warn!("mcp: endpoint exited; restarting in 1 s");
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .expect("spawn MCP thread")
}

/// One MCP conversation mapped to an nvoc consumer session.
struct McpSession {
    nvoc_id: String,
    last_activity: Mutex<Instant>,
}

/// Live MCP sessions, keyed by the MCP transport session id. A process-global
/// mirrors [`crate::session`]'s own registry.
static SESSIONS: LazyLock<Mutex<HashMap<String, Arc<McpSession>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn sessions() -> std::sync::MutexGuard<'static, HashMap<String, Arc<McpSession>>> {
    SESSIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Bind (or renew) the nvoc consumer session for this MCP session id and return
/// its id. The first call opens the session in the MCP band and starts a lease
/// heartbeat; later calls just refresh activity.
pub fn ensure_session(config: &SharedConfig, mcp_id: &str) -> String {
    let mut map = sessions();
    if let Some(s) = map.get(mcp_id) {
        *s.last_activity.lock().unwrap_or_else(|p| p.into_inner()) = Instant::now();
        return s.nvoc_id.clone();
    }
    let opened = session::open("mcp", session::PRIORITY_MCP);
    let s = Arc::new(McpSession {
        nvoc_id: opened.id.clone(),
        last_activity: Mutex::new(Instant::now()),
    });
    map.insert(mcp_id.to_string(), s.clone());
    spawn_lease_heartbeat(mcp_id.to_string(), s, config.clone());
    opened.id
}

/// Renew the nvoc lease while the MCP session is active, then release it.
///
/// The thread is self-terminating: it stops renewing once no tool call has
/// arrived within [`IDLE_RELEASE`], or once the nvoc session has vanished. On
/// exit it closes the nvoc session (restoring control if this session was the
/// owner) and drops itself from [`SESSIONS`]. A client crash therefore releases
/// control within [`IDLE_RELEASE`]; the nvoc lease is the hard backstop if this
/// thread somehow stalls.
fn spawn_lease_heartbeat(mcp_id: String, s: Arc<McpSession>, config: SharedConfig) {
    let _ = std::thread::Builder::new()
        .name("nvoc-mcp-lease".into())
        .spawn(move || {
            loop {
                std::thread::sleep(HEARTBEAT);
                let idle = s
                    .last_activity
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .elapsed();
                if idle > IDLE_RELEASE {
                    log::info!("mcp: session {mcp_id} idle {idle:?}; releasing nvoc lease");
                    break;
                }
                if session::heartbeat(&s.nvoc_id).is_err() {
                    break;
                }
            }
            let _ = session::close(&s.nvoc_id);
            sessions().remove(&mcp_id);
            // Apply the ownership change immediately rather than waiting for
            // the control loop's next periodic sweep.
            let _ = session::sweep_and_apply(&config);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RuntimeConfig;
    use crate::http::ServerState;
    use std::io::{Read, Write};

    fn test_state() -> Arc<ServerState> {
        let handles = crate::runtime::setup(RuntimeConfig::default());
        Arc::new(ServerState {
            config: handles.config.clone(),
            status: handles.status.clone(),
            backend: handles.backend.clone(),
            history: handles.history.clone(),
            cmd_tx: handles.cmd_tx.clone(),
        })
    }

    /// First call opens a session in the MCP band; a repeat call for the same
    /// MCP session returns the same nvoc id (renewal, not a new session).
    #[test]
    fn ensure_session_is_idempotent_per_mcp_session() {
        // This test reads the process-global consumer registry that the session
        // tests also mutate; take the crate-wide test lock.
        let _g = crate::session::test_guard();
        let state = test_state();
        let cfg = state.config.clone();
        let a = ensure_session(&cfg, "test-session-a");
        let b = ensure_session(&cfg, "test-session-a");
        assert_eq!(a, b, "same MCP session reuses its nvoc session");
        let c = ensure_session(&cfg, "test-session-b");
        assert_ne!(a, c, "different MCP sessions get distinct nvoc sessions");
        assert!(sessions().contains_key("test-session-a"));
        // The registered consumer carries the MCP priority band.
        let (_, views) = session::snapshot();
        let view = views.iter().find(|v| v.id == a).expect("session listed");
        assert_eq!(view.priority, session::PRIORITY_MCP);
        assert_eq!(view.name, "mcp");
    }

    /// The endpoint answers a real MCP `initialize` handshake (proves the
    /// axum+rmcp wiring, not just the handler logic).
    #[test]
    fn initialize_handshake_over_http() {
        let state = test_state();
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        std_listener.set_nonblocking(true).expect("nonblocking");
        let addr = std_listener.local_addr().expect("local_addr");
        let listener = rt
            .block_on(async { tokio::net::TcpListener::from_std(std_listener) })
            .expect("tokio listener");
        rt.spawn(super::serve::serve_on(listener, state));

        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
        let request = format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{}",
            addr.port(),
            body.len(),
            body
        );
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        stream.write_all(request.as_bytes()).expect("write request");
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        assert!(response.contains("200"), "unexpected response: {response}");
        assert!(
            response.contains("serverInfo") || response.contains("protocolVersion"),
            "no initialize result in response: {response}"
        );
    }
}
