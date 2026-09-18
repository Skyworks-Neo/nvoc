//! Authenticated loopback API for hosted optimizer tasks, separate from legacy controls.
use crate::{
    scan_process,
    scans::{Manager, Origin, ScanRequest},
};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};
use tiny_http::{Header, Method, Request, Response, Server};

pub struct Host {
    pub manager: Arc<Manager>,
    worker: Option<thread::JoinHandle<()>>,
    http: Option<thread::JoinHandle<()>>,
    http_stop: Arc<AtomicBool>,
}
impl Host {
    pub fn start(
        root: PathBuf,
        executable: PathBuf,
        manual: String,
        automation: String,
    ) -> Result<Self, String> {
        if manual.len() < 32 || automation.len() < 32 || manual == automation {
            return Err(
                "two distinct scan API credentials of at least 32 characters are required".into(),
            );
        }
        let server = Server::http("127.0.0.1:14515").map_err(|e| e.to_string())?;
        Self::start_with(
            root,
            manual,
            automation,
            server,
            scan_process::NativeBackend { executable },
        )
    }
    pub fn start_with(
        root: PathBuf,
        manual: String,
        automation: String,
        server: Server,
        backend: impl scan_process::ScanBackend + Send + 'static,
    ) -> Result<Self, String> {
        let manager = Manager::open(root)?;
        let state = manager.clone();
        let worker = thread::spawn(move || scan_process::worker_with(state, backend));
        let state = manager.clone();
        let http_stop = Arc::new(AtomicBool::new(false));
        let stop = http_stop.clone();
        let reading_bodies = Arc::new(AtomicUsize::new(0));
        let http = thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match server.recv_timeout(Duration::from_millis(200)) {
                    Ok(Some(request)) => {
                        // A partial POST body must not block cancel/status. Bound
                        // body-reader threads; they never own a GPU or a process.
                        if request.method() == &Method::Post
                            && request.url().split('?').next() == Some("/v1/scans")
                        {
                            if reading_bodies.load(Ordering::Acquire) >= 4 {
                                let _ = request.respond(
                                    Response::from_string(
                                        "{\"error\":\"BUSY: request readers full\"}",
                                    )
                                    .with_status_code(503),
                                );
                                continue;
                            }
                            reading_bodies.fetch_add(1, Ordering::AcqRel);
                            let readers = reading_bodies.clone();
                            let state = state.clone();
                            let manual = manual.clone();
                            let automation = automation.clone();
                            thread::spawn(move || {
                                struct Guard(Arc<AtomicUsize>);
                                impl Drop for Guard {
                                    fn drop(&mut self) {
                                        self.0.fetch_sub(1, Ordering::AcqRel);
                                    }
                                }
                                let _guard = Guard(readers);
                                handle(request, &state, &manual, &automation);
                            });
                        } else {
                            handle(request, &state, &manual, &automation);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        log::error!("scan HTTP receive: {e}");
                        let _ = state.shutdown();
                        break;
                    }
                }
            }
        });
        Ok(Self {
            manager,
            worker: Some(worker),
            http: Some(http),
            http_stop,
        })
    }
    pub fn stop(mut self) {
        self.shutdown();
    }
    fn shutdown(&mut self) {
        if let Err(e) = self.manager.shutdown() {
            log::error!("scan shutdown journal: {e}");
        }
        self.http_stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if let Some(http) = self.http.take() {
            let _ = http.join();
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        if self.worker.is_some() || self.http.is_some() {
            self.shutdown();
        }
    }
}

