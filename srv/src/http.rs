//! Loopback-only HTTP control plane (tiny_http, one supervised thread).
//!
//! CSRF posture unchanged from the legacy service: every mutating endpoint
//! requires POST **and** the non-simple `X-Requested-With: XMLHttpRequest`
//! header, which browsers refuse to attach to cross-origin requests without
//! a successful preflight.

use crate::config::{
    ControlMode, INTERVAL_MS_MAX, INTERVAL_MS_MIN, LoopKind, RuntimeConfig, SensorKind,
};
use crate::controller::{ControlBackend, GpuControlStatus};
use crate::monitor::{OffsetBackend, OffsetDomain};
use crate::runtime::{ServiceCmd, SharedBackend, SharedConfig, SharedHistory, SharedStatus, lock};
use crate::{audit, auth, web};
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
    loop_kind: LoopKind,
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

/// 401 + Basic challenge (browser pops the native login dialog).
fn challenge(request: tiny_http::Request) {
    let header =
        Header::from_bytes("WWW-Authenticate", "Basic realm=\"nvoc-srv\"").expect("valid header");
    respond(
        request,
        Response::from_string("authentication required")
            .with_status_code(401)
            .with_header(header),
    );
}

/// Audit + execute a backend write op from the HTTP plane.
fn audited_write(
    auth_user: &str,
    action: &str,
    detail: String,
    op: impl FnOnce() -> Result<(), String>,
) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    match op() {
        Ok(()) => {
            audit::record(auth_user, action, &detail);
            text_response_body(format!("OK: {action} {detail}"))
        }
        Err(e) => text_response_body(format!("ERR: {e}")),
    }
}

fn text_response_body(body: String) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body)
}

