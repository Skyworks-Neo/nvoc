use nvoc_srv::scans::{Manager, Mode, Origin, ScanRequest, State};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "nvoc-srv-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
    fn manager(&self) -> Arc<Manager> {
        Manager::open(self.0.join("state")).unwrap()
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let parent = std::env::temp_dir().canonicalize().unwrap();
        assert!(self.0.starts_with(&parent) && self.0 != parent);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn request(id: &str) -> ScanRequest {
    ScanRequest {
        request_id: id.into(),
        gpu_id: 256,
        mode: Mode::Standard,
    }
}

#[test]
fn retry_returns_same_task_but_changed_payload_is_rejected() {
    let ws = Workspace::new();
    let m = ws.manager();
    let first = m.submit(request("a"), Origin::Manual).unwrap();
    assert_eq!(m.submit(request("a"), Origin::Manual).unwrap().id, first.id);
    let mut changed = request("a");
    changed.mode = Mode::Legacy;
    assert!(
        m.submit(changed, Origin::Manual)
            .unwrap_err()
            .starts_with("CONFLICT")
    );
    assert_eq!(
        m.submit(request("b"), Origin::Manual).unwrap_err(),
        "SCAN_BUSY"
    );
}

#[test]
fn concurrent_admission_runs_only_one_task() {
    let ws = Workspace::new();
    let m = ws.manager();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let m = m.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                m.submit(request(&format!("r{i}")), Origin::Manual).is_ok()
            })
        })
        .collect();
    assert_eq!(
        threads
            .into_iter()
            .filter_map(|t| t.join().ok())
            .filter(|accepted| *accepted)
            .count(),
        1
    );
}

#[test]
fn manual_takeover_is_sticky_and_does_not_cancel_manual_tasks() {
    let ws = Workspace::new();
    let m = ws.manager();
    let task = m.submit(request("a"), Origin::Automation).unwrap();
    m.takeover(256).unwrap();
    assert_eq!(m.get(task.id).unwrap().state, State::Cancelling);
    assert_eq!(m.release(256).unwrap_err(), "SCAN_BUSY");
    m.finish(task.id, None, None, true).unwrap();
    assert_eq!(
        m.submit(request("b"), Origin::Automation).unwrap_err(),
        "MANUAL_CONTROL"
    );
    let manual = m.submit(request("c"), Origin::Manual).unwrap();
    m.takeover(256).unwrap();
    assert_eq!(m.get(manual.id).unwrap().state, State::Queued);
    assert_eq!(
        m.cancel(manual.id, Origin::Automation).unwrap_err(),
        "FORBIDDEN"
    );
    m.finish(manual.id, Some(0), None, true).unwrap();
    m.release(256).unwrap();
    m.submit(request("d"), Origin::Automation).unwrap();
}

#[test]
fn cancellation_cannot_rewrite_sealed_or_terminal_result() {
    let ws = Workspace::new();
    let m = ws.manager();
    let t = m.submit(request("a"), Origin::Manual).unwrap();
    assert!(!m.begin_recovery(t.id).unwrap());
    assert_eq!(m.cancel(t.id, Origin::Manual).unwrap().cancel_reason, None);
    m.finish(t.id, Some(0), None, true).unwrap();
    m.cancel(t.id, Origin::Manual).unwrap();
    m.finish(t.id, None, Some("late error".into()), false)
        .unwrap();
    assert_eq!(m.get(t.id).unwrap().state, State::Succeeded);
}

#[test]
fn restart_requires_explicit_recovery_and_release() {
    let ws = Workspace::new();
    let m = ws.manager();
    let t = m.submit(request("a"), Origin::Automation).unwrap();
    drop(m);
    let m = ws.manager();
    assert_eq!(m.get(t.id).unwrap().state, State::Interrupted);
    assert_eq!(
        m.submit(request("b"), Origin::Manual).unwrap_err(),
        "RECOVERY_REQUIRED"
    );
    let recovery = m.request_recovery(256).unwrap();
    m.begin_recovery(recovery.id).unwrap();
    m.finish(recovery.id, None, None, true).unwrap();
    assert_eq!(
        m.submit(request("c"), Origin::Automation).unwrap_err(),
        "MANUAL_CONTROL"
    );
    m.release(256).unwrap();
    m.submit(request("d"), Origin::Automation).unwrap();
}

#[test]
fn journal_failure_does_not_admit_a_task() {
    let ws = Workspace::new();
    let m = ws.manager();
    std::fs::create_dir(m.root.join("tasks.json.tmp")).unwrap();
    assert!(m.submit(request("a"), Origin::Manual).is_err());
    assert!(!m.busy());
    assert!(m.registry.lock().unwrap().stopping);
}

