//! Windows Job Object owns the optimizer and every descendant, even on service crash.
use crate::scans::{Manager, State, Task, TaskKind};
use std::{
    fs::{self, File},
    io,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{JobObjects::*, Threading::*},
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
pub struct ProcessTree {
    job: Handle,
    process: Handle,
}
fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}
/// `CreateProcessW` accepts a verbatim `\\?\` current directory, but child
/// processes created with one hang before running any code. Strip the verbatim
/// prefix so callers may pass canonicalized paths safely.
fn plain_cwd(path: &Path) -> PathBuf {
    let text: Vec<u16> = path.as_os_str().encode_wide().collect();
    if text.starts_with(&[
        u16::from(b'\\'),
        u16::from(b'\\'),
        u16::from(b'?'),
        u16::from(b'\\'),
    ]) {
        if text.len() > 8
            && text[4..8]
                == [
                    u16::from(b'U'),
                    u16::from(b'N'),
                    u16::from(b'C'),
                    u16::from(b'\\'),
                ]
        {
            let mut plain = vec![u16::from(b'\\'), u16::from(b'\\')];
            plain.extend_from_slice(&text[8..]);
            return PathBuf::from(std::ffi::OsString::from_wide(&plain));
        }
        return PathBuf::from(std::ffi::OsString::from_wide(&text[4..]));
    }
    path.to_path_buf()
}
fn os_error() -> String {
    io::Error::last_os_error().to_string()
}
impl ProcessTree {
    pub fn spawn(exe: &Path, args: &[String], cwd: &Path) -> Result<Self, String> {
        fs::create_dir_all(cwd).map_err(|e| e.to_string())?;
        let output = File::create(cwd.join("output.log")).map_err(|e| e.to_string())?;
        let input = File::open("NUL").map_err(|e| e.to_string())?;
        // The service passes only generated, whitespace-free arguments, never raw
        // client argv. The executable path is supplied separately as application.
        if args.iter().any(|a| a.contains([' ', '\t', '"', '\0'])) {
            return Err("invalid internal argument".into());
        }
        let application = wide(exe.as_os_str());
        let mut command = wide(std::ffi::OsStr::new(&format!(
            "\"{}\" {}",
            exe.display(),
            args.join(" ")
        )));
        let directory = wide(plain_cwd(cwd).as_os_str());
        // SAFETY: all Win32 input/output structures and buffers remain live for
        // their calls. Child starts suspended and cannot touch a GPU before job assignment.
        unsafe {
            let job = Handle(CreateJobObjectW(std::ptr::null(), std::ptr::null()));
            if job.0.is_null() {
                return Err(os_error());
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err(os_error());
            }
            let out = output.as_raw_handle() as HANDLE;
            let stdin = input.as_raw_handle() as HANDLE;
            if SetHandleInformation(out, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0
                || SetHandleInformation(stdin, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0
            {
                return Err(os_error());
            }
            // Inherit only the two redirected stdio handles, never service or
            // unrelated client handles. Attribute storage must be pointer-aligned.
            let mut size = 0;
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
            let mut storage = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
            let attributes = storage.as_mut_ptr().cast();
            if InitializeProcThreadAttributeList(attributes, 1, 0, &mut size) == 0 {
                return Err(os_error());
            }
            struct Attributes(LPPROC_THREAD_ATTRIBUTE_LIST);
            impl Drop for Attributes {
                fn drop(&mut self) {
                    unsafe {
                        DeleteProcThreadAttributeList(self.0);
                    }
                }
            }
            let attributes = Attributes(attributes);
            let inherited = [out, stdin];
            if UpdateProcThreadAttribute(
                attributes.0,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                inherited.as_ptr().cast(),
                std::mem::size_of_val(&inherited),
                std::ptr::null_mut(),
                std::ptr::null(),
            ) == 0
            {
                return Err(os_error());
            }
            let mut startup: STARTUPINFOEXW = std::mem::zeroed();
            startup.StartupInfo.cb = std::mem::size_of_val(&startup) as u32;
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            startup.StartupInfo.hStdInput = stdin;
            startup.StartupInfo.hStdOutput = out;
            startup.StartupInfo.hStdError = out;
            startup.lpAttributeList = attributes.0;
            let mut info: PROCESS_INFORMATION = std::mem::zeroed();
            if CreateProcessW(
                application.as_ptr(),
                command.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
                CREATE_SUSPENDED | CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                std::ptr::null(),
                directory.as_ptr(),
                &startup.StartupInfo,
                &mut info,
            ) == 0
            {
                return Err(os_error());
            }
            let process = Handle(info.hProcess);
            let main_thread = Handle(info.hThread);
            if AssignProcessToJobObject(job.0, process.0) == 0 {
                let error = os_error();
                TerminateProcess(process.0, 1);
                WaitForSingleObject(process.0, 5000);
                return Err(error);
            }
            if ResumeThread(main_thread.0) == u32::MAX {
                let error = os_error();
                TerminateJobObject(job.0, 1);
                WaitForSingleObject(process.0, 5000);
                return Err(error);
            }
            Ok(Self { job, process })
        }
    }
    pub fn poll(&self) -> Result<Option<i32>, String> {
        unsafe {
            match WaitForSingleObject(self.process.0, 0) {
                WAIT_TIMEOUT => Ok(None),
                WAIT_OBJECT_0 => {
                    let mut code = 0;
                    if GetExitCodeProcess(self.process.0, &mut code) == 0 {
                        Err(os_error())
                    } else {
                        Ok(Some(code as i32))
                    }
                }
                _ => Err(os_error()),
            }
        }
    }
    pub fn stop(&self) -> Result<(), String> {
        unsafe {
            if TerminateJobObject(self.job.0, 1) == 0 {
                return Err(os_error());
            }
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            if unsafe {
                QueryInformationJobObject(
                    self.job.0,
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(os_error());
            }
            // Job accounting can reach zero before the root process handle is
            // signalled. Wait for both, otherwise cancellation returns too early.
            if info.ActiveProcesses == 0 && self.poll()?.is_some() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("optimizer process tree did not stop within 10 seconds".into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

pub fn validate_gpu(id: u32) -> Result<(), String> {
    let inventory =
        nvoc_core::discover_targets(nvoc_core::BackendSet::Both).map_err(|e| e.to_string())?;
    let gpu = inventory
        .target_by_id(nvoc_core::GpuId(id))
        .map_err(|e| e.to_string())?;
    if !gpu.has_nvapi() {
        return Err("GPU does not support NVAPI optimization".into());
    }
    Ok(())
}

/// Cancellation has an explicit default-reset policy, not a claim to restore an
/// arbitrary pre-scan configuration. Try every reset and report every failure.
pub fn recover(id: u32) -> Result<(), String> {
    use nvoc_core::*;
    let inventory = discover_targets(BackendSet::Both).map_err(|e| e.to_string())?;
    let gpu = inventory
        .target_by_id(GpuId(id))
        .map_err(|e| e.to_string())?;
    let mut failures = Vec::new();
    macro_rules! reset {
        ($op:expr) => {
            if let Err(e) = run(&gpu, $op) {
                // These controls do not exist on every GPU generation. An
                // unsupported control could not have been applied by the scan.
                if !matches!(
                    &e,
                    Error::VfpUnsupported
                        | Error::FeatureUnsupportedErr
                        | Error::Nvapi(nvapi_hi::Error::Nvapi(nvapi_hi::NvapiError {
                            status: nvapi_hi::Status::NotSupported
                                | nvapi_hi::Status::NoImplementation,
                            ..
                        }))
                ) {
                    failures.push(format!("{}: {e}", stringify!($op)));
                }
            }
        };
    }
    reset!(ResetPublicVftableOffset {
        domain: VfpResetDomain::All
    });
    reset!(ResetPublicVftableGpcLock);
    for domain in [ClockDomain::Graphics, ClockDomain::Memory] {
        reset!(ResetVfpFrequencyLock { domain });
    }
    match run(&gpu, QueryGpuInfo) {
        Ok(info) => {
            let offsets = info
                .output
                .pstate_limits
                .iter()
                .flat_map(|(&pstate, limits)| {
                    limits
                        .iter()
                        .filter(|(_, l)| l.frequency_delta.is_some())
                        .map(move |(&domain, _)| (pstate, domain))
                })
                .collect();
            reset!(ResetPstateGlobalFreqOffset { offsets });
            if !info.output.power_limits.is_empty() {
                reset!(ResetNvapiPowerLimits);
            }
            if !info.output.sensor_limits.is_empty() {
                reset!(ResetNvapiSensorLimits);
            }
            if !info.output.coolers.is_empty() {
                reset!(ResetCoolerLevels);
            }
            match fetch_gpu_type(&info.output) {
                Ok(kind) if kind.is_legacy_voltage() => {
                    reset!(ResetLegacyGpcRailOvervoltLimit);
                }
                Ok(_) => {
                    reset!(SetVoltageBoost {
                        boost: Percentage(0)
                    });
                }
                Err(e) => failures.push(e.to_string()),
            }
        }
        Err(e) => failures.push(e.to_string()),
    }
    // Match the optimizer's fan cleanup, including NVML fan overrides.
    // Keep NVML's typed NotSupported error: QueryFanInfo currently erases it.
    let fans = (|| {
        let nvml = gpu.nvml().map_err(|e| e.to_string())?;
        for index in 0..nvml.device_count().map_err(|e| e.to_string())? {
            let device = nvml.device_by_index(index).map_err(|e| e.to_string())?;
            if nvoc_core::target::gpu_id_from_nvml_device(&device).map_err(|e| e.to_string())?
                == gpu.id
            {
                return match device.num_fans() {
                    Ok(count) => Ok(count),
                    Err(nvml_wrapper::error::NvmlError::NotSupported) => Ok(0),
                    Err(e) => Err(e.to_string()),
                };
            }
        }
        Err("GPU not found in NVML during fan recovery".to_string())
    })();
    match fans {
        Ok(count) => {
            for fan_index in 0..count {
                reset!(ResetFanSpeed { fan_index });
            }
        }
        Err(e) => failures.push(e.to_string()),
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

pub trait RunningScan {
    fn poll(&self) -> Result<Option<i32>, String>;
    fn stop(&self) -> Result<(), String>;
}
impl RunningScan for ProcessTree {
    fn poll(&self) -> Result<Option<i32>, String> {
        self.poll()
    }
    fn stop(&self) -> Result<(), String> {
        self.stop()
    }
}
pub trait ScanBackend {
    type Process: RunningScan;
    fn validate(&self, gpu: u32) -> Result<(), String>;
    fn spawn(&self, task: &Task, directory: &Path) -> Result<Self::Process, String>;
    fn recover(&self, gpu: u32) -> Result<(), String>;
}
pub struct NativeBackend {
    pub executable: std::path::PathBuf,
}
impl ScanBackend for NativeBackend {
    type Process = ProcessTree;
    fn validate(&self, gpu: u32) -> Result<(), String> {
        validate_gpu(gpu)
    }
    fn spawn(&self, task: &Task, directory: &Path) -> Result<ProcessTree, String> {
        let args = vec![
            format!("--gpu={}", task.request.gpu_id),
            "--no-color".into(),
            "optimize".into(),
            "--yes".into(),
            "--mode".into(),
            task.request.mode.argument().into(),
            "--workspace".into(),
            "scan".into(),
        ];
        ProcessTree::spawn(&self.executable, &args, directory)
    }
    fn recover(&self, gpu: u32) -> Result<(), String> {
        recover(gpu)
    }
}

pub fn worker(manager: Arc<Manager>, executable: std::path::PathBuf) {
    worker_with(manager, NativeBackend { executable });
}
pub fn worker_with(manager: Arc<Manager>, backend: impl ScanBackend) {
    loop {
        let task = {
            let r = manager.registry.lock().unwrap();
            r.tasks.values().find(|t| !t.state.terminal()).cloned()
        };
        if let Some(task) = task {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute(&manager, &backend, &task)
            }))
            .unwrap_or_else(|_| Err("scan worker panicked; explicit recovery required".into()));
            if let Err(e) = result {
                log::error!("scan worker failed: {e}");
                // Fail closed on journal/worker errors; no later task can start.
                manager.fail_worker(task.id, e);
                break;
            }
        } else if manager.registry.lock().unwrap().stopping {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

pub fn execute(manager: &Manager, backend: &impl ScanBackend, task: &Task) -> Result<(), String> {
    let id = task.id;
    if task.kind == TaskKind::Recovery {
        manager.begin_recovery(id)?;
        let result = backend.recover(task.request.gpu_id);
        return manager.finish(id, None, result.as_ref().err().cloned(), result.is_ok());
    }
    // Cancellation before launch performs no driver writes.
    if task.cancel_reason.is_some() {
        return manager.finish(id, None, None, true);
    }
    if let Err(e) = backend.validate(task.request.gpu_id) {
        return manager.finish(id, None, Some(e), true);
    }
    manager.update(id, |t| {
        if t.cancel_reason.is_none() {
            t.state = State::Running;
        }
    })?;
    if manager.get(id)?.cancel_reason.is_some() {
        return manager.finish(id, None, None, true);
    }
    let tree = match backend.spawn(task, &manager.directory(id)) {
        Ok(tree) => tree,
        Err(e) => return manager.finish(id, None, Some(e), true),
    };
    let outcome = loop {
        if manager.get(id)?.cancel_reason.is_some() {
            break Ok(None);
        }
        match tree.poll() {
            Ok(Some(code)) => break Ok(Some(code)),
            Err(e) => break Err(e),
            _ => {}
        }
        thread::sleep(Duration::from_millis(100));
    };
    // Even on success, reap any leftover descendant before releasing admission.
    let stopped = tree.stop();
    let cancelled = manager.begin_recovery(id)?;
    let recovery = if stopped.is_ok() && (cancelled || !matches!(outcome, Ok(Some(0)))) {
        backend.recover(task.request.gpu_id)
    } else {
        Ok(())
    };
    let code = outcome.as_ref().ok().copied().flatten();
    let error = stopped
        .as_ref()
        .err()
        .cloned()
        .or_else(|| recovery.as_ref().err().cloned())
        .or_else(|| outcome.err())
        .or_else(|| {
            code.filter(|c| *c != 0 && !cancelled)
                .map(|c| format!("optimizer exited with code {c}"))
        });
    manager.finish(id, code, error, stopped.is_ok() && recovery.is_ok())
}
