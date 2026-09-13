//! Loopback-only HTTP control plane (tiny_http, one supervised thread).
//!
//! CSRF posture unchanged from the legacy service: every mutating endpoint
//! requires POST **and** the non-simple `X-Requested-With: XMLHttpRequest`
//! header, which browsers refuse to attach to cross-origin requests without
//! a successful preflight.

use crate::config::{ControlMode, INTERVAL_MS_MAX, INTERVAL_MS_MIN, SensorKind};
use crate::controller::GpuControlStatus;
use crate::runtime::{ServiceCmd, SharedConfig, SharedStatus, lock};
use flume::Sender;
use log::{error, info, warn};
use serde::Serialize;
use std::collections::HashMap;
use std::thread::JoinHandle;
use std::time::Duration;
use tiny_http::{Header, Response, Server};

// Accepted OC frequency-delta range (kHz) for /oc_global (legacy semantics).
const OC_DELTA_MIN: i32 = -2_000_000;
const OC_DELTA_MAX: i32 = 2_000_000;

#[derive(Serialize)]
struct StatusView<'a> {
    mode: ControlMode,
    /// Control tick period (ms) — the PID dt.
    interval_ms: u64,
    /// Current PID setpoint (°C), for interpreting `pid.error_c`.
    target_c: f32,
    gpus: &'a [GpuControlStatus],
}

/// Decode a percent-encoded byte sequence (application/x-www-form-urlencoded style).
/// `+` → space, `%XX` → byte 0xXX; invalid escapes pass through as-is.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'+' {
            out.push(' ');
            i += 1;
        } else if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte as char);
                i += 3;
            } else {
                out.push('%');
                i += 1;
            }
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// Parse a `key=value&key=value` query string; duplicate keys keep the last value.
fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().filter(|k| !k.is_empty())?;
            let val = parts.next().unwrap_or("");
            Some((percent_decode(key), percent_decode(val)))
        })
        .collect()
}

/// Send a response, logging on failure (client may have disconnected).
fn respond(request: tiny_http::Request, response: Response<std::io::Cursor<Vec<u8>>>) {
    if let Err(e) = request.respond(response) {
        warn!("HTTP: failed to send response: {e}");
    }
}

/// CSRF guard for state-mutating endpoints (POST + non-simple header).
fn is_mutation_request(request: &tiny_http::Request) -> bool {
    if request.method() != &tiny_http::Method::Post {
        return false;
    }
    request.headers().iter().any(|h| {
        h.field.equiv("x-requested-with") && h.value.as_str().eq_ignore_ascii_case("xmlhttprequest")
    })
}

fn reject_mutation(request: tiny_http::Request, path: &str) {
    warn!("Rejected non-POST or missing X-Requested-With on {path}");
    respond(
        request,
        Response::from_string("Method Not Allowed: use POST with X-Requested-With: XMLHttpRequest")
            .with_status_code(405),
    );
}

fn json_content_type() -> Header {
    Header::from_bytes("Content-Type", "application/json").expect("static header is valid ASCII")
}

fn json_response<T: Serialize>(request: tiny_http::Request, value: &T) {
    match serde_json::to_string(value) {
        Ok(json) => respond(
            request,
            Response::from_string(json)
                .with_status_code(200)
                .with_header(json_content_type()),
        ),
        Err(e) => {
            error!("HTTP: serialize failed: {e}");
            respond(
                request,
                Response::from_string("Internal error").with_status_code(500),
            );
        }
    }
}

fn text_response(request: tiny_http::Request, code: u16, msg: impl Into<String>) {
    respond(
        request,
        Response::from_string(msg.into()).with_status_code(code),
    );
}