#[test]
fn shutdown_prevents_new_tasks_and_cancels_existing() {
    let ws = Workspace::new();
    let m = ws.manager();
    let t = m.submit(request("a"), Origin::Manual).unwrap();
    m.shutdown().unwrap();
    assert_eq!(m.get(t.id).unwrap().state, State::Cancelling);
    assert_eq!(
        m.submit(request("b"), Origin::Manual).unwrap_err(),
        "STOPPING"
    );
}

#[cfg(windows)]
mod worker_tests {
    use super::*;
    use nvoc_srv::scan_process::{RunningScan, ScanBackend, execute};
    use nvoc_srv::scans::Task;
    use std::{cell::RefCell, rc::Rc};
    struct Mock {
        calls: Rc<RefCell<Vec<&'static str>>>,
        manager: Arc<Manager>,
        task: u64,
        fail_recovery: bool,
        cancel_on_poll: bool,
        code: i32,
    }
    struct Process {
        calls: Rc<RefCell<Vec<&'static str>>>,
        manager: Arc<Manager>,
        task: u64,
        cancel: bool,
        code: i32,
    }
    impl RunningScan for Process {
        fn poll(&self) -> Result<Option<i32>, String> {
            if self.cancel {
                self.manager.cancel(self.task, Origin::Manual)?;
            }
            Ok(Some(self.code))
        }
        fn stop(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("stop_tree");
            Ok(())
        }
    }
    impl ScanBackend for Mock {
        type Process = Process;
        fn validate(&self, _: u32) -> Result<(), String> {
            self.calls.borrow_mut().push("validate");
            Ok(())
        }
        fn spawn(&self, _: &Task, _: &std::path::Path) -> Result<Process, String> {
            self.calls.borrow_mut().push("spawn");
            Ok(Process {
                calls: self.calls.clone(),
                manager: self.manager.clone(),
                task: self.task,
                cancel: self.cancel_on_poll,
                code: self.code,
            })
        }
        fn recover(&self, _: u32) -> Result<(), String> {
            self.calls.borrow_mut().push("recover");
            assert_eq!(
                self.manager.get(self.task).unwrap().state,
                State::Recovering
            );
            assert!(self.manager.busy());
            if self.fail_recovery {
                Err("reset failed".into())
            } else {
                Ok(())
            }
        }
    }
    fn run(cancel: bool, failed: bool, code: i32) -> (State, Vec<&'static str>, bool) {
        let ws = Workspace::new();
        let manager = ws.manager();
        let task = manager.submit(request("a"), Origin::Manual).unwrap();
        let backend = Mock {
            calls: Rc::new(RefCell::new(Vec::new())),
            manager: manager.clone(),
            task: task.id,
            fail_recovery: failed,
            cancel_on_poll: cancel,
            code,
        };
        execute(&manager, &backend, &task).unwrap();
        let calls = backend.calls.borrow().clone();
        let blocked = manager
            .registry
            .lock()
            .unwrap()
            .recovery_required
            .contains(&256);
        (manager.get(task.id).unwrap().state, calls, blocked)
    }
    #[test]
    fn cancel_stops_tree_before_recovery_and_releasing_admission() {
        assert_eq!(
            run(true, false, 0),
            (
                State::Cancelled,
                vec!["validate", "spawn", "stop_tree", "recover"],
                false
            )
        );
    }
    #[test]
    fn success_keeps_optimizer_result_and_stops_leftover_children() {
        assert_eq!(
            run(false, false, 0),
            (
                State::Succeeded,
                vec!["validate", "spawn", "stop_tree"],
                false
            )
        );
    }
    #[test]
    fn failed_recovery_blocks_future_scans() {
        assert_eq!(run(true, true, 0).0, State::Failed);
        assert!(run(true, true, 0).2);
    }
    #[test]
    fn optimizer_failure_recovers_but_preserves_failure_result() {
        assert_eq!(
            run(false, false, 7),
            (
                State::Failed,
                vec!["validate", "spawn", "stop_tree", "recover"],
                false
            )
        );
    }
    #[test]
    fn queued_cancel_does_not_touch_hardware() {
        let ws = Workspace::new();
        let manager = ws.manager();
        let task = manager.submit(request("a"), Origin::Manual).unwrap();
        let task = manager.cancel(task.id, Origin::Manual).unwrap();
        let backend = Mock {
            calls: Rc::new(RefCell::new(Vec::new())),
            manager: manager.clone(),
            task: task.id,
            fail_recovery: false,
            cancel_on_poll: false,
            code: 0,
        };
        execute(&manager, &backend, &task).unwrap();
        assert!(backend.calls.borrow().is_empty());
        assert_eq!(manager.get(task.id).unwrap().state, State::Cancelled);
    }
}