/// `/api/*` — JSON plane: monitoring, history, info, OC writes, audit log.
#[allow(clippy::too_many_arguments)]
fn handle_api(
    request: tiny_http::Request,
    api_path: &str,
    params: &HashMap<String, String>,
    config: &SharedConfig,
    status: &SharedStatus,
    backend: &SharedBackend,
    history: &SharedHistory,
    cmd_tx: &Sender<ServiceCmd>,
    auth_user: &str,
) {
    // cmd_tx is plumbed for future API commands (shutdown/restore live on the
    // legacy routes; new API mutations talk to the backend directly).
    let _ = cmd_tx;
    let gpu_index = || -> Result<usize, String> {
        params
            .get("gpu")
            .ok_or_else(|| "missing 'gpu'".to_string())?
            .parse::<usize>()
            .map_err(|_| "invalid 'gpu'".to_string())
    };

    match (request.method(), api_path) {
        (_, "log") => json_response(request, &audit::snapshot()),

        // ---- reads ----
        (&tiny_http::Method::Get, "status") => {
            let cfg = lock(config);
            let guard = lock(status);
            json_response(
                request,
                &serde_json::json!({
                    "mode": cfg.mode,
                    "loop_kind": cfg.loop_kind,
                    "interval_ms": cfg.interval_ms,
                    "target_c": cfg.pid.target_c,
                    "gpus": &*guard,
                }),
            );
        }
        (&tiny_http::Method::Get, "gpus") => {
            let guard = lock(status);
            let list: Vec<serde_json::Value> = guard
                .iter()
                .map(|g| {
                    serde_json::json!({
                        "index": g.index,
                        "name": g.name,
                        "mode": g.mode,
                        "failsafe": g.failsafe,
                    })
                })
                .collect();
            json_response(request, &serde_json::json!({ "gpus": list }));
        }
        (&tiny_http::Method::Get, "history") => {
            let index = match gpu_index() {
                Ok(v) => v,
                Err(e) => return text_response(request, 400, format!("Bad request: {e}")),
            };
            let seconds = params
                .get("seconds")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(300)
                .min(3600);
            let guard = lock(history);
            json_response(request, &guard.last(index, seconds));
        }
        (&tiny_http::Method::Get, "info") | (&tiny_http::Method::Get, "vfcurve") => {
            let index = match gpu_index() {
                Ok(v) => v,
                Err(e) => return text_response(request, 400, format!("Bad request: {e}")),
            };
            let mut guard = lock(backend);
            let Some(backend) = guard.as_mut() else {
                return text_response(request, 503, "backend not ready");
            };
            if api_path == "info" {
                match backend.read_gpu_info_json(index) {
                    Ok(v) => json_response(request, &v),
                    Err(e) => text_response(request, 502, e),
                }
            } else {
                match backend.read_vf_curve(index) {
                    Ok(pts) => json_response(
                        request,
                        &serde_json::json!({
                            "points": pts.iter()
                                .map(|(mv, mhz)| serde_json::json!({"mv": mv, "mhz": mhz}))
                                .collect::<Vec<_>>()
                        }),
                    ),
                    Err(e) => text_response(request, 502, e),
                }
            }
        }

        // ---- OC reads for the form ----
        (&tiny_http::Method::Get, "oc") => {
            let index = match gpu_index() {
                Ok(v) => v,
                Err(e) => return text_response(request, 400, format!("Bad request: {e}")),
            };
            let mut guard = lock(backend);
            let Some(backend) = guard.as_mut() else {
                return text_response(request, 503, "backend not ready");
            };
            let core_off = backend
                .read_offset_mhz(index, OffsetDomain::Core, OffsetBackend::Nvml)
                .ok();
            let mem_off = backend
                .read_offset_mhz(index, OffsetDomain::Mem, OffsetBackend::Nvml)
                .ok();
            let power = backend.read_power_limit_w(index).ok().flatten();
            let temp = backend.read_temp_limit_c(index).ok().flatten();
            json_response(
                request,
                &serde_json::json!({
                    "core_offset_mhz": core_off,
                    "mem_offset_mhz": mem_off,
                    "power": power.map(|(min, cur, max)| {
                        serde_json::json!({"min_w": min, "current_w": cur, "max_w": max})
                    }),
                    "temp": temp.map(|(min, cur, max)| {
                        serde_json::json!({"min_c": min, "current_c": cur, "max_c": max})
                    }),
                }),
            );
        }

        // ---- writes (audited) ----
        (&tiny_http::Method::Post, "oc/offset") => {
            let (index, domain, backend_kind, value) = match oc_offset_params(params) {
                Ok(v) => v,
                Err(e) => return text_response(request, 400, format!("Bad request: {e}")),
            };
            let detail = format!(
                "gpu {index} {domain} {value} MHz via {}",
                backend_kind.as_str()
            );
            let mut guard = lock(backend);
            let Some(backend) = guard.as_mut() else {
                return text_response(request, 503, "backend not ready");
            };
            let response = audited_write(auth_user, "oc.offset", detail, || {
                backend.write_offset_mhz(index, domain, backend_kind, value)
            });
            respond(request, response);
        }
        (&tiny_http::Method::Post, "oc/power") => {
            let index = match gpu_index() {
                Ok(v) => v,
                Err(e) => return text_response(request, 400, format!("Bad request: {e}")),
            };
            let Some(watts) = params.get("watt").and_then(|s| s.parse::<u32>().ok()) else {
                return text_response(request, 400, "Bad request: 'watt' required");
            };
            let mut guard = lock(backend);
            let Some(backend) = guard.as_mut() else {
                return text_response(request, 503, "backend not ready");
            };
            let detail = format!("gpu {index} {watts} W");
            let response = audited_write(auth_user, "oc.power", detail, || {
                backend.write_power_limit_w(index, watts)
            });
            respond(request, response);
        }
        (&tiny_http::Method::Post, "oc/temp") => {
            let index = match gpu_index() {
                Ok(v) => v,
                Err(e) => return text_response(request, 400, format!("Bad request: {e}")),
            };
            let Some(c) = params.get("c").and_then(|s| s.parse::<i32>().ok()) else {
                return text_response(request, 400, "Bad request: 'c' required");
            };
            let mut guard = lock(backend);
            let Some(backend) = guard.as_mut() else {
                return text_response(request, 503, "backend not ready");
            };
            let detail = format!("gpu {index} {c} °C");
            let response = audited_write(auth_user, "oc.temp", detail, || {
                backend.write_temp_limit_c(index, c)
            });
            respond(request, response);
        }
        (&tiny_http::Method::Post, "reset") => {
            let index = match gpu_index() {
                Ok(v) => v,
                Err(e) => return text_response(request, 400, format!("Bad request: {e}")),
            };
            let Some(kind) = params.get("kind").map(|s| s.as_str()) else {
                return text_response(request, 400, "Bad request: 'kind' required");
            };
            let mut guard = lock(backend);
            let Some(backend) = guard.as_mut() else {
                return text_response(request, 503, "backend not ready");
            };
            let result = match kind {
                "offset_core" | "offset_core_nvml" => {
                    let backend_kind = if kind == "offset_core_nvml" {
                        OffsetBackend::Nvml
                    } else {
                        OffsetBackend::Nvapi
                    };
                    // NVML reset semantics = write 0 on P0.
                    if backend_kind == OffsetBackend::Nvml {
                        backend.write_offset_mhz(index, OffsetDomain::Core, backend_kind, 0)
                    } else {
                        backend.reset_offset(index, OffsetDomain::Core)
                    }
                }
                "offset_mem" => backend.reset_offset(index, OffsetDomain::Mem),
                "power" => backend.reset_power_limit(index),
                "temp" => backend.reset_temp_limit(index),
                other => {
                    return text_response(
                        request,
                        400,
                        format!("Bad request: unknown kind {other:?}"),
                    );
                }
            };
            let detail = format!("gpu {index} {kind}");
            let response = audited_write(auth_user, "reset", detail, || result);
            respond(request, response);
        }

        _ => text_response(request, 404, "Not found"),
    }
}

