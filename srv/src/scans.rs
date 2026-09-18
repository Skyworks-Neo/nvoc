//! Transport-independent hosted optimizer state. GUI/TUI ownership is unchanged.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Manual,
    Automation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Standard,
    Ultrafast,
    Legacy,
}
impl Mode {
    pub fn argument(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Ultrafast => "ultrafast",
            Self::Legacy => "legacy",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScanRequest {
    pub request_id: String,
    pub gpu_id: u32,
    pub mode: Mode,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Queued,
    Running,
    Cancelling,
    Recovering,
    Succeeded,
    Cancelled,
    Failed,
    Interrupted,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    #[default]
    Scan,
    Recovery,
}
impl State {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Cancelled | Self::Failed | Self::Interrupted
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    #[serde(default)]
    pub kind: TaskKind,
    pub id: u64,
    pub request: ScanRequest,
    pub origin: Origin,
    pub state: State,
    pub error: Option<String>,
    pub cancel_reason: Option<String>,
    pub exit_code: Option<i32>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    pub tasks: BTreeMap<u64, Task>,
    pub manual_hold: BTreeSet<u32>,
    pub recovery_required: BTreeSet<u32>,
    next_id: u64,
    #[serde(skip)]
    pub stopping: bool,
}
pub struct Manager {
    pub registry: Mutex<Registry>,
    /// Held by service writes and admission, never for the duration of a scan.
    pub admission: Mutex<()>,
    pub root: PathBuf,
}
impl Manager {
    pub fn open(root: PathBuf) -> Result<Arc<Self>, String> {
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let path = root.join("tasks.json");
        let mut r: Registry = if path.exists() {
            serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?
        } else {
            Registry::default()
        };
        for task in r.tasks.values_mut().filter(|t| !t.state.terminal()) {
            task.state = State::Interrupted;
            task.error = Some("Service restarted; explicit recovery required".into());
            r.recovery_required.insert(task.request.gpu_id);
            r.manual_hold.insert(task.request.gpu_id);
        }
        let manager = Arc::new(Self {
            registry: Mutex::new(r),
            admission: Mutex::new(()),
            root,
        });
        manager.persist()?;
        Ok(manager)
    }
    pub fn persist(&self) -> Result<(), String> {
        self.save(&self.registry.lock().unwrap())
    }
    fn save(&self, r: &Registry) -> Result<(), String> {
        let data = serde_json::to_vec_pretty(r).map_err(|e| e.to_string())?;
        let temp = self.root.join("tasks.json.tmp");
        let dest = self.root.join("tasks.json");
        let mut f = fs::File::create(&temp).map_err(|e| e.to_string())?;
        f.write_all(&data)
            .and_then(|_| f.sync_all())
            .map_err(|e| e.to_string())?;
        drop(f);
        #[cfg(windows)]
        if dest.exists() {
            use std::os::windows::ffi::OsStrExt;
            let src: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let dst: Vec<u16> = dest.as_os_str().encode_wide().chain(Some(0)).collect();
            // SAFETY: paths are terminated and live for the call; optional args null.
            if unsafe {
                windows_sys::Win32::Storage::FileSystem::ReplaceFileW(
                    dst.as_ptr(),
                    src.as_ptr(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                )
            } == 0
            {
                return Err(std::io::Error::last_os_error().to_string());
            }
            return Ok(());
        }
        fs::rename(temp, dest).map_err(|e| e.to_string())
    }
    pub fn submit(&self, request: ScanRequest, origin: Origin) -> Result<Task, String> {
        if request.request_id.is_empty()
            || request.request_id.len() > 128
            || !request
                .request_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
        {
            return Err(
                "INVALID_ARGUMENT: request_id must be 1..128 ASCII letters, digits, '-' or '_'"
                    .into(),
            );
        }
        let _admission = self.admission.lock().unwrap();
        let mut r = self.registry.lock().unwrap();
        if let Some(t) = r
            .tasks
            .values()
            .find(|t| t.origin == origin && t.request.request_id == request.request_id)
        {
            return if t.request == request {
                Ok(t.clone())
            } else {
                Err("CONFLICT: request_id reused with different parameters".into())
            };
        }
        if r.stopping {
            return Err("STOPPING".into());
        }
        if !r.recovery_required.is_empty() {
            return Err("RECOVERY_REQUIRED".into());
        }
        if origin == Origin::Automation && r.manual_hold.contains(&request.gpu_id) {
            return Err("MANUAL_CONTROL".into());
        }
        // Existing optimizer recovery can reset the driver, so hosted scans are
        // globally exclusive until recovery is reliably device-scoped.
        if r.tasks.values().any(|t| !t.state.terminal()) {
            return Err("SCAN_BUSY".into());
        }
        if r.tasks.len() >= 10_000 {
            return Err("TASK_LIMIT".into());
        }
        r.next_id += 1;
        let task = Task {
            kind: TaskKind::Scan,
            id: r.next_id,
            request,
            origin,
            state: State::Queued,
            error: None,
            cancel_reason: None,
            exit_code: None,
        };
        r.tasks.insert(task.id, task.clone());
        if let Err(e) = self.save(&r) {
            r.tasks.remove(&task.id);
            r.stopping = true;
            return Err(e);
        }
        Ok(task)
    }
    pub fn get(&self, id: u64) -> Result<Task, String> {
        self.registry
            .lock()
            .unwrap()
            .tasks
            .get(&id)
            .cloned()
            .ok_or_else(|| "TASK_NOT_FOUND".into())
    }
    pub fn busy(&self) -> bool {
        self.registry
            .lock()
            .unwrap()
            .tasks
            .values()
            .any(|t| !t.state.terminal())
    }
    pub fn cancel(&self, id: u64, origin: Origin) -> Result<Task, String> {
        let mut r = self.registry.lock().unwrap();
        let t = r.tasks.get_mut(&id).ok_or("TASK_NOT_FOUND")?;
        if origin == Origin::Automation && t.origin != Origin::Automation {
            return Err("FORBIDDEN".into());
        }
        if matches!(t.state, State::Queued | State::Running | State::Cancelling)
            && t.kind == TaskKind::Scan
        {
            t.cancel_reason = Some("cancel_requested".into());
            t.state = State::Cancelling;
        }
        let task = t.clone();
        self.save(&r)?;
        Ok(task)
    }
    pub fn takeover(&self, gpu: u32) -> Result<(), String> {
        let mut r = self.registry.lock().unwrap();
        r.manual_hold.insert(gpu);
        for t in r.tasks.values_mut().filter(|t| {
            t.request.gpu_id == gpu
                && matches!(t.state, State::Queued | State::Running | State::Cancelling)
                && t.origin == Origin::Automation
        }) {
            t.cancel_reason = Some("manual_takeover".into());
            t.state = State::Cancelling;
        }
        self.save(&r)
    }
    pub fn release(&self, gpu: u32) -> Result<(), String> {
        let mut r = self.registry.lock().unwrap();
        if r.recovery_required.contains(&gpu) {
            return Err("RECOVERY_REQUIRED".into());
        }
        if r.tasks
            .values()
            .any(|t| t.request.gpu_id == gpu && !t.state.terminal())
        {
            return Err("SCAN_BUSY".into());
        }
        let mut next = r.clone();
        next.manual_hold.remove(&gpu);
        self.save(&next)?;
        *r = next;
        Ok(())
    }
    pub fn shutdown(&self) -> Result<(), String> {
        let mut r = self.registry.lock().unwrap();
        r.stopping = true;
        for t in r.tasks.values_mut().filter(|t| {
            matches!(t.state, State::Queued | State::Running | State::Cancelling)
                && t.kind == TaskKind::Scan
        }) {
            t.state = State::Cancelling;
            t.cancel_reason = Some("service_shutdown".into());
        }
        self.save(&r)
    }
    pub fn update(&self, id: u64, update: impl FnOnce(&mut Task)) -> Result<(), String> {
        let mut r = self.registry.lock().unwrap();
        update(r.tasks.get_mut(&id).ok_or("TASK_NOT_FOUND")?);
        self.save(&r)
    }
    pub fn finish(
        &self,
        id: u64,
        exit_code: Option<i32>,
        error: Option<String>,
        recovered: bool,
    ) -> Result<(), String> {
        let mut r = self.registry.lock().unwrap();
        let t = r.tasks.get_mut(&id).ok_or("TASK_NOT_FOUND")?;
        if t.state.terminal() {
            return Ok(());
        }
        t.exit_code = exit_code;
        t.error = error;
        t.state = if t.error.is_some() {
            State::Failed
        } else if t.cancel_reason.is_some() {
            State::Cancelled
        } else {
            State::Succeeded
        };
        let gpu = t.request.gpu_id;
        let clear_recovery = t.kind == TaskKind::Recovery && t.state == State::Succeeded;
        if !recovered {
            r.recovery_required.insert(gpu);
            r.manual_hold.insert(gpu);
        }
        if clear_recovery {
            r.recovery_required.remove(&gpu);
        }
        if let Err(e) = self.save(&r) {
            r.stopping = true;
            r.recovery_required.insert(gpu);
            r.manual_hold.insert(gpu);
            return Err(e);
        }
        Ok(())
    }
    /// Seal the cancellation decision only after the entire process tree exits.
    /// A later cancel cannot turn successful completion into an unperformed reset.
    pub fn begin_recovery(&self, id: u64) -> Result<bool, String> {
        let mut r = self.registry.lock().unwrap();
        let t = r.tasks.get_mut(&id).ok_or("TASK_NOT_FOUND")?;
        let cancelled = t.cancel_reason.is_some();
        t.state = State::Recovering;
        self.save(&r)?;
        Ok(cancelled)
    }
    pub fn request_recovery(&self, gpu: u32) -> Result<Task, String> {
        let _admission = self.admission.lock().unwrap();
        let mut r = self.registry.lock().unwrap();
        if r.stopping {
            return Err("STOPPING".into());
        }
        if r.tasks.values().any(|t| !t.state.terminal()) {
            return Err("SCAN_BUSY".into());
        }
        if !r.recovery_required.contains(&gpu) {
            return Err("INVALID_ARGUMENT: no recovery required for this GPU".into());
        }
        r.next_id += 1;
        let task = Task {
            kind: TaskKind::Recovery,
            id: r.next_id,
            request: ScanRequest {
                request_id: format!("recovery-{}", r.next_id),
                gpu_id: gpu,
                mode: Mode::Standard,
            },
            origin: Origin::Manual,
            state: State::Queued,
            error: None,
            cancel_reason: None,
            exit_code: None,
        };
        r.tasks.insert(task.id, task.clone());
        if let Err(e) = self.save(&r) {
            r.tasks.remove(&task.id);
            r.stopping = true;
            return Err(e);
        }
        Ok(task)
    }
    pub fn fail_worker(&self, id: u64, error: String) {
        let mut r = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        r.stopping = true;
        if let Some(t) = r.tasks.get_mut(&id) {
            t.state = State::Failed;
            t.error = Some(error);
            let gpu = t.request.gpu_id;
            r.recovery_required.insert(gpu);
            r.manual_hold.insert(gpu);
        }
        let _ = self.save(&r);
    }
    pub fn directory(&self, id: u64) -> PathBuf {
        self.root.join(id.to_string())
    }
}
