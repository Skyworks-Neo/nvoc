//! Consumer session: register with the resident srv, declare a control claim,
//! and keep the lease alive.
//!
//! The srv owns the control loop; a client's job is only to *register* and
//! say what it wants. [`Session::open`] registers and starts a background
//! heartbeat so the lease never lapses while the client is alive; dropping
//! the session (or an explicit [`Session::close`]) deregisters, which is how
//! the srv learns to hand control back. A crashed client simply stops
//! heartbeating and its claim lapses on the srv side — no child-process
//! teardown to get right.

use crate::http::SrvClient;
use anyhow::{Result, anyhow};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

/// Renew well inside the srv lease window (30 s) so a single missed beat is
/// harmless.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

pub struct Session {
    id: String,
    port: u16,
    /// Dropping this wakes the heartbeat thread's `recv_timeout`.
    beat: Option<Sender<()>>,
    hb: Option<JoinHandle<()>>,
    closed: Arc<AtomicBool>,
}

impl Session {
    /// Register a consumer and start the lease heartbeat.
    pub fn open(port: u16, name: &str, priority: i32) -> Result<Self> {
        let client = SrvClient::new(port);
        let body = client.post(&format!(
            "/api/session/open?name={}&priority={priority}",
            percent_encode(name)
        ))?;
        let id = serde_json::from_str::<serde_json::Value>(&body)
            .map_err(|e| anyhow!("srv session/open returned non-JSON: {e}"))?
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("srv session/open response missing 'id'"))?;

        let (beat, rx) = mpsc::channel::<()>();
        let stop = Arc::new(AtomicBool::new(false));
        let hb = {
            let stop = stop.clone();
            let hb_client = SrvClient::new(port);
            let hb_id = id.clone();
            std::thread::Builder::new()
                .name("nvoc-srv-heartbeat".into())
                .spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match rx.recv_timeout(HEARTBEAT_INTERVAL) {
                            // Stop signalled (channel closed) — exit.
                            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                            Err(RecvTimeoutError::Timeout) => {
                                if let Err(e) =
                                    hb_client.post(&format!("/api/session/heartbeat?id={hb_id}"))
                                {
                                    log::warn!("srv session heartbeat failed: {e}");
                                }
                            }
                        }
                    }
                })
                .expect("spawn heartbeat thread")
        };

        log::info!("registered with nvoc-srv on port {port} as {name:?} (session {id})");
        Ok(Self {
            id,
            port,
            beat: Some(beat),
            hb: Some(hb),
            closed: stop,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Declare (or replace) this consumer's control claim. Returns whether
    /// this consumer is now the control owner (the srv arbitrates by
    /// priority, so a lower-priority claim may not win).
    pub fn claim(
        &self,
        mode: &str,
        target_c: Option<f32>,
        manual_percent: Option<u32>,
    ) -> Result<bool> {
        let mut q = format!("/api/session/claim?id={}&mode={}", self.id, mode);
        if let Some(t) = target_c {
            q.push_str(&format!("&target_c={t}"));
        }
        if let Some(p) = manual_percent {
            q.push_str(&format!("&manual_percent={p}"));
        }
        let body = SrvClient::new(self.port).post(&q)?;
        let v: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| anyhow!("srv claim returned non-JSON: {e}"))?;
        Ok(v.get("owner").and_then(|o| o.as_bool()).unwrap_or(false))
    }

    /// Hand the control band back without deregistering: the session's claim is
    /// dropped (control falls to the next claim, or `Auto` if none remain) but
    /// the session stays registered and heartbeating, so it can re-claim later.
    pub fn release(&self) -> Result<()> {
        SrvClient::new(self.port).post(&format!("/api/session/release?id={}", self.id))?;
        Ok(())
    }

    /// Deregister and stop the heartbeat. Idempotent.
    pub fn close(&mut self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return; // already closed
        }
        // Dropping the sender closes the channel → the heartbeat thread exits.
        self.beat = None;
        if let Some(hb) = self.hb.take() {
            let _ = hb.join();
        }
        if let Err(e) =
            SrvClient::new(self.port).post(&format!("/api/session/close?id={}", self.id))
        {
            log::warn!("srv session close failed: {e}");
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

/// Percent-encode a query value (RFC 3986 unreserved set is left alone).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding() {
        assert_eq!(percent_encode("nvoc-auto-optimizer"), "nvoc-auto-optimizer");
        assert_eq!(percent_encode("a b&c"), "a%20b%26c");
    }
}
