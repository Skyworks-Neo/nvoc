//! Opt-in `--target-temp`: register with the **resident** nvoc-srv as a
//! control consumer for the duration of the scan.
//!
//! This does not spawn or own a srv process. It ensures a resident srv is
//! running (starting the installed service if needed), registers a session,
//! and claims closed-loop fan control at the requested setpoint. The session
//! is released from the cleanup path (both success and error); if the process
//! dies outright, the srv lease lapses on its own and control is handed back.

use anyhow::{Result, anyhow};
use clap::ArgMatches;
use nvoc_srv_client::{DEFAULT_PORT, PRIORITY_OPTIMIZER, Session, ensure_resident};
use std::sync::Mutex;

/// The live session, held for the whole scan. `None` when `--target-temp` is
/// not in play.
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<Session>> {
    SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Engage the thermal session if `--target-temp` was requested. No-op
/// otherwise, so the flag stays strictly opt-in.
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
    let port = matches
        .get_one::<u16>("srv_port")
        .copied()
        .unwrap_or(DEFAULT_PORT);

    let mut guard = lock();
    if guard.is_some() {
        return Ok(()); // already engaged (nested optimize workflow)
    }
    ensure_resident(port).map_err(|e| anyhow!("{e}"))?;
    let session = Session::open(port, "nvoc-auto-optimizer", PRIORITY_OPTIMIZER)?;
    let owner = session.claim("pid", Some(target_c), None)?;
    if owner {
        eprintln!("nvoc-srv on port {port}: registered; holding {target_c} °C for the scan");
    } else {
        eprintln!(
            "nvoc-srv on port {port}: registered, but a higher-priority consumer currently \
             holds control"
        );
    }
    *guard = Some(session);
    Ok(())
}

/// Release the session, handing control back to the srv's arbitration. Safe
/// to call when no session is active. Call this from the cleanup path on
/// every exit, success and error alike.
pub fn finish() {
    let mut guard = lock();
    if let Some(mut session) = guard.take() {
        session.close();
        eprintln!("nvoc-srv: thermal session released");
    }
}
