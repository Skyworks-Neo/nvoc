//! Real TCP/HTTP integration: the host owns optimizer launch, logs and
//! cancellation through the live endpoints. The optimizer is a harmless
//! self-spawned fixture process; no GPU is touched and nothing is installed.
#![cfg(windows)]
use nvoc_srv::scan_api::Host;
use nvoc_srv::scan_process::{ProcessTree, ScanBackend};
use nvoc_srv::scans::Task;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

struct FakeBackend;
impl ScanBackend for FakeBackend {
    type Process = ProcessTree;
    fn validate(&self, _gpu: u32) -> Result<(), String> {
        Ok(())
    }
    fn spawn(&self, _task: &Task, directory: &Path) -> Result<ProcessTree, String> {
        ProcessTree::spawn(
            &std::env::current_exe().unwrap(),
            &[
                "--exact".into(),
                "fixture_fake_optimizer".into(),
                "--ignored".into(),
                "--nocapture".into(),
            ],
            directory,
        )
    }
    fn recover(&self, _gpu: u32) -> Result<(), String> {
        Ok(())
    }
}

#[test]
#[ignore = "subprocess fixture, invoked by the hosted HTTP tests"]
fn fixture_fake_optimizer() {
    std::fs::write("fake.pid", std::process::id().to_string()).unwrap();
    eprintln!("fake-optimizer-started pid={}", std::process::id());
    for i in 0..600 {
        println!("fake-progress-{i}");
        thread::sleep(Duration::from_millis(200));
    }
}

struct Root(PathBuf);
impl Root {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("nvoc-http-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const MANUAL: &str = "manual-token-0123456789abcdef0123456789";
const AUTOMATION: &str = "automation-token-0123456789abcdef01234567";

fn start(root: &Root) -> (Host, SocketAddr) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let tiny_http::ListenAddr::IP(addr) = server.server_addr();
    let host = Host::start_with(
        root.0.clone(),
        MANUAL.into(),
        AUTOMATION.into(),
        server,
        FakeBackend,
    )
    .unwrap();
    (host, addr)
}

fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: &str,
    body: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut text = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n"
    );
    if let Some(body) = body {
        text.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
    } else {
        text.push_str("\r\n");
    }
    stream.write_all(text.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let full = String::from_utf8_lossy(&raw).into_owned();
    let status = full
        .split_whitespace()
        .nth(1)
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    let body = full.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

fn get_json(addr: SocketAddr, path: &str, token: &str) -> Value {
    let (status, body) = request(addr, "GET", path, token, None);
    assert_eq!(status, 200, "GET {path}: {body}");
    serde_json::from_str(&body).unwrap()
}

fn pid_alive(pid: u32) -> bool {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

fn wait_for(deadline_secs: u64, step: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(deadline_secs);
    while !step() {
        assert!(Instant::now() < until, "condition not met in time");
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn hosted_scan_lifecycle_over_real_http() {
    let root = Root::new("lifecycle");
    let (host, addr) = start(&root);

    let (status, body) = request(
        addr,
        "POST",
        "/v1/scans",
        MANUAL,
        Some(r#"{"request_id":"e2e-hosting","gpu_id":0,"mode":"standard"}"#),
    );
    assert_eq!(status, 202, "{body}");
    let id = serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_u64()
        .unwrap();

    // The worker launches the optimizer through the backend: the fixture's pid
    // file only appears once ProcessTree::spawn has run inside the host worker.
    let directory = root.0.join(id.to_string());
    wait_for(15, || directory.join("fake.pid").exists());
    let pid: u32 = std::fs::read_to_string(directory.join("fake.pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(pid_alive(pid), "optimizer pid {pid} should be running");

    // srv captures the child's stdout; progress lines stream through the API.
    wait_for(10, || {
        get_json(addr, &format!("/v1/scans/{id}/log?offset=0"), MANUAL)["text"]
            .as_str()
            .unwrap()
            .contains("fake-progress-")
    });

    // Cancellation terminates the whole process tree and seals the state.
    let (status, body) = request(
        addr,
        "POST",
        &format!("/v1/scans/{id}/cancel"),
        MANUAL,
        Some(""),
    );
    assert_eq!(status, 202, "{body}");
    wait_for(15, || {
        get_json(addr, &format!("/v1/scans/{id}"), MANUAL)["state"] == "cancelled"
    });
    wait_for(10, || !pid_alive(pid));

    let task = get_json(addr, &format!("/v1/scans/{id}"), MANUAL);
    assert_eq!(task["state"], "cancelled");
    assert!(task["error"].is_null(), "{}", task["error"]);
    // A clean cancel performs no failed recovery, so no manual hold is set.
    assert_eq!(
        get_json(addr, "/v1/control", MANUAL)["manual_hold"],
        json!([])
    );

    host.stop();
}

#[test]
fn incomplete_submit_body_does_not_block_control() {
    let root = Root::new("stuck-reader");
    let (host, addr) = start(&root);

    let mut stuck = TcpStream::connect(addr).unwrap();
    stuck
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let partial = format!(
        "POST /v1/scans HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {MANUAL}\r\nContent-Length: 4096\r\n\r\n{{\"request_id\""
    );
    stuck.write_all(partial.as_bytes()).unwrap();

    // The submit-reader thread is parked on the incomplete body; control
    // endpoints and other POSTs must stay responsive meanwhile.
    let (status, body) = request(addr, "GET", "/v1/control", MANUAL, None);
    assert_eq!(status, 200, "{body}");
    let (status, body) = request(addr, "POST", "/v1/scans/999/cancel", MANUAL, Some(""));
    assert_eq!(status, 404, "{body}");

    // Releasing the socket frees the bounded reader; submissions work again.
    drop(stuck);
    let (status, body) = request(
        addr,
        "POST",
        "/v1/scans",
        MANUAL,
        Some(r#"{"request_id":"after-stuck","gpu_id":0,"mode":"ultrafast"}"#),
    );
    assert_eq!(status, 202, "{body}");

    // host.stop() cancels the accepted scan, kills its fixture and joins.
    host.stop();
}
