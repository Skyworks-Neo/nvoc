//! Developer-only manual override of the active pressure-test result.
//!
//! The listener deliberately uses normal, line-buffered stdin instead of raw
//! terminal input. Typing `p`/`pass` or `f`/`fail` followed by Enter requests a
//! result without changing terminal modes, intercepting Ctrl+C, or relying on
//! platform-specific console APIs.

use std::io::{self, BufRead, IsTerminal};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;

const NO_REQUEST: u8 = 0;
const PASS_REQUEST: u8 = 1;
const FAIL_REQUEST: u8 = 2;

/// A pending manual override of the current pressure-test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideRequest {
    Pass,
    Fail,
}

impl OverrideRequest {
    fn encode(self) -> u8 {
        match self {
            Self::Pass => PASS_REQUEST,
            Self::Fail => FAIL_REQUEST,
        }
    }

    fn decode(value: u8) -> Option<Self> {
        match value {
            PASS_REQUEST => Some(Self::Pass),
            FAIL_REQUEST => Some(Self::Fail),
            _ => None,
        }
    }

    pub fn result(self) -> (i32, &'static str) {
        match self {
            Self::Pass => (0, "MANUAL PASS"),
            Self::Fail => (1, "MANUAL FAIL"),
        }
    }
}

/// Process-local controller shared by every pressure test in one autoscan.
///
/// The reader thread may block in `read_line` until the CLI exits. Rust does not
/// wait for detached threads when `main` returns, so this preserves normal
/// process shutdown without changing terminal modes merely to wake the reader.
pub struct ManualOverride {
    pending: Arc<AtomicU8>,
}

impl ManualOverride {
    /// Start one stdin listener for the current autoscan command.
    pub fn start() -> io::Result<Self> {
        if !io::stdin().is_terminal() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "--manual-override requires an interactive stdin terminal",
            ));
        }

        let pending = Arc::new(AtomicU8::new(NO_REQUEST));
        let listener_pending = Arc::clone(&pending);
        thread::Builder::new()
            .name("nvoc-manual-override".to_string())
            .spawn(move || {
                let stdin = io::stdin();
                read_commands(stdin.lock(), listener_pending.as_ref());
            })?;

        eprintln!("Manual override enabled: enter 'p'/'pass' or 'f'/'fail', then press Enter.");
        Ok(Self { pending })
    }

    /// Remove input typed before the next stressor attempt became active.
    pub fn clear(&self) {
        self.pending.store(NO_REQUEST, Ordering::Release);
    }

    /// Atomically consume the latest pending request.
    pub fn take(&self) -> Option<OverrideRequest> {
        OverrideRequest::decode(self.pending.swap(NO_REQUEST, Ordering::AcqRel))
    }

    #[cfg(test)]
    fn record(&self, request: OverrideRequest) {
        self.pending.store(request.encode(), Ordering::Release);
    }
}

fn parse_command(line: &str) -> Option<OverrideRequest> {
    match line.trim().to_ascii_lowercase().as_str() {
        "p" | "pass" => Some(OverrideRequest::Pass),
        "f" | "fail" => Some(OverrideRequest::Fail),
        _ => None,
    }
}

fn read_commands(reader: impl BufRead, pending: &AtomicU8) {
    for line in reader.lines() {
        match line {
            Ok(line) if line.trim().is_empty() => {}
            Ok(line) => match parse_command(&line) {
                Some(request) => pending.store(request.encode(), Ordering::Release),
                None => {
                    eprintln!("Unknown manual override command; enter 'p'/'pass' or 'f'/'fail'.")
                }
            },
            Err(error) => {
                eprintln!("Manual override input stopped: {error}");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn controller() -> ManualOverride {
        ManualOverride {
            pending: Arc::new(AtomicU8::new(NO_REQUEST)),
        }
    }

    #[test]
    fn parses_short_and_long_commands_case_insensitively() {
        for command in ["p", " pass ", "P", "PASS"] {
            assert_eq!(parse_command(command), Some(OverrideRequest::Pass));
        }
        for command in ["f", " fail ", "F", "FAIL"] {
            assert_eq!(parse_command(command), Some(OverrideRequest::Fail));
        }
        assert_eq!(parse_command(""), None);
        assert_eq!(parse_command("skip"), None);
    }

    #[test]
    fn latest_request_wins_and_take_consumes_it() {
        let controller = controller();
        controller.record(OverrideRequest::Pass);
        controller.record(OverrideRequest::Fail);

        assert_eq!(controller.take(), Some(OverrideRequest::Fail));
        assert_eq!(controller.take(), None);
    }

    #[test]
    fn clear_discards_a_stale_request() {
        let controller = controller();
        controller.record(OverrideRequest::Pass);
        controller.clear();

        assert_eq!(controller.take(), None);
    }

    #[test]
    fn reader_records_the_latest_valid_command() {
        let pending = AtomicU8::new(NO_REQUEST);
        read_commands(Cursor::new("pass\n\nfail\n"), &pending);

        assert_eq!(
            OverrideRequest::decode(pending.load(Ordering::Acquire)),
            Some(OverrideRequest::Fail)
        );
    }

    #[test]
    fn requests_map_to_pressure_test_results() {
        assert_eq!(OverrideRequest::Pass.result(), (0, "MANUAL PASS"));
        assert_eq!(OverrideRequest::Fail.result(), (1, "MANUAL FAIL"));
    }
}
