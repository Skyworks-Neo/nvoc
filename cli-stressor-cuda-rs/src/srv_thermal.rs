//! Opt-in `--srv-target-temp`: register with the **resident** nvoc-srv as a
//! debug thermal consumer while a standalone stress run is in flight.
//!
//! The optimizer's own `--target-temp` scan does not use this path: it holds a
//! higher-priority claim (`PRIORITY_OPTIMIZER`) for the whole scan and does not
//! forward the flag to its bundled stress worker. This channel exists so the
//! stressor can be driven on its own and pin a cooling setpoint while it hammers
//! the GPU, at the lower `PRIORITY_STRESSOR` band.
//!
//! The session is released from the run's normal tail; if the process dies
//! outright the srv lease lapses on its own and control is handed back.

use nvoc_srv_client::{DEFAULT_PORT, PRIORITY_STRESSOR, Session, ensure_resident};
use std::sync::Mutex;

/// The live session, held for the whole stress run. `None` when
/// `--srv-target-temp` is not in play.
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<Session>> {
    SESSION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Engage the srv thermal session if `--srv-target-temp` was requested. No-op
/// otherwise, so the channel stays strictly opt-in. Returns a message on
/// failure so the caller can decide whether to abort.
pub fn engage(target_c: Option<f32>, port: Option<u16>) -> Result<(), String> {
    let Some(target_c) = target_c else {
        return Ok(());
    };
    if !(30.0..=110.0).contains(&target_c) {
        return Err(format!(
            "--srv-target-temp must be 30-110 C, got {target_c}"
        ));
    }
    let port = port.unwrap_or(DEFAULT_PORT);

    let mut guard = lock();
    if guard.is_some() {
        return Ok(()); // already engaged
    }
    ensure_resident(port)?;
    let session = Session::open(port, "nvoc-stressor-cuda-rs", PRIORITY_STRESSOR)
        .map_err(|e| e.to_string())?;
    let owner = session
        .claim("pid", Some(target_c), None)
        .map_err(|e| e.to_string())?;
    if owner {
        eprintln!("nvoc-srv on port {port}: registered; holding {target_c} C for the stress run");
    } else {
        eprintln!(
            "nvoc-srv on port {port}: registered, but a higher-priority consumer currently \
             holds control"
        );
    }
    *guard = Some(session);
    Ok(())
}

/// Release the session, handing control back to the srv's arbitration. Safe to
/// call when no session is active.
pub fn release() {
    let mut guard = lock();
    if let Some(mut session) = guard.take() {
        session.close();
        eprintln!("nvoc-srv: thermal session released");
    }
}