/// Parse + validate `oc/offset` params: `gpu`, `domain=core|mem`,
/// `backend=nvml|nvapi`, `value=±MHz`.
fn oc_offset_params(
    params: &HashMap<String, String>,
) -> Result<(usize, OffsetDomain, OffsetBackend, i32), String> {
    let index = params
        .get("gpu")
        .ok_or("missing 'gpu'")?
        .parse::<usize>()
        .map_err(|_| "invalid 'gpu'")?;
    let domain = params
        .get("domain")
        .and_then(|s| OffsetDomain::parse(s))
        .ok_or("'domain' must be core|mem")?;
    let backend_kind = params
        .get("backend")
        .and_then(|s| OffsetBackend::parse(s))
        .unwrap_or(OffsetBackend::Nvml);
    let raw = params.get("value").ok_or("missing 'value'")?;
    let value = raw
        .trim_end_matches(['m', 'H', 'z'])
        .parse::<i32>()
        .map_err(|_| "invalid 'value'")?;
    if !(-1000..=1000).contains(&value) {
        return Err(format!("'value' must be -1000..1000 MHz, got {value}"));
    }
    Ok((index, domain, backend_kind, value))
}

fn json_content_type() -> Header {
    Header::from_bytes("Content-Type", "application/json").expect("static header is valid ASCII")
}

