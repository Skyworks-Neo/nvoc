//! Manual override of the current pressure-test result.
//!
//! When `--manual-override` is active (Windows only), a background thread reads
//! the console input queue and watches for `Alt+P` (force current test PASS) and
//! `Alt+F` (force current test FAIL). The request lands in a global slot that the
//! pressure-test monitor loop drains once per iteration via [`take_override`].
//!
//! Design notes:
//! - Reads come from the **console input queue** (`CONIN$`), not the process
//!   stdin handle, so this never competes with the stressor subprocess or
//!   indicatif, and it never changes the console mode (no raw mode).
//! - The global slot mirrors the `ACTIVE_SCAN_PROGRESS` pattern in
//!   `progressbar.rs`: a `Lazy<Mutex<Option<…>>>` plus an RAII guard that clears
//!   it when the scan ends, so an override can never leak across scans.
//! - Linux/other platforms compile against no-op stubs; `--manual-override`
//!   simply does nothing there.
// On non-Windows the whole mechanism is a no-op, so the guard/enum look unused.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::Mutex;

#[cfg(windows)]
use std::sync::Arc;
#[cfg(windows)]
use std::sync::atomic::{AtomicBool, Ordering};

use once_cell::sync::Lazy;

/// A pending manual override of the current pressure-test result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideRequest {
    /// Treat the current test as passed (exit code 0).
    Pass,
    /// Treat the current test as failed (exit code != 0).
    Fail,
}

static ACTIVE_OVERRIDE: Lazy<Mutex<Option<OverrideRequest>>> = Lazy::new(|| Mutex::new(None));

/// RAII guard: while alive, a scan is willing to accept manual overrides and the
/// keyboard listener (if any) is running. Clears the global slot on drop so no
/// stale request survives into the next scan.
pub struct ManualOverrideGuard {
    #[cfg(windows)]
    stop: Arc<AtomicBool>,
}

impl ManualOverrideGuard {
    /// Register the override slot and (on Windows) spawn the console-input
    /// listener thread. Call only when the caller has already validated that the
    /// `--manual-override` flag was passed.
    pub fn enter() -> Self {
        if let Ok(mut slot) = ACTIVE_OVERRIDE.lock() {
            *slot = None;
        }

        #[cfg(windows)]
        {
            let stop = Arc::new(AtomicBool::new(false));
            spawn_windows_listener(Arc::clone(&stop));
            Self { stop }
        }

        #[cfg(not(windows))]
        {
            // On non-Windows the struct has no fields; use the empty braced form.
            Self {}
        }
    }
}

#[cfg(windows)]
impl Drop for ManualOverrideGuard {
    fn drop(&mut self) {
        // Signal the listener to stop, then drop the slot it was writing to.
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut slot) = ACTIVE_OVERRIDE.lock() {
            *slot = None;
        }
    }
}

#[cfg(not(windows))]
impl Drop for ManualOverrideGuard {
    fn drop(&mut self) {
        if let Ok(mut slot) = ACTIVE_OVERRIDE.lock() {
            *slot = None;
        }
    }
}

/// Atomically take and clear any pending override request.
///
/// Called once per monitor-loop iteration from `run_pressure_test`. The swap
/// semantics mean rapid repeat presses collapse to a single request, and a
/// request never survives past the one test it was meant to end.
pub fn take_override() -> Option<OverrideRequest> {
    match ACTIVE_OVERRIDE.lock() {
        Ok(mut slot) => slot.take(),
        Err(_) => None,
    }
}

// ───────────────────────── Windows listener ─────────────────────────────────

#[cfg(windows)]
fn spawn_windows_listener(stop: Arc<AtomicBool>) {
    use std::thread;

    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Console::{
        GetNumberOfConsoleInputEvents, GetStdHandle, INPUT_RECORD, KEY_EVENT, KEY_EVENT_RECORD,
        LEFT_ALT_PRESSED, RIGHT_ALT_PRESSED, ReadConsoleInputW, STD_INPUT_HANDLE,
    };
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    // Use the process console input handle. This reads the console input queue
    // (not the stressor's stdin) and supports WaitForSingleObject so we can poll
    // for input with a bounded shutdown latency. We do NOT call CloseHandle on a
    // STD handle; the OS owns it for the process lifetime.
    let raw = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if raw.is_null() {
        eprintln!("manual-override: no console input handle; listener disabled.");
        return;
    }

    // HANDLEs are `*mut c_void` and not `Send`; carry the value across threads
    // as an isize (lossless round-trip) and cast back at each FFI call site.
    let handle_bits = raw as isize;

    thread::spawn(move || {
        let handle = handle_bits as windows_sys::Win32::Foundation::HANDLE;
        // Poll loop: wait up to 200ms for input, drain events if signaled, then
        // re-check the stop flag. This bounds shutdown latency without needing
        // CancelIoEx on a blocking ReadConsoleInputW.
        while !stop.load(Ordering::Relaxed) {
            let waited = unsafe { WaitForSingleObject(handle, 200) };
            if waited == WAIT_TIMEOUT {
                continue;
            }
            if waited != WAIT_OBJECT_0 {
                // Handle became invalid / error: stop listening.
                break;
            }

            // Peek the count first so we never block on an empty queue.
            let mut available: u32 = 0;
            if unsafe { GetNumberOfConsoleInputEvents(handle, &mut available) } == 0
                || available == 0
            {
                continue;
            }

            let mut buf: [INPUT_RECORD; 64] = unsafe { std::mem::zeroed() };
            let mut read: u32 = 0;
            if unsafe { ReadConsoleInputW(handle, buf.as_mut_ptr(), buf.len() as u32, &mut read) }
                == 0
                || read == 0
            {
                continue;
            }

            for record in buf.iter().take(read as usize) {
                // INPUT_RECORD::EventType == 1 is KEY_EVENT.
                if record.EventType != KEY_EVENT as u16 {
                    continue;
                }
                // SAFETY: the EventType discriminant just checked guarantees the
                // Event union currently holds the KeyEvent variant.
                let ke: KEY_EVENT_RECORD = unsafe { record.Event.KeyEvent };

                // Only act on key-down transitions.
                if ke.bKeyDown == 0 {
                    continue;
                }

                let alt = ke.dwControlKeyState & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0;
                if !alt {
                    continue;
                }

                let ch = char::from_u32(unsafe { ke.uChar.UnicodeChar } as u32);
                let req = match ch {
                    Some('p') | Some('P') => Some(OverrideRequest::Pass),
                    Some('f') | Some('F') => Some(OverrideRequest::Fail),
                    _ => None,
                };

                if let Some(req) = req {
                    if let Ok(mut slot) = ACTIVE_OVERRIDE.lock() {
                        *slot = Some(req);
                    }
                }
            }
        }
    });
}
