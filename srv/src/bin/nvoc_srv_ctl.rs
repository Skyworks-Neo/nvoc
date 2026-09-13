//! `nvoc-srv-ctl` — Windows SCM management for `nvoc_service`.
//!
//! On Linux there is no SCM: install the unit from `srv/systemd/` instead.

#[cfg(windows)]
mod windows_impl {
    use clap::{Parser, Subcommand};
    use std::ffi::OsString;
    use std::time::Duration;
    use windows_service::{
        service::{
            ServiceAccess, ServiceAction, ServiceActionType, ServiceErrorControl,
            ServiceFailureActions, ServiceFailureResetPeriod, ServiceInfo, ServiceStartType,
            ServiceState, ServiceType,
        },
        service_manager::{ServiceManager, ServiceManagerAccess},
    };

    pub const SERVICE_NAME: &str = "nvoc_service";

    #[derive(Parser)]
    #[command(
        name = "nvoc-srv-ctl",
        about = "Manage the nvoc_service Windows service (install/uninstall/status/failure actions)"
    )]
    pub struct Cli {
        #[command(subcommand)]
        pub command: Command,
    }

    #[derive(Subcommand)]
    pub enum Command {
        /// Install nvoc_service (manual start, LocalSystem, sibling binary).
        Install,
        /// Uninstall nvoc_service (stops it first if running).
        Uninstall,
        /// Print the current service state.
        Status,
        /// Configure failure actions: restart after 5 s / 30 s / 60 s. This is
        /// the last-resort failsafe when the process aborts while fans are
        /// software-controlled.
        FailureActions,
    }

    fn manager(access: ServiceManagerAccess) -> windows_service::Result<ServiceManager> {
        ServiceManager::local_computer(None::<&str>, access)
    }

    /// Restart ladder 5 s / 30 s / 60 s with non-crash failures enabled —
    /// the process-level safety net for a fan controller that dies (stale
    /// NVML segfault) while the fan is software-pinned.
    fn configure_failure_actions(
        service: &windows_service::service::Service,
    ) -> windows_service::Result<()> {
        let actions = vec![
            ServiceAction {
                action_type: ServiceActionType::Restart,
                delay: Duration::from_secs(5),
            },
            ServiceAction {
                action_type: ServiceActionType::Restart,
                delay: Duration::from_secs(30),
            },
            ServiceAction {
                action_type: ServiceActionType::Restart,
                delay: Duration::from_secs(60),
            },
        ];
        service.update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86_400)),
            reboot_msg: None,
            command: None,
            actions: Some(actions),
        })?;
        service.set_failure_actions_on_non_crash_failures(true)
    }

    pub fn install() -> windows_service::Result<()> {
        let manager_access = ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE;
        let service_manager = manager(manager_access)?;
        let service_binary_path = std::env::current_exe()
            .expect("current exe")
            .with_file_name("nvoc_service.exe");

        let service_info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from("nvoc service"),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::OnDemand,
            error_control: ServiceErrorControl::Normal,
            executable_path: service_binary_path,
            launch_arguments: vec![],
            dependencies: vec![],
            account_name: None, // run as LocalSystem
            account_password: None,
        };
        println!("Installing service...");
        let service = service_manager.create_service(
            &service_info,
            ServiceAccess::START | ServiceAccess::QUERY_STATUS | ServiceAccess::CHANGE_CONFIG,
        )?;
        service.set_description(
            "NVOC closed-loop control service: fan PID thermal control via NVAPI/NVML",
        )?;
        // Safety default: a dead controller must not leave the fan pinned
        // with nobody at the wheel — restart it.
        configure_failure_actions(&service)?;
        println!(
            "Service installed successfully (failure actions: restart 5/30/60 s). \
             Start it with: net start {SERVICE_NAME}"
        );
        Ok(())
    }

    pub fn uninstall() -> windows_service::Result<()> {
        let service_manager = manager(ServiceManagerAccess::CONNECT)?;
        let service_access =
            ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE;
        let service = service_manager.open_service(SERVICE_NAME, service_access)?;

        // Marked for deletion on success; the database entry disappears once
        // the service is stopped and all handles are closed.
        service.delete()?;
        if service.query_status()?.current_state != ServiceState::Stopped {
            service.stop()?;
        }
        drop(service);

        // Win32 gives no wait-for-deletion; poll for it.
        let start = std::time::Instant::now();
        let timeout = Duration::from_secs(5);
        while start.elapsed() < timeout {
            match service_manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
                Err(windows_service::Error::Winapi(e))
                    if e.raw_os_error()
                        == Some(
                            windows_sys::Win32::Foundation::ERROR_SERVICE_DOES_NOT_EXIST as i32,
                        ) =>
                {
                    println!("{SERVICE_NAME} is deleted.");
                    return Ok(());
                }
                _ => {}
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        println!("{SERVICE_NAME} is marked for deletion (fully removed on restart).");
        Ok(())
    }

    pub fn status() -> windows_service::Result<()> {
        let service_manager = manager(ServiceManagerAccess::CONNECT)?;
        let service = service_manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)?;
        let state = service.query_status()?.current_state;
        println!("{SERVICE_NAME}: {state:?}");
        Ok(())
    }

    pub fn failure_actions() -> windows_service::Result<()> {
        let service_manager = manager(ServiceManagerAccess::CONNECT)?;
        let service = service_manager.open_service(
            SERVICE_NAME,
            ServiceAccess::QUERY_STATUS | ServiceAccess::CHANGE_CONFIG,
        )?;
        configure_failure_actions(&service)?;
        println!(
            "Failure actions configured: restart 5 s / 30 s / 60 s, reset after 24 h, \
             enabled on non-crash failures."
        );
        Ok(())
    }
}

fn main() {
    #[cfg(windows)]
    {
        use clap::Parser as _;
        let cli = windows_impl::Cli::parse();
        let result = match cli.command {
            windows_impl::Command::Install => windows_impl::install(),
            windows_impl::Command::Uninstall => windows_impl::uninstall(),
            windows_impl::Command::Status => windows_impl::status(),
            windows_impl::Command::FailureActions => windows_impl::failure_actions(),
        };
        if let Err(e) = result {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }

    #[cfg(not(windows))]
    {
        eprintln!(
            "nvoc-srv-ctl manages the Windows SCM only. On Linux, install the unit from \
             srv/systemd/nvoc-srv.service instead."
        );
        std::process::exit(1);
    }
}
