//! Consumer/session registry: **who may drive the control loop**.
//!
//! nvoc-srv is a single resident control plane. Multiple clients may want the
//! fan/frequency loop: the auto-optimizer's `--target-temp` scan, the CUDA
//! stressor's debug channel, and a human at the web console. Instead of the
//! client spawning (and owning) its own srv, each client registers here, holds
//! a lease, and declares a [`Claim`] (its control intent). The registry
//! arbitrates — the live session with the highest priority that holds a claim
//! is the **owner**, and the owner's claim is what gets applied to the shared
//! [`RuntimeConfig`]. When no claim remains, control is handed back to `Auto`.
//!
//! Leases expire (crash-safety), which is what replaces the old "the client
//! spawned the srv as a child, so the client shuts it down" ownership model:
//! a client that dies simply stops heartbeating and its claim lapses.
//!
//! Arbitration is deliberate and last-resort-visible: a human console action
//! is the reserved [`CONSOLE_ID`] session at top priority, so a person always
//! preempts automation; it carries a short lease so automation resumes soon
//! after. Broadcast a lower priority (e.g. the stressor debug channel) never
//! preempts a higher one (the optimizer).

use crate::config::ControlMode;
use crate::runtime::{SharedConfig, lock};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Lease window: a consumer must heartbeat within this period or its claim
/// (and the session) lapses. Chosen so a crashed client frees control within
/// a bounded time without a manual `/restore`.
pub const LEASE_TTL: Duration = Duration::from_secs(30);

/// Priority band for the MCP control-plane endpoint (an AI agent acting as a
/// consumer). Sits **below** the auto-optimizer's scan (35): the scan holds a
/// temperature-critical exclusive session for its entire duration and must not
/// be preempted mid-measurement, so an agent's advisory request yields to it.
/// Below the desktop GUI/TUI (40) and the human console, which preempt it.
pub const PRIORITY_MCP: i32 = 30;

/// Priority band for the desktop GUI/TUI acting as a consumer. Above the MCP
/// agent so the human's explicit operation wins, below the console so a
/// web-console human action still preempts.
pub const PRIORITY_DESKTOP: i32 = 40;

/// Reserved session id for human console operations.
pub const CONSOLE_ID: &str = "console";
/// Human console wins every arbitration.
const CONSOLE_PRIORITY: i32 = i32::MAX;
/// ...but only holds that preemption briefly, so automation resumes.
const CONSOLE_TTL: Duration = Duration::from_secs(30);

/// A consumer's declared control intent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Claim {
    pub mode: ControlMode,
    /// PID setpoint (°C) for a `Pid` claim; `None` leaves the shared setpoint.
    pub target_c: Option<f32>,
    /// Pinned duty for a `Manual` claim.
    pub manual_percent: Option<u32>,
    /// Monotonic ordering for tie-breaks (most recent claim wins).
    seq: u64,
}

impl Claim {
    /// A claim with an unset ordering; [`claim`] stamps the real sequence
    /// number, so callers never supply it.
    pub fn new(mode: ControlMode, target_c: Option<f32>, manual_percent: Option<u32>) -> Self {
        Self {
            mode,
            target_c,
            manual_percent,
            seq: 0,
        }
    }
}

#[derive(Debug)]
struct Session {
    name: String,
    priority: i32,
    expires_at: Instant,
    claim: Option<Claim>,
}

#[derive(Debug, Default)]
struct Registry {
    sessions: HashMap<String, Session>,
    /// Monotonic claim counter, for tie-breaking equal priorities.
    seq: u64,
    /// Whether the last applied state had an owner. Lets us restore `Auto`
    /// exactly on the owner→none transition without clobbering a startup
    /// `mode` configured in TOML before any consumer has acted.
    had_owner: bool,
}

static REGISTRY: std::sync::LazyLock<Mutex<Registry>> =
    std::sync::LazyLock::new(|| Mutex::new(Registry::default()));

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Serialize tests that touch the process-global registry. Both the session and
/// the MCP test modules mutate `REGISTRY`; without a shared lock one module's
/// `reset()` clears another module's sessions mid-assertion.
#[cfg(test)]
pub(crate) fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static T: Mutex<()> = Mutex::new(());
    T.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now() -> Instant {
    Instant::now()
}

