use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Response, Server};
use serde::{Deserialize, Serialize};
use log::{error, info, warn};

// Accepted temperature-limit range (°C) for /set_temp_limit_soft_vfp.
const TEMP_LIMIT_MIN: u32 = 40;
const TEMP_LIMIT_MAX: u32 = 120;

// Accepted OC frequency-delta range (kHz) for /oc_global.
// ±2 000 MHz covers every realistic consumer-GPU overclock without risking i32 overflow.
const OC_DELTA_MIN: i32 = -2_000_000;
const OC_DELTA_MAX: i32 = 2_000_000;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NVOCServiceConfig {
    pub vfp_lock_point: usize,
    pub temp_limit: u32,
}

pub struct NVOCServiceCmd {
    pub gpu_index: usize,
    pub cmd: String,
    pub over_freq: i32,
}

/// Decode a percent-encoded byte sequence (application/x-www-form-urlencoded style).
/// `+` → space, `%XX` → byte 0xXX; invalid escapes are passed through as-is.
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

/// Parse a `key=value&key=value` query string into a flat map with percent-decoded values.
/// Duplicate keys keep the last value; keys without '=' get an empty string value.
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

/// Send a response, logging on failure (tiny_http returns an error if the client disconnected).
fn respond(request: tiny_http::Request, response: Response<std::io::Cursor<Vec<u8>>>) {
    if let Err(e) = request.respond(response) {
        warn!("HTTP: failed to send response: {}", e);
    }
}

fn json_content_type() -> Header {
    Header::from_bytes("Content-Type", "application/json")
        .expect("static header is valid ASCII")
}

pub fn start_http_server(
    config: Arc<Mutex<NVOCServiceConfig>>,
    cmd_tx: flume::Sender<NVOCServiceCmd>,
) {
    // Bind only to loopback; network-reachable clients cannot reach this endpoint.
    let server = match Server::http("127.0.0.1:14514") {
        Ok(s) => s,
        Err(e) => {
            error!("HTTP server failed to bind on 127.0.0.1:14514: {}", e);
            return;
        }
    };
    info!("HTTP config server listening on 127.0.0.1:14514");

    for request in server.incoming_requests() {
        let full_url = request.url().to_owned();
        let (path, query_str) = match full_url.split_once('?') {
            Some((p, q)) => (p, q),
            None => (full_url.as_str(), ""),
        };
        let params = parse_query(query_str);

        match path {
            "/config" => {
                let response = match config.lock() {
                    Ok(cfg) => match serde_json::to_string(&*cfg) {
                        Ok(json) => Response::from_string(json)
                            .with_status_code(200)
                            .with_header(json_content_type()),
                        Err(e) => {
                            error!("Failed to serialize config: {}", e);
                            Response::from_string("Internal error").with_status_code(500)
                        }
                    },
                    Err(e) => {
                        error!("Config mutex poisoned: {}", e);
                        Response::from_string("Internal error").with_status_code(500)
                    }
                };
                respond(request, response);
            }

            "/set_temp_limit_soft_vfp" => {
                // Accepts: ?limit=<u32>   Range: TEMP_LIMIT_MIN–TEMP_LIMIT_MAX °C
                let response = match params.get("limit").and_then(|s| s.parse::<u32>().ok()) {
                    Some(limit) if (TEMP_LIMIT_MIN..=TEMP_LIMIT_MAX).contains(&limit) => {
                        match config.lock() {
                            Ok(mut cfg) => {
                                cfg.temp_limit = limit;
                                info!("Temp limit updated to {}°C", limit);
                                Response::from_string("OK").with_status_code(200)
                            }
                            Err(e) => {
                                error!("Config mutex poisoned: {}", e);
                                Response::from_string("Internal error").with_status_code(500)
                            }
                        }
                    }
                    Some(limit) => {
                        warn!(
                            "Rejected temp limit {} (valid range: {}–{}°C)",
                            limit, TEMP_LIMIT_MIN, TEMP_LIMIT_MAX
                        );
                        Response::from_string(format!(
                            "Bad request: limit must be {TEMP_LIMIT_MIN}–{TEMP_LIMIT_MAX}"
                        ))
                        .with_status_code(400)
                    }
                    None => {
                        warn!("Missing or non-numeric 'limit' query parameter");
                        Response::from_string("Bad request: missing or invalid 'limit'")
                            .with_status_code(400)
                    }
                };
                respond(request, response);
            }

            "/oc_global" => {
                // Accepts: ?oc=<i32 kHz delta>[&gpu=<usize index>]
                // gpu defaults to 0 when absent; a present but non-numeric value is rejected.
                let gpu_index: usize = match params.get("gpu") {
                    None => 0,
                    Some(s) => match s.parse::<usize>() {
                        Ok(n) => n,
                        Err(_) => {
                            warn!("Rejected non-numeric 'gpu' parameter: {:?}", s);
                            respond(
                                request,
                                Response::from_string(format!(
                                    "Bad request: 'gpu' must be a non-negative integer, got {:?}",
                                    s
                                ))
                                .with_status_code(400),
                            );
                            continue;
                        }
                    },
                };

                let response = match params.get("oc").and_then(|s| s.parse::<i32>().ok()) {
                    Some(freq_val) if (OC_DELTA_MIN..=OC_DELTA_MAX).contains(&freq_val) => {
                        match cmd_tx.send(NVOCServiceCmd {
                            gpu_index,
                            cmd: "set_oc_global".to_string(),
                            over_freq: freq_val,
                        }) {
                            Ok(_) => {
                                info!("GPU {}: queued OC delta {} kHz", gpu_index, freq_val);
                                Response::from_string("OK").with_status_code(200)
                            }
                            Err(e) => {
                                error!("Failed to enqueue OC command: {}", e);
                                Response::from_string("Internal error").with_status_code(500)
                            }
                        }
                    }
                    Some(freq_val) => {
                        warn!(
                            "Rejected OC delta {} kHz (valid range: {}–{} kHz)",
                            freq_val, OC_DELTA_MIN, OC_DELTA_MAX
                        );
                        Response::from_string(format!(
                            "Bad request: oc must be {OC_DELTA_MIN}–{OC_DELTA_MAX} kHz"
                        ))
                        .with_status_code(400)
                    }
                    None => {
                        warn!("Missing or non-numeric 'oc' query parameter");
                        Response::from_string("Bad request: missing or invalid 'oc'")
                            .with_status_code(400)
                    }
                };
                respond(request, response);
            }

            _ => {
                respond(
                    request,
                    Response::from_string("Not found").with_status_code(404),
                );
            }
        }
    }
}
