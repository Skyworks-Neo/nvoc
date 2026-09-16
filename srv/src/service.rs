//! Windows SCM integration (`#[cfg(windows)]`): service dispatcher, control
//! handler, and the service-mode wiring around the shared control loop.

use log::{error, info};
use std::ffi::OsString;
use std::time::Duration;
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

pub const SERVICE_NAME: &str = "nvoc_service";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

/// Block on the SCM dispatcher until the service stops.
pub fn dispatch() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

/// Whether the current process token is elevated (admin, UAC-expanded).
pub fn process_is_elevated() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TokenElevation};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), 0x0008 /*TOKEN_QUERY*/, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut ret: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) {
    // No console is attached in service context: file sink + stdio redirect.
    let log_path = match crate::logging::init(false) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("logging init failed: {e}");
            return;
        }
    };
    crate::logging::redirect_stdio_to(&log_path);

    let config = match crate::config::load_file(&crate::config::default_config_path()) {
        Ok(c) => c,
        Err(e) => {
            error!("config load failed ({e}); running on built-in defaults");
            crate::config::RuntimeConfig::default()
        }
    };

    let handles = crate::runtime::setup(config);

    let shutdown_tx = handles.shutdown_tx.clone();
    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop => {
                let _ = shutdown_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            // UserEvent 130 remains a stop request (legacy service contract).
            ServiceControl::UserEvent(code) => {
                if code.to_raw() == 130 {
                    let _ = shutdown_tx.send(());
                }
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };
    let status_handle = match service_control_handler::register(SERVICE_NAME, event_handler) {
        Ok(h) => h,
        Err(e) => {
            error!("SCM handler registration failed: {e}");
            return;
        }
    };

    let report = |state: ServiceState, accept: ServiceControlAccept| {
        let _ = status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });
    };
    report(ServiceState::Running, ServiceControlAccept::STOP);

    let state = std::sync::Arc::new(crate::http::ServerState {
        config: handles.config.clone(),
        status: handles.status.clone(),
        backend: handles.backend.clone(),
        history: handles.history.clone(),
        cmd_tx: handles.cmd_tx.clone(),
    });
    crate::http::spawn_supervised(state);

    if let Err(e) = crate::runtime::run_control_loop(handles) {
        // Discovery failure etc. — the service stops (and SCM failure
        // actions may restart it); the fans were never touched.
        error!("control loop failed: {e}");
    }

    report(ServiceState::Stopped, ServiceControlAccept::empty());
    info!("nvoc_service stopped");
}