/// Opaque, unguessable-enough session id (loopback-only; not a secret under
/// `auth=off`). 128 bits of time+counter+address entropy, hex-encoded.
fn fresh_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let stack = &nanos as *const _ as usize;
    let pid = std::process::id() as u128;
    format!(
        "{:016x}{:08x}{:08x}",
        nanos,
        pid,
        (nanos as usize ^ stack) as u32
    )
}

/// Serializable view for `GET /api/sessions`.
#[derive(Debug, Serialize)]
pub struct SessionView {
    pub id: String,
    pub name: String,
    pub priority: i32,
    pub mode: Option<ControlMode>,
    pub owner: bool,
    pub expires_in_s: u64,
}

/// Result of opening a session.
#[derive(Debug, Serialize)]
pub struct Opened {
    pub id: String,
    pub ttl_s: u64,
}

/// Open a new consumer session. No priority is privileged except the reserved
/// console one; callers pick their own band.
pub fn open(name: &str, priority: i32) -> Opened {
    let id = fresh_id();
    let mut reg = registry();
    reg.sessions.insert(
        id.clone(),
        Session {
            name: name.to_string(),
            priority,
            expires_at: now() + LEASE_TTL,
            claim: None,
        },
    );
    log::info!("session: opened {id} ({name:?}, priority {priority})");
    Opened {
        id,
        ttl_s: LEASE_TTL.as_secs(),
    }
}

/// Renew a session's lease. Returns whether it currently holds the claim.
pub fn heartbeat(id: &str) -> Result<bool, String> {
    let mut reg = registry();
    let s = reg
        .sessions
        .get_mut(id)
        .ok_or_else(|| format!("unknown session {id:?}"))?;
    s.expires_at = now() + LEASE_TTL;
    Ok(reg.owner_id().as_deref() == Some(id))
}

/// Declare/replace a session's control claim, then re-arbitrate.
pub fn claim(id: &str, claim: Claim) -> Result<(), String> {
    let mut reg = registry();
    reg.seq += 1;
    let seq = reg.seq;
    let s = reg
        .sessions
        .get_mut(id)
        .ok_or_else(|| format!("unknown session {id:?}"))?;
    s.expires_at = now() + LEASE_TTL;
    s.claim = Some(Claim { seq, ..claim });
    Ok(())
}

/// Drop a session's control claim *without* deregistering it — a consumer
/// yielding its band (e.g. the desktop GUI/TUI handing fan control back to the
/// driver) while staying registered so it can re-claim on the next operation.
/// The caller re-arbitrates with [`sweep_and_apply`], exactly as on close.
pub fn release(id: &str) -> Result<(), String> {
    let mut reg = registry();
    let s = reg
        .sessions
        .get_mut(id)
        .ok_or_else(|| format!("unknown session {id:?}"))?;
    s.claim = None;
    Ok(())
}

/// Drop a session and re-arbitrate (its claim is gone).
pub fn close(id: &str) -> Result<(), String> {
    let mut reg = registry();
    if reg.sessions.remove(id).is_some() {
        log::info!("session: closed {id}");
        Ok(())
    } else {
        Err(format!("unknown session {id:?}"))
    }
}

/// A human console action: create/refresh the reserved top-priority console
/// session and set its claim, so a person always preempts automation.
pub fn console_claim(mode: ControlMode, target_c: Option<f32>, manual_percent: Option<u32>) {
    let mut reg = registry();
    reg.seq += 1;
    let seq = reg.seq;
    reg.sessions.insert(
        CONSOLE_ID.to_string(),
        Session {
            name: "console".to_string(),
            priority: CONSOLE_PRIORITY,
            expires_at: now() + CONSOLE_TTL,
            claim: Some(Claim {
                mode,
                target_c,
                manual_percent,
                seq,
            }),
        },
    );
}

/// Expire lapsed sessions, re-arbitrate, and apply the resulting owner's
/// claim to `config`. Called after every session mutation and periodically by
/// the control loop (lease sweep). Returns the current owner id, if any.
pub fn sweep_and_apply(config: &SharedConfig) -> Option<String> {
    let mut reg = registry();
    let deadline = now();
    let before = reg.sessions.len();
    reg.sessions.retain(|_, s| s.expires_at > deadline);
    if reg.sessions.len() != before {
        log::info!(
            "session: {} lapsed session(s) expired",
            before - reg.sessions.len()
        );
    }
    reg.apply_owner(config)
}