/// Supervised HTTP thread: restart the accept loop if it ever exits, so a
/// control-plane failure doesn't leave the service running but unresponsive
/// (#67). (Release builds abort on panic — SCM failure actions are the last
/// resort there; this loop covers clean exits and debug-build panics.)
pub fn spawn_supervised(
    config: SharedConfig,
    status: SharedStatus,
    cmd_tx: Sender<ServiceCmd>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("nvoc-http".into())
        .spawn(move || {
            loop {
                start_http_server(&config, &status, &cmd_tx);
                warn!("HTTP server exited; restarting in 1 s");
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .expect("spawn HTTP thread")
}

pub fn start_http_server(
    config: &SharedConfig,
    status: &SharedStatus,
    cmd_tx: &Sender<ServiceCmd>,
) {
    let port = lock(config).port;
    let bind = format!("127.0.0.1:{port}");
    let server = match Server::http(bind.as_str()) {
        Ok(s) => s,
        Err(e) => {
            error!("HTTP server failed to bind on {bind}: {e}");
            return;
        }
    };
    info!("HTTP control plane listening on {bind}");

    for request in server.incoming_requests() {
        let full_url = request.url().to_owned();
        let (path, query_str) = match full_url.split_once('?') {
            Some((p, q)) => (p, q),
            None => (full_url.as_str(), ""),
        };
        let params = parse_query(query_str);
        handle_request(request, path, &params, config, status, cmd_tx);
    }
}

fn handle_request(
    request: tiny_http::Request,
    path: &str,
    params: &HashMap<String, String>,
    config: &SharedConfig,
    status: &SharedStatus,
    cmd_tx: &Sender<ServiceCmd>,
) {
    match path {
        "/" => {
            let help = "nvoc-srv control plane\n\
                        GET  /status   — per-GPU temps, fan duty, PID terms, failsafe state\n\
                        GET  /config   — effective runtime configuration\n\
                        POST /pid?target_c=&kp=&ki=&kd=&base_percent=&min_percent=&max_percent=&emergency_delta_c=&interval_ms=&sensor=\n\
                        POST /mode?value=auto|pid|manual\n\
                        POST /fan?percent=0-100        (switches to manual)\n\
                        POST /restore                  (alias of /mode?value=auto)\n\
                        POST /oc_global?oc=<kHz>&gpu=<index>\n\
                        POST /shutdown\n\
                        Mutations require POST + X-Requested-With: XMLHttpRequest.\n";
            text_response(request, 200, help);
        }
        "/status" => {
            let (mode, interval_ms, target_c) = {
                let cfg = lock(config);
                (cfg.mode, cfg.interval_ms, cfg.pid.target_c)
            };
            let guard = lock(status);
            json_response(
                request,
                &StatusView {
                    mode,
                    interval_ms,
                    target_c,
                    gpus: &guard,
                },
            );
        }
        "/config" => {
            let cfg = lock(config);
            json_response(request, &*cfg);
        }
        "/pid" => {
            if !is_mutation_request(&request) {
                reject_mutation(request, path);
                return;
            }
            handle_pid_update(request, params, config);
        }
        "/mode" => {
            if !is_mutation_request(&request) {
                reject_mutation(request, path);
                return;
            }
            match params.get("value").map(|s| s.to_ascii_lowercase()) {
                Some(v) if v == "auto" || v == "pid" || v == "manual" => {
                    let mode = match v.as_str() {
                        "auto" => ControlMode::Auto,
                        "pid" => ControlMode::Pid,
                        _ => ControlMode::Manual,
                    };
                    lock(config).mode = mode;
                    info!("control mode set to {mode:?} via HTTP");
                    text_response(request, 200, format!("OK: mode={mode:?}"));
                }
                _ => text_response(request, 400, "Bad request: 'value' must be auto|pid|manual"),
            }
        }
        "/fan" => {
            if !is_mutation_request(&request) {
                reject_mutation(request, path);
                return;
            }
            match params.get("percent").and_then(|s| s.parse::<u32>().ok()) {
                Some(p) if p <= 100 => {
                    let mut cfg = lock(config);
                    cfg.mode = ControlMode::Manual;
                    cfg.manual_percent = p;
                    info!("manual fan duty {p}% via HTTP");
                    text_response(request, 200, format!("OK: manual {p}%"));
                }
                _ => text_response(request, 400, "Bad request: 'percent' must be 0–100"),
            }
        }
        "/restore" => {
            if !is_mutation_request(&request) {
                reject_mutation(request, path);
                return;
            }
            lock(config).mode = ControlMode::Auto;
            info!("fan control restored to driver via HTTP /restore");
            text_response(request, 200, "OK: mode=Auto, driver control resumes");
        }
        "/shutdown" => {
            if !is_mutation_request(&request) {
                reject_mutation(request, path);
                return;
            }
            match cmd_tx.send(ServiceCmd::Shutdown) {
                Ok(()) => {
                    info!("shutdown requested via HTTP");
                    text_response(request, 200, "OK: shutting down");
                }
                Err(e) => {
                    error!("HTTP: cannot deliver shutdown: {e}");
                    text_response(request, 500, "Internal error");
                }
            }
        }
        "/oc_global" => {
            if !is_mutation_request(&request) {
                reject_mutation(request, path);
                return;
            }
            handle_oc_global(request, params, cmd_tx);
        }
        _ => text_response(request, 404, "Not found"),
    }
}

/// Assign `params[key]` into `dst` when present; a present-but-invalid value
/// records an error so the whole update is rejected atomically.
fn assign_f32(
    params: &HashMap<String, String>,
    key: &str,
    dst: &mut f32,
    errors: &mut Vec<String>,
) {
    if let Some(raw) = params.get(key) {
        match raw.parse::<f32>() {
            Ok(v) if v.is_finite() => *dst = v,
            _ => errors.push(format!("invalid '{key}'")),
        }
    }
}

fn assign_u32(
    params: &HashMap<String, String>,
    key: &str,
    dst: &mut u32,
    errors: &mut Vec<String>,
) {
    if let Some(raw) = params.get(key) {
        match raw.parse::<u32>() {
            Ok(v) => *dst = v,
            _ => errors.push(format!("invalid '{key}'")),
        }
    }
}

/// Partial PID update; absent parameters keep their current value. Full
/// param-set validation runs after the merge so a bad combination (min>max)
/// is rejected atomically.
fn handle_pid_update(
    request: tiny_http::Request,
    params: &HashMap<String, String>,
    config: &SharedConfig,
) {
    let mut cfg = lock(config);
    let mut p = cfg.pid.clone();
    let mut errors: Vec<String> = Vec::new();

    assign_f32(params, "target_c", &mut p.target_c, &mut errors);
    assign_f32(params, "kp", &mut p.kp, &mut errors);
    assign_f32(params, "ki", &mut p.ki, &mut errors);
    assign_f32(params, "kd", &mut p.kd, &mut errors);
    assign_f32(params, "base_percent", &mut p.base_percent, &mut errors);
    assign_f32(params, "min_percent", &mut p.min_percent, &mut errors);
    assign_f32(params, "max_percent", &mut p.max_percent, &mut errors);
    assign_f32(
        params,
        "emergency_delta_c",
        &mut p.emergency_delta_c,
        &mut errors,
    );
    assign_f32(
        params,
        "release_below_c",
        &mut p.release_below_c,
        &mut errors,
    );
    assign_f32(params, "engage_below_c", &mut p.engage_below_c, &mut errors);
    assign_u32(params, "release_ticks", &mut p.release_ticks, &mut errors);
    assign_f32(
        params,
        "write_deadband_percent",
        &mut p.write_deadband_percent,
        &mut errors,
    );
    if let Some(v) = params
        .get("interval_ms")
        .and_then(|s| s.parse::<u64>().ok())
    {
        if (INTERVAL_MS_MIN..=INTERVAL_MS_MAX).contains(&v) {
            cfg.interval_ms = v;
        } else {
            errors.push(format!(
                "'interval_ms' must be {INTERVAL_MS_MIN}–{INTERVAL_MS_MAX}"
            ));
        }
    }
    if let Some(s) = params.get("sensor") {
        match SensorKind::parse(s) {
            Some(kind) => cfg.sensor = kind,
            None => errors.push("'sensor' must be core|hotspot|memory|board|max".to_string()),
        }
    }

    if !errors.is_empty() {
        text_response(request, 400, format!("Bad request: {}", errors.join("; ")));
        return;
    }
    if let Err(e) = p.validate() {
        text_response(request, 400, format!("Bad request: {e}"));
        return;
    }
    // validate() already bounds the setpoint; this log line is the audit trail.
    let target = p.target_c;
    let kp = p.kp;
    let ki = p.ki;
    let kd = p.kd;
    let base = p.base_percent;
    cfg.pid = p;
    info!("PID updated via HTTP: target={target} kp={kp} ki={ki} kd={kd} base={base}");
    text_response(request, 200, "OK: pid updated");
}

fn handle_oc_global(
    request: tiny_http::Request,
    params: &HashMap<String, String>,
    cmd_tx: &Sender<ServiceCmd>,
) {
    let gpu_index: usize = match params.get("gpu") {
        None => 0,
        Some(s) => match s.parse::<usize>() {
            Ok(n) => n,
            Err(_) => {
                text_response(
                    request,
                    400,
                    format!("Bad request: 'gpu' must be a non-negative integer, got {s:?}"),
                );
                return;
            }
        },
    };
    match params.get("oc").and_then(|s| s.parse::<i32>().ok()) {
        Some(delta) if (OC_DELTA_MIN..=OC_DELTA_MAX).contains(&delta) => {
            match cmd_tx.send(ServiceCmd::SetOcGlobal {
                gpu_index,
                delta_khz: delta,
            }) {
                Ok(()) => {
                    info!("GPU {gpu_index}: queued OC delta {delta} kHz");
                    text_response(request, 200, "OK");
                }
                Err(e) => {
                    error!("HTTP: cannot enqueue OC command: {e}");
                    text_response(request, 500, "Internal error");
                }
            }
        }
        Some(delta) => text_response(
            request,
            400,
            format!("Bad request: oc must be {OC_DELTA_MIN}–{OC_DELTA_MAX} kHz, got {delta}"),
        ),
        None => text_response(request, 400, "Bad request: missing or invalid 'oc'"),
    }
}