fn credential(request: &Request, manual: &str, automation: &str) -> Result<Origin, String> {
    let token = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("authorization"))
        .and_then(|h| h.value.as_str().strip_prefix("Bearer "))
        .ok_or("UNAUTHORIZED")?;
    if token == manual {
        Ok(Origin::Manual)
    } else if token == automation {
        Ok(Origin::Automation)
    } else {
        Err("UNAUTHORIZED".into())
    }
}
fn handle(mut request: Request, manager: &Manager, manual: &str, automation: &str) {
    let result = credential(&request, manual, automation)
        .and_then(|origin| route(&mut request, manager, origin));
    let (code, body) = match result {
        Ok((code, value)) => (code, value),
        Err(error) => {
            let code = if error.starts_with("UNAUTHORIZED") {
                401
            } else if error.starts_with("FORBIDDEN") {
                403
            } else if error.contains("NOT_FOUND") {
                404
            } else if [
                "SCAN_BUSY",
                "MANUAL_CONTROL",
                "RECOVERY_REQUIRED",
                "CONFLICT",
                "STOPPING",
            ]
            .iter()
            .any(|s| error.starts_with(s))
            {
                409
            } else {
                400
            };
            (code, json!({"error": error}))
        }
    };
    let response = Response::from_string(body.to_string())
        .with_status_code(code)
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap());
    if let Err(e) = request.respond(response) {
        log::debug!("scan client disconnected: {e}");
    }
}
fn route(request: &mut Request, manager: &Manager, origin: Origin) -> Result<(u16, Value), String> {
    let url = request.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    if request.method() == &Method::Get {
        return match parts.as_slice() {
            ["v1", "scans"] => Ok((
                200,
                json!({"tasks": manager.registry.lock().unwrap().tasks.values().collect::<Vec<_>>()}),
            )),
            ["v1", "control"] => {
                let r = manager.registry.lock().unwrap();
                Ok((
                    200,
                    json!({"manual_hold": r.manual_hold, "recovery_required": r.recovery_required}),
                ))
            }
            ["v1", "scans", id] => Ok((200, json!(manager.get(parse_id(id)?)?))),
            ["v1", "scans", id, "log"] => {
                let id = parse_id(id)?;
                manager.get(id)?;
                let offset = query
                    .strip_prefix("offset=")
                    .unwrap_or("0")
                    .parse::<u64>()
                    .map_err(|_| "INVALID_ARGUMENT: offset")?;
                let path = manager.directory(id).join("output.log");
                if !path.exists() {
                    return Ok((200, json!({"text":"", "next_offset":0})));
                }
                let mut f = File::open(path).map_err(|e| e.to_string())?;
                let offset = offset.min(f.metadata().map_err(|e| e.to_string())?.len());
                f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
                let mut bytes = Vec::new();
                f.take(64 * 1024)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                // Keep an incomplete trailing UTF-8 codepoint for the next poll.
                if !manager.get(id)?.state.terminal()
                    && let Err(error) = std::str::from_utf8(&bytes)
                    && error.error_len().is_none()
                {
                    bytes.truncate(error.valid_up_to());
                }
                Ok((
                    200,
                    json!({"text": String::from_utf8_lossy(&bytes), "next_offset": offset + bytes.len() as u64}),
                ))
            }
            ["v1", "scans", id, "result"] => {
                let id = parse_id(id)?;
                let task = manager.get(id)?;
                let path = manager.directory(id).join("scan").join("vfp-final.csv");
                let csv = if path.exists() {
                    let mut text = String::new();
                    File::open(path)
                        .map_err(|e| e.to_string())?
                        .take(4 * 1024 * 1024)
                        .read_to_string(&mut text)
                        .map_err(|e| e.to_string())?;
                    Some(text)
                } else {
                    None
                };
                Ok((
                    200,
                    json!({"task":task, "final_csv": csv, "directory": manager.directory(id)}),
                ))
            }
            _ => Err("NOT_FOUND".into()),
        };
    }
    if request.method() != &Method::Post {
        return Err("INVALID_METHOD: use GET or POST".into());
    }
    // No CORS. Bearer authentication cannot be supplied by a cross-origin HTML form.
    match parts.as_slice() {
        ["v1", "scans"] => {
            if request.body_length().is_none_or(|n| n > 4096) {
                return Err("INVALID_ARGUMENT: Content-Length of at most 4096 is required".into());
            }
            let mut bytes = Vec::new();
            request
                .as_reader()
                .take(4097)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 4096 {
                return Err("INVALID_ARGUMENT: body too large".into());
            }
            let input: ScanRequest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            Ok((202, json!(manager.submit(input, origin)?)))
        }
        ["v1", "scans", id, "cancel"] => Ok((202, json!(manager.cancel(parse_id(id)?, origin)?))),
        ["v1", "control", gpu, action] => {
            if origin != Origin::Manual {
                return Err("FORBIDDEN".into());
            }
            let gpu = gpu.parse::<u32>().map_err(|_| "INVALID_ARGUMENT: GPU ID")?;
            match *action {
                "takeover" => manager.takeover(gpu)?,
                "release" => manager.release(gpu)?,
                "recover" => {
                    return Ok((202, json!(manager.request_recovery(gpu)?)));
                }
                _ => return Err("NOT_FOUND".into()),
            }
            Ok((200, json!({"accepted":true, "busy":manager.busy()})))
        }
        _ => Err("NOT_FOUND".into()),
    }
}
fn parse_id(id: &str) -> Result<u64, String> {
    id.parse().map_err(|_| "INVALID_ARGUMENT: task ID".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use tiny_http::TestRequest;
    struct Workspace(PathBuf);
    impl Workspace {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "nvoc-api-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for Workspace {
        fn drop(&mut self) {
            let base = std::env::temp_dir().canonicalize().unwrap();
            assert!(self.0.starts_with(&base) && self.0 != base);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    const BODY: &str = r#"{"request_id":"api-test","gpu_id":256,"mode":"standard"}"#;
    fn post(path: &str, body: &'static str) -> Request {
        TestRequest::new()
            .with_method(Method::Post)
            .with_path(path)
            .with_body(body)
            .into()
    }
    #[test]
    fn credentials_determine_role_not_request_parameters() {
        let unknown: Request = TestRequest::new().into();
        assert_eq!(
            credential(&unknown, "manual", "auto").unwrap_err(),
            "UNAUTHORIZED"
        );
        let known: Request = TestRequest::new()
            .with_header(Header::from_bytes("Authorization", "Bearer auto").unwrap())
            .into();
        assert_eq!(
            credential(&known, "manual", "auto").unwrap(),
            Origin::Automation
        );
        let ws = Workspace::new();
        let m = Manager::open(ws.0.clone()).unwrap();
        assert_eq!(
            route(
                &mut post("/v1/control/256/takeover", ""),
                &m,
                Origin::Automation
            )
            .unwrap_err(),
            "FORBIDDEN"
        );
        assert!(
            route(
                &mut post(
                    "/v1/scans",
                    r#"{"request_id":"a","gpu_id":256,"mode":"standard","origin":"manual"}"#
                ),
                &m,
                Origin::Automation
            )
            .is_err()
        );
    }
    #[test]
    fn submit_status_cancel_and_repeat_cancel() {
        let ws = Workspace::new();
        let m = Manager::open(ws.0.clone()).unwrap();
        let (status, body) = route(&mut post("/v1/scans", BODY), &m, Origin::Manual).unwrap();
        assert_eq!(status, 202);
        let id = body["id"].as_u64().unwrap();
        let (_, repeated) = route(&mut post("/v1/scans", BODY), &m, Origin::Manual).unwrap();
        assert_eq!(body, repeated);
        let mut get: Request = TestRequest::new()
            .with_path(&format!("/v1/scans/{id}"))
            .into();
        assert_eq!(
            route(&mut get, &m, Origin::Manual).unwrap().1["state"],
            "queued"
        );
        for _ in 0..2 {
            let (_, cancelled) = route(
                &mut post(&format!("/v1/scans/{id}/cancel"), ""),
                &m,
                Origin::Manual,
            )
            .unwrap();
            assert_eq!(cancelled["state"], "cancelling");
        }
    }
    #[test]
    fn oversized_or_unrecognized_requests_are_rejected() {
        let ws = Workspace::new();
        let m = Manager::open(ws.0.clone()).unwrap();
        assert!(route(&mut post("/v1/scans", "{}"), &m, Origin::Manual).is_err());
        assert!(
            route(
                &mut post("/v1/scans/../../file/cancel", ""),
                &m,
                Origin::Manual
            )
            .is_err()
        );
        assert!(
            route(
                &mut post(
                    "/v1/scans",
                    r#"{"request_id":"a","gpu_id":256,"mode":"bogus"}"#
                ),
                &m,
                Origin::Manual
            )
            .is_err()
        );
    }
    #[test]
    fn log_reading_is_bounded_and_offsets_are_bytes() {
        let ws = Workspace::new();
        let m = Manager::open(ws.0.clone()).unwrap();
        let task = m
            .submit(serde_json::from_str(BODY).unwrap(), Origin::Manual)
            .unwrap();
        std::fs::create_dir_all(m.directory(task.id)).unwrap();
        let text = "中".repeat(30_000);
        std::fs::write(m.directory(task.id).join("output.log"), &text).unwrap();
        let mut req: Request = TestRequest::new()
            .with_path(&format!("/v1/scans/{}/log?offset=0", task.id))
            .into();
        let (_, page) = route(&mut req, &m, Origin::Manual).unwrap();
        let first = page["text"].as_str().unwrap();
        assert!(!first.contains('\u{fffd}'));
        assert!(first.len() <= 65536);
        assert_eq!(page["next_offset"].as_u64().unwrap(), first.len() as u64);
    }
}