impl Registry {
    /// Highest-priority live claim (tie → most recently declared).
    fn current_claim(&self) -> Option<(String, Claim)> {
        self.sessions
            .iter()
            .filter_map(|(id, s)| s.claim.map(|c| (id.clone(), s.priority, c)))
            .max_by(|a, b| {
                a.1.cmp(&b.1) // priority
                    .then(a.2.seq.cmp(&b.2.seq)) // then recency
            })
            .map(|(id, _, c)| (id, c))
    }

    fn owner_id(&self) -> Option<String> {
        self.current_claim().map(|(id, _)| id)
    }

    /// Apply the current owner's claim (or hand back to `Auto` on the
    /// owner→none transition). Assumes the registry lock is held; takes the
    /// config lock **inside** (lock order: registry → config).
    fn apply_owner(&mut self, config: &SharedConfig) -> Option<String> {
        let owner = self.current_claim();
        let mut cfg = lock(config);
        match &owner {
            Some((id, claim)) => {
                if cfg.mode != claim.mode {
                    log::info!("session: owner {id} sets mode {:?}", claim.mode);
                    cfg.mode = claim.mode;
                }
                if let Some(t) = claim.target_c
                    && cfg.pid.target_c != t
                {
                    log::info!("session: owner {id} sets target_c {t}");
                    cfg.pid.target_c = t;
                }
                if let Some(p) = claim.manual_percent
                    && cfg.manual_percent != p
                {
                    log::info!("session: owner {id} sets manual_percent {p}");
                    cfg.manual_percent = p;
                }
                self.had_owner = true;
            }
            None => {
                if self.had_owner && cfg.mode != ControlMode::Auto {
                    log::info!("session: no owner left; restoring Auto");
                    cfg.mode = ControlMode::Auto;
                }
                self.had_owner = false;
            }
        }
        owner.map(|(id, _)| id)
    }
}

