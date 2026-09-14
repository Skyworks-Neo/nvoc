//! Minimal HTTP/1.1 client for the nvoc-srv loopback control plane.
//!
//! The srv endpoints are plain GET/POST with query strings — a hand-written
//! request over `TcpStream` keeps the optimizer dependency-free (no HTTP
//! crate in the workspace). Every mutating call carries the
//! `X-Requested-With: XMLHttpRequest` header (srv's CSRF guard).

use anyhow::{Result, anyhow};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub const DEFAULT_PORT: u16 = 14514;
const IO_TIMEOUT: Duration = Duration::from_secs(3);

pub struct SrvClient {
    pub port: u16,
}

/// Outcome of [`SrvClient::probe_state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeState {
    /// Healthy srv with its control loop live (`gpus` non-empty).
    Ready,
    /// srv answers but discovery is still running (`gpus` empty).
    Starting,
    /// Nothing listens (connection refused) — safe to spawn.
    Absent,
}

/// Probe failure that is NOT "absent": the port answered with something we
/// cannot drive. Distinguishing this from [`ProbeState::Absent`] prevents
/// spawning a blind second instance on top of an unknown port occupant.
#[derive(Debug, Clone)]
pub struct ProbeError {
    pub message: String,
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl SrvClient {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    /// `POST path` with the CSRF header; returns the response body on 200.
    pub fn post(&self, path_and_query: &str) -> Result<String> {
        let (code, body) = self.raw_request("POST", path_and_query)?;
        if code < 400 {
            Ok(body)
        } else {
            Err(anyhow!("srv returned {code}: {}", body.trim()))
        }
    }

    /// Probe the control plane (tri-state; see [`ProbeState`]).
    pub fn probe_state(&self) -> Result<ProbeState, ProbeError> {
        match self.raw_request("GET", "/status") {
            Err(e)
                if e.downcast_ref::<io::Error>()
                    .is_some_and(|io| io.kind() == io::ErrorKind::ConnectionRefused) =>
            {
                Ok(ProbeState::Absent)
            }
            Err(e) => Err(ProbeError {
                message: e.to_string(),
            }),
            Ok((code, body)) => {
                if code >= 400 {
                    return Err(ProbeError {
                        message: format!("srv returned {code}: {}", body.trim()),
                    });
                }
                let ok =
                    serde_json::from_str::<serde_json::Value>(&body).map_err(|e| ProbeError {
                        message: format!("srv /status is not JSON: {e}"),
                    })?;
                let gpus = ok
                    .get("gpus")
                    .and_then(|g| g.as_array())
                    .ok_or_else(|| ProbeError {
                        message: "srv /status missing 'gpus'".into(),
                    })?;
                if gpus.is_empty() {
                    Ok(ProbeState::Starting)
                } else {
                    Ok(ProbeState::Ready)
                }
            }
        }
    }

    fn raw_request(&self, method: &str, path_and_query: &str) -> Result<(u16, String)> {
        let request = build_request(method, self.port, path_and_query);
        let mut stream = TcpStream::connect(("127.0.0.1", self.port))?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream.write_all(request.as_bytes())?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf)?;
        parse_response(&String::from_utf8_lossy(&buf))
    }
}

/// Build the wire request (separated for unit testing).
fn build_request(method: &str, port: u16, path_and_query: &str) -> String {
    let extra = if method == "POST" {
        // CSRF guard: mutations require the non-simple header.
        "X-Requested-With: XMLHttpRequest\r\n"
    } else {
        ""
    };
    format!(
        "{method} {path_and_query} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         {extra}\
         Connection: close\r\n\
         Content-Length: 0\r\n\
         \r\n"
    )
}

/// Split an HTTP response into (status_code, body); chunked transfer bodies
/// are not decoded — srv responses are small enough that tiny_http sends
/// them with Content-Length.
fn parse_response(raw: &str) -> Result<(u16, String)> {
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("srv response has no header/body separator"))?;
    let status_line = head.lines().next().unwrap_or_default();
    let code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("srv response has no status line: {status_line:?}"))?;
    Ok((code, body.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_request_carries_csrf_header() {
        let req = build_request("POST", 14514, "/pid?target_c=70");
        assert!(req.starts_with("POST /pid?target_c=70 HTTP/1.1\r\n"));
        assert!(req.contains("X-Requested-With: XMLHttpRequest\r\n"));
        assert!(req.contains("Host: 127.0.0.1:14514\r\n"));
        assert!(req.ends_with("Content-Length: 0\r\n\r\n"));
    }

    #[test]
    fn get_request_has_no_csrf_header() {
        let req = build_request("GET", 14514, "/status");
        assert!(req.starts_with("GET /status HTTP/1.1\r\n"));
        assert!(!req.contains("X-Requested-With"));
    }

    #[test]
    fn response_parsing() {
        let ok = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"gpus\":[]}";
        assert_eq!(
            parse_response(ok).unwrap(),
            (200, "{\"gpus\":[]}".to_string())
        );
        let bad = "HTTP/1.1 400 Bad Request\r\n\r\nBad request: nope";
        assert_eq!(
            parse_response(bad).unwrap(),
            (400, "Bad request: nope".to_string())
        );
        assert!(parse_response("garbage").is_err());
    }
}
