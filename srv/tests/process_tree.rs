#![cfg(windows)]
//! Harmless subprocess fixtures. These tests never load or modify a GPU.
use nvoc_srv::scan_process::ProcessTree;
use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0},
    System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "nvoc-job-test-{}-{}",
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
fn wait_child(ws: &Workspace) -> u32 {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(value) = std::fs::read_to_string(ws.0.join("child.pid"))
            && let Ok(pid) = value.parse()
        {
            return pid;
        }
        assert!(Instant::now() < until, "fixture child did not start");
        thread::sleep(Duration::from_millis(20));
    }
}
fn start(ws: &Workspace, fixture: &str) -> ProcessTree {
    ProcessTree::spawn(
        &std::env::current_exe().unwrap(),
        &[
            "--exact".into(),
            fixture.into(),
            "--ignored".into(),
            "--nocapture".into(),
        ],
        &ws.0,
    )
    .unwrap()
}

#[test]
fn stop_reaps_descendants_before_returning() {
    let ws = Workspace::new();
    let tree = start(&ws, "fixture_parent");
    let pid = wait_child(&ws);
    // SAFETY: this PID belongs to a fixture we just started; handles are closed.
    unsafe {
        let child = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        assert!(!child.is_null());
        tree.stop().unwrap();
        assert!(tree.poll().unwrap().is_some());
        assert_eq!(WaitForSingleObject(child, 5000), WAIT_OBJECT_0);
        CloseHandle(child);
    }
}
#[test]
fn closing_owner_kills_child_even_after_parent_exits() {
    let ws = Workspace::new();
    let tree = start(&ws, "fixture_exiting_parent");
    let pid = wait_child(&ws);
    let until = Instant::now() + Duration::from_secs(10);
    while tree.poll().unwrap().is_none() {
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(20));
    }
    unsafe {
        let child = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        assert!(!child.is_null());
        drop(tree);
        assert_eq!(WaitForSingleObject(child, 5000), WAIT_OBJECT_0);
        CloseHandle(child);
    }
}
#[test]
fn failed_spawn_does_not_leave_a_running_job() {
    let ws = Workspace::new();
    assert!(ProcessTree::spawn(&ws.0.join("missing.exe"), &[], &ws.0).is_err());
}

#[allow(clippy::zombie_processes)] // The owning Job Object, not this fixture, reaps the child.
fn spawn_child() {
    let _child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "fixture_child", "--ignored", "--nocapture"])
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !std::path::Path::new("child.pid").exists() {
        assert!(Instant::now() < until);
        thread::sleep(Duration::from_millis(20));
    }
}
#[test]
#[ignore = "subprocess fixture, invoked by the process-tree tests"]
fn fixture_parent() {
    spawn_child();
    thread::sleep(Duration::from_secs(60));
}
#[test]
#[ignore = "subprocess fixture, invoked by the process-tree tests"]
fn fixture_exiting_parent() {
    spawn_child();
}
#[test]
#[ignore = "subprocess fixture, invoked by the process-tree tests"]
fn fixture_child() {
    std::fs::write("child.pid", std::process::id().to_string()).unwrap();
    thread::sleep(Duration::from_secs(60));
}