/// Snapshot for `GET /api/sessions`. Expired sessions are pruned first.
pub fn snapshot() -> (Option<String>, Vec<SessionView>) {
    let mut reg = registry();
    let deadline = now();
    reg.sessions.retain(|_, s| s.expires_at > deadline);
    let owner = reg.owner_id();
    let views = reg
        .sessions
        .iter()
        .map(|(id, s)| {
            let expires_in_s = s.expires_at.saturating_duration_since(deadline).as_secs();
            SessionView {
                id: id.clone(),
                name: s.name.clone(),
                priority: s.priority,
                mode: s.claim.map(|c| c.mode),
                owner: owner.as_deref() == Some(id.as_str()),
                expires_in_s,
            }
        })
        .collect();
    (owner, views)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ControlMode, RuntimeConfig};
    use std::sync::{Arc, Mutex};

    fn cfg_shared() -> SharedConfig {
        Arc::new(Mutex::new(RuntimeConfig::default()))
    }

    fn claim_of(mode: ControlMode) -> Claim {
        Claim {
            mode,
            target_c: None,
            manual_percent: None,
            seq: 0,
        }
    }

    // Tests share the process-global registry; serialize them (and the MCP
    // tests, which touch the same registry) through the crate-wide lock.
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        test_guard()
    }

    fn reset() {
        let mut reg = registry();
        reg.sessions.clear();
        reg.had_owner = false;
    }

    #[test]
    fn owner_is_highest_priority_claim() {
        let _g = guard();
        reset();
        let config = cfg_shared();
        let low = open("stressor", 10).id;
        let high = open("optimizer", 20).id;
        claim(&low, claim_of(ControlMode::Manual)).unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Manual);
        claim(&high, claim_of(ControlMode::Pid)).unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Pid);
        // Removing the owner hands control to the next claim, not Auto.
        close(&high).unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Manual);
        close(&low).unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Auto, "no claims → restore");
    }

    #[test]
    fn lower_priority_never_preempts() {
        let _g = guard();
        reset();
        let config = cfg_shared();
        let high = open("optimizer", 20).id;
        let low = open("stressor", 10).id;
        claim(&high, claim_of(ControlMode::Pid)).unwrap();
        sweep_and_apply(&config);
        claim(&low, claim_of(ControlMode::Manual)).unwrap();
        sweep_and_apply(&config);
        assert_eq!(
            lock(&config).mode,
            ControlMode::Pid,
            "10 does not preempt 20"
        );
    }

    #[test]
    fn optimizer_band_beats_mcp() {
        let _g = guard();
        reset();
        let config = cfg_shared();
        let agent = open("mcp", PRIORITY_MCP).id;
        let scan = open("optimizer", 35).id;
        claim(&agent, claim_of(ControlMode::Manual)).unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Manual);
        // The scan's temperature-critical session outranks the agent.
        claim(&scan, claim_of(ControlMode::Pid)).unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Pid);
        // The agent cannot preempt the scan mid-measurement.
        claim(&agent, claim_of(ControlMode::Manual)).unwrap();
        sweep_and_apply(&config);
        assert_eq!(
            lock(&config).mode,
            ControlMode::Pid,
            "35 does not yield to 30"
        );
    }

    #[test]
    fn release_yields_band_but_keeps_session() {
        let _g = guard();
        reset();
        let config = cfg_shared();
        let scan = open("optimizer", 35).id;
        let gui = open("gui", PRIORITY_DESKTOP).id;
        claim(&scan, claim_of(ControlMode::Pid)).unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Pid);
        claim(&gui, claim_of(ControlMode::Manual)).unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Manual);
        // The desktop hands back: its claim is gone, control falls to the scan.
        release(&gui).unwrap();
        sweep_and_apply(&config);
        assert_eq!(
            lock(&config).mode,
            ControlMode::Pid,
            "scan resumes on yield"
        );
        // ...but the desktop session stays registered (and is not the owner).
        let (_, views) = snapshot();
        let view = views.iter().find(|v| v.id == gui).expect("session kept");
        assert!(!view.owner);
        assert!(view.mode.is_none());
        // Re-claiming preempts the scan again.
        claim(&gui, claim_of(ControlMode::Manual)).unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Manual);
    }

    #[test]
    fn desktop_band_beats_mcp_then_lapses() {
        let _g = guard();
        reset();
        let config = cfg_shared();
        let agent = open("mcp", PRIORITY_MCP).id;
        let human = open("gui", PRIORITY_DESKTOP).id;
        claim(&agent, claim_of(ControlMode::Pid)).unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Pid);
        // The desktop (human) band preempts the agent.
        claim(&human, claim_of(ControlMode::Manual)).unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Manual);
        // The agent never preempts the desktop band.
        claim(&agent, claim_of(ControlMode::Pid)).unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Manual);
        // A lapsed desktop lease restores (crash safety).
        registry().sessions.get_mut(&human).unwrap().expires_at = now() - Duration::from_secs(1);
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Pid, "agent claim remains");
        registry().sessions.get_mut(&agent).unwrap().expires_at = now() - Duration::from_secs(1);
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Auto);
    }

    #[test]
    fn console_preempts_then_lapses() {
        let _g = guard();
        reset();
        let config = cfg_shared();
        let opt = open("optimizer", 20).id;
        claim(&opt, claim_of(ControlMode::Pid)).unwrap();
        sweep_and_apply(&config);
        console_claim(ControlMode::Manual, None, Some(70));
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Manual);
        assert_eq!(lock(&config).manual_percent, 70);
        // Console lease lapses → the automated claim resumes.
        registry().sessions.get_mut(CONSOLE_ID).unwrap().expires_at =
            now() - Duration::from_secs(1);
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Pid);
    }

    #[test]
    fn expired_lease_releases_control() {
        let _g = guard();
        reset();
        let config = cfg_shared();
        let id = open("optimizer", 20).id;
        claim(&id, claim_of(ControlMode::Pid)).unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).mode, ControlMode::Pid);
        registry().sessions.get_mut(&id).unwrap().expires_at = now() - Duration::from_secs(1);
        sweep_and_apply(&config);
        assert_eq!(
            lock(&config).mode,
            ControlMode::Auto,
            "lapsed lease restores"
        );
    }

    #[test]
    fn unknown_session_errors() {
        let _g = guard();
        reset();
        assert!(heartbeat("nope").is_err());
        assert!(close("nope").is_err());
        assert!(claim("nope", claim_of(ControlMode::Pid)).is_err());
        assert!(release("nope").is_err());
    }

    #[test]
    fn pid_claim_carries_setpoint() {
        let _g = guard();
        reset();
        let config = cfg_shared();
        let id = open("optimizer", 20).id;
        claim(
            &id,
            Claim {
                mode: ControlMode::Pid,
                target_c: Some(68.0),
                manual_percent: None,
                seq: 0,
            },
        )
        .unwrap();
        sweep_and_apply(&config);
        assert_eq!(lock(&config).pid.target_c, 68.0);
    }
}
