//! In-memory audit trail: every mutation through the API and every failed
//! login lands here (bounded ring, newest first via `/api/log`). Restart
//! clears it — persistent audit is a later concern.

use serde::Serialize;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const CAPACITY: usize = 500;

#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    /// RFC-ish local timestamp string (`2026-09-17 12:34:56`).
    pub time: String,
    /// Authenticated user, or "anonymous" when auth is off, "-" for failed logins.
    pub user: String,
    /// Short action id (`oc.offset`, `mode`, `login.failed`, …).
    pub action: String,
    pub detail: String,
}

#[derive(Debug, Default)]
pub struct AuditLog {
    entries: Mutex<Vec<AuditEntry>>,
}

pub static AUDIT: AuditLog = AuditLog {
    entries: Mutex::new(Vec::new()),
};

pub fn record(user: &str, action: &str, detail: &str) {
    let mut guard = AUDIT.entries.lock().unwrap_or_else(|p| p.into_inner());
    if guard.len() >= CAPACITY {
        guard.remove(0);
    }
    guard.push(AuditEntry {
        time: timestamp(),
        user: user.to_string(),
        action: action.to_string(),
        detail: detail.to_string(),
    });
}

pub fn snapshot() -> Vec<AuditEntry> {
    let guard = AUDIT.entries.lock().unwrap_or_else(|p| p.into_inner());
    guard.iter().rev().cloned().collect()
}

fn timestamp() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    // Days since epoch -> civil date (Howard Hinnant's algorithm), UTC.
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{day:02} {h:02}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_is_bounded_and_newest_first() {
        for i in 0..(CAPACITY + 20) {
            record("u", "test.action", &format!("entry {i}"));
        }
        let snap = snapshot();
        assert_eq!(snap.len(), CAPACITY);
        assert!(snap[0].detail.contains("entry"), "newest first");
        assert!(snap.iter().all(|e| e.action == "test.action"));
    }
}