/// Serve an embedded asset with its content type.
fn serve_static(request: tiny_http::Request, body: &'static str, content_type: &str) {
    let header =
        Header::from_bytes("Content-Type", content_type).expect("static header is valid ASCII");
    respond(request, Response::from_string(body).with_header(header));
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
/// Shared plane state threaded through every request.
pub struct ServerState {
    pub config: SharedConfig,
    pub status: SharedStatus,
    pub backend: SharedBackend,
    pub history: SharedHistory,
    pub cmd_tx: Sender<ServiceCmd>,
}

pub fn spawn_supervised(state: std::sync::Arc<ServerState>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("nvoc-http".into())
        .spawn(move || {
            loop {
                start_http_server(&state);
                warn!("HTTP server exited; restarting in 1 s");
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .expect("spawn HTTP thread")
}

pub fn start_http_server(state: &std::sync::Arc<ServerState>) {
    let port = lock(&state.config).port;
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
        handle_request(request, path, &params, state);
    }
}

fn handle_request(
    request: tiny_http::Request,
    path: &str,
    params: &HashMap<String, String>,
    state: &std::sync::Arc<ServerState>,
) {
    let ServerState {
        config,
        status,
        backend,
        history,
        cmd_tx,
    } = state.as_ref();

    // OS-account authentication gate (whole site — one gate, one audit story).
    let auth_enabled = lock(config).auth.resolves_enabled();
    let mut auth_user = "anonymous".to_string();
    if auth_enabled {
        let header = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .map(|h| h.value.as_str().to_string());
        match header.as_deref().and_then(auth::parse_basic) {
            Some(creds) => {
                let reason = {
                    let cfg = lock(config);
                    auth::verify(&creds.user, &creds.password, &cfg).err()
                };
                match reason {
                    None => auth_user = creds.user,
                    Some(reason) => {
                        audit::record("-", "login.failed", &reason);
                        warn!("auth: failed login for {:?}: {reason}", creds.user);
                        challenge(request);
                        return;
                    }
                }
            }
            None => {
                challenge(request);
                return;
            }
        }
    }

    // API plane (JSON, audited).
    if let Some(api) = path.strip_prefix("/api/") {
        handle_api(
            request, api, params, config, status, backend, history, cmd_tx, &auth_user,
        );
        return;
    }

    match path {
        "/" => {
            // The embedded console page (GET is same-origin safe).
            serve_static(request, web::INDEX_HTML, "text/html; charset=utf-8");
        }
        "/ui.css" => {
            serve_static(request, web::STYLE_CSS, "text/css; charset=utf-8");
        }

        "/help" => {
            let help = "nvoc-srv control plane\n\
                        GET  /status   — per-GPU temps, fan duty, PID terms, failsafe state\n\
                        GET  /config   — effective runtime configuration\n\
                        POST /pid?target_c=&kp=&ki=&kd=&base_percent=&min_percent=&max_percent=&emergency_delta_c=&idle_delta_c=&temp_guard_c=&min_mhz=&max_mhz=&interval_ms=&sensor=\n\
                        POST /mode?value=auto|pid|manual\n\
                        POST /loop?value=fan_temp|freq_temp|freq_power\n\
                        POST /fan?percent=0-100        (switches to manual)\n\
                        POST /restore                  (alias of /mode?value=auto)\n\
                        POST /oc_global?oc=<kHz>&gpu=<index>\n\
                        POST /shutdown\n\
                        Mutations require POST + X-Requested-With: XMLHttpRequest.\n";
            text_response(request, 200, help);
        }
        "/loop" => {
            if !is_mutation_request(&request) {
                reject_mutation(request, path);
                return;
            }
            match params.get("value").and_then(|v| LoopKind::parse(v)) {
                Some(kind) => {
                    lock(config).loop_kind = kind;
                    info!("control loop set to {kind:?} via HTTP");
                    text_response(request, 200, format!("OK: loop={kind:?}"));
                }
                None => text_response(
                    request,
                    400,
                    "Bad request: 'value' must be fan_temp|freq_temp|freq_power",
                ),
            }
        }
        "/status" => {
            let (mode, loop_kind, interval_ms, target_c) = {
                let cfg = lock(config);
                (cfg.mode, cfg.loop_kind, cfg.interval_ms, cfg.pid.target_c)
            };
            let guard = lock(status);
            json_response(
                request,
                &StatusView {
                    mode,
                    loop_kind,
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

fn assign_bool(
    params: &HashMap<String, String>,
    key: &str,
    dst: &mut bool,
    errors: &mut Vec<String>,
) {
    if let Some(raw) = params.get(key) {
        match raw.to_ascii_lowercase().as_str() {
            "true" | "1" | "on" => *dst = true,
            "false" | "0" | "off" => *dst = false,
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
    let mut errors: Vec<String> = Vec::new();

    // Route the shared parameter names to the ACTIVE loop: `[pid]` for the
    // fan loop, `[freq]` for the frequency loops (units per loop kind).
    match cfg.loop_kind {
        LoopKind::FanTemp => {
            let mut p = cfg.pid.clone();
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
            assign_f32(params, "idle_delta_c", &mut p.idle_delta_c, &mut errors);
            assign_f32(
                params,
                "write_deadband_percent",
                &mut p.write_deadband_percent,
                &mut errors,
            );
            assign_bool(params, "adaptive_base", &mut p.adaptive_base, &mut errors);
            if errors.is_empty() {
                if let Err(e) = p.validate() {
                    errors.push(e);
                } else {
                    cfg.pid = p;
                }
            }
        }
        LoopKind::FreqTemp | LoopKind::FreqPower => {
            let mut f = cfg.freq.clone();
            // `target_c` and `target` both address the setpoint (unit per
            // loop kind: degC for freq_temp, W for freq_power).
            let target_raw = params
                .get("target_c")
                .or_else(|| params.get("target"))
                .cloned();
            if let Some(raw) = target_raw {
                match raw.parse::<f32>() {
                    Ok(v) if v.is_finite() => f.target = v,
                    _ => errors.push("invalid 'target_c'".to_string()),
                }
            }
            assign_f32(params, "kp", &mut f.kp, &mut errors);
            assign_f32(params, "ki", &mut f.ki, &mut errors);
            assign_f32(params, "kd", &mut f.kd, &mut errors);
            assign_f32(params, "base_percent", &mut f.base_percent, &mut errors);
            assign_f32(params, "min_percent", &mut f.min_percent, &mut errors);
            assign_f32(params, "max_percent", &mut f.max_percent, &mut errors);
            assign_f32(
                params,
                "emergency_delta_c",
                &mut f.emergency_delta,
                &mut errors,
            );
            assign_f32(params, "idle_delta_c", &mut f.idle_delta, &mut errors);
            assign_f32(params, "temp_guard_c", &mut f.temp_guard_c, &mut errors);
            assign_f32(
                params,
                "write_deadband_percent",
                &mut f.write_deadband_percent,
                &mut errors,
            );
            assign_bool(params, "adaptive_base", &mut f.adaptive_base, &mut errors);
            assign_u32(params, "min_mhz", &mut f.min_mhz, &mut errors);
            assign_u32(params, "max_mhz", &mut f.max_mhz, &mut errors);
            if errors.is_empty() {
                if let Err(e) = f.validate() {
                    errors.push(e);
                } else {
                    cfg.freq = f;
                }
            }
        }
    }
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
    let (loop_name, target) = match cfg.loop_kind {
        LoopKind::FanTemp => ("fan_temp", cfg.pid.target_c),
        LoopKind::FreqTemp => ("freq_temp", cfg.freq.target),
        LoopKind::FreqPower => ("freq_power", cfg.freq.target),
    };
    info!(
        "PID updated via HTTP: loop={loop_name} target={target} gains={}",
        cfg_active_gains(&cfg)
    );
    text_response(request, 200, "OK: pid updated");
}

/// Gains of the active loop, for the audit log line.
fn cfg_active_gains(cfg: &RuntimeConfig) -> String {
    match cfg.loop_kind {
        LoopKind::FanTemp => format!(
            "{}/{}/{}/base={}",
            cfg.pid.kp, cfg.pid.ki, cfg.pid.kd, cfg.pid.base_percent
        ),
        LoopKind::FreqTemp | LoopKind::FreqPower => format!(
            "{}/{}/{}/base={}",
            cfg.freq.kp, cfg.freq.ki, cfg.freq.kd, cfg.freq.base_percent
        ),
    }
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
