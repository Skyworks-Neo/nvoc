// Standalone smoke test for the NVOC-SRV HTTP control layer.
//
// Runs ONLY `start_http_server` (binds 127.0.0.1:14514) with a throwaway config
// and a draining command channel. It deliberately does NOT start the service
// worker loop, so it never enumerates or writes to any GPU — safe to run on
// production cards just to verify the control plane is reachable.
//
// Not installed as a Windows service: this is a plain console program.

#[cfg(windows)]
#[path = "../websrv.rs"]
mod websrv;

#[cfg(windows)]
fn main() {
    use std::sync::{Arc, Mutex};
    use websrv::{NVOCServiceCmd, NVOCServiceConfig, start_http_server};

    let config = Arc::new(Mutex::new(NVOCServiceConfig {
        vfp_lock_point: 70,
        temp_limit: 60,
    }));

    // Keep the receiver alive so /oc_global enqueues succeed, but drain commands
    // to a log instead of executing them — no GPU side effects.
    let (cmd_tx, cmd_rx) = flume::unbounded::<NVOCServiceCmd>();
    std::thread::spawn(move || {
        for c in cmd_rx.iter() {
            eprintln!(
                "[drain] command received (NOT applied to GPU): cmd={} gpu_index={} over_freq={} kHz",
                c.cmd, c.gpu_index, c.over_freq
            );
        }
    });

    eprintln!(
        "[smoketest] HTTP control layer only — no GPU access. Listening on http://127.0.0.1:14514"
    );
    start_http_server(config, cmd_tx);
}

#[cfg(not(windows))]
fn main() {
    panic!("This program is only intended to run on Windows.");
}
