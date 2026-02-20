// Ping service example.
//
// You can install and uninstall this service using other example programs.
// All commands mentioned below shall be executed in Command Prompt with Administrator privileges.
//
// Service installation: `install_service.exe`
// Service uninstallation: `uninstall_service.exe`
//
// Start the service: `net start ping_service`
// Stop the service: `net stop ping_service`
//
// Ping server sends a text message to local UDP port 1234 once a second.
// You can verify that service works by running netcat, i.e: `ncat -ul 1234`.

// extern crate windows_service;
// extern crate nvml_wrapper;
// extern crate nvml_wrapper_sys;

#[cfg(windows)]
fn main() -> windows_service::Result<()> {
    nvoc_service::run()
}

#[cfg(not(windows))]
fn main() {
    panic!("This program is only intended to run on Windows.");
}

#[cfg(windows)]
mod nvoc_service {
    use std::{
        time::{SystemTime, UNIX_EPOCH},
        fs::OpenOptions,
        io::Write,
        ffi::OsString, sync::mpsc, time::Duration
    };
    use windows_service::{
        define_windows_service,
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher, Result,
    };
    use nvml_wrapper::{Nvml, enum_wrappers::device::{TemperatureThreshold, TemperatureSensor}};

    const SERVICE_NAME: &str = "nvoc_service";
    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

    pub fn run() -> Result<()> {
        // Register generated `ffi_service_main` with the system and start the service, blocking
        // this thread until the service is stopped.
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
    }

    // Generate the windows service boilerplate.
    // The boilerplate contains the low-level service entry function (ffi_service_main) that parses
    // incoming service arguments into Vec<OsString> and passes them to user defined service
    // entry (my_service_main).
    define_windows_service!(ffi_service_main, my_service_main);

    // Service entry function which is called on background thread by the system with service
    // parameters. There is no stdout or stderr at this point so make sure to configure the log
    // output to file if needed.
    pub fn my_service_main(_arguments: Vec<OsString>) {
        if let Err(_e) = run_service() {
            // Handle the error, by logging or something.
        }
    }

    pub fn run_service() -> Result<()> {
        // Create a channel to be able to poll a stop event from the service worker loop.
        let (shutdown_tx, shutdown_rx) = mpsc::channel();

        // Define system service event handler that will be receiving service events.
        let event_handler = move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                // Notifies a service to report its current status information to the service
                // control manager. Always return NoError even if not implemented.
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,

                // Handle stop
                ServiceControl::Stop => {
                    shutdown_tx.send(()).unwrap();
                    ServiceControlHandlerResult::NoError
                }

                // treat the UserEvent as a stop request
                ServiceControl::UserEvent(code) => {
                    if code.to_raw() == 130 {
                        shutdown_tx.send(()).unwrap();
                    }
                    ServiceControlHandlerResult::NoError
                }

                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        // Register system service event handler.
        // The returned status handle should be used to report service status changes to the system.
        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)?;

        // Tell the system that service is running
        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        // Nvml initialization is done here to ensure that the service can be stopped even if Nvml fails to initialize.

        // let default = Nvml::init().err().unwrap_err();
        let nvml = Nvml::init().unwrap();
    
        // 根据环境变量决定输出位置
        let mut file = OpenOptions::new()
        .create(true)   // 如果文件不存在则创建
        .append(true)   // 追加模式，保留原有内容
        .open("nvoc-srv-output.log").unwrap();

        loop {
            let count = nvml.device_count().unwrap_or(0);
            println!("Detected {} GPUs via NVML", count);

            // 遍历 GPU
            for i in 0..count {
                let device = nvml.device_by_index(i).unwrap();
                let name = device.name().unwrap_or("Unknown".to_string()); // GPU 名称
                let uuid = device.uuid().unwrap_or("Non UUID".to_string());                 // GPU UUID
                let temperature = device.temperature(TemperatureSensor::Gpu).unwrap_or(0); // GPU 温度
                let thresholds = [
                    TemperatureThreshold::Shutdown,
                    TemperatureThreshold::Slowdown,
                    TemperatureThreshold::MemoryMax,
                    TemperatureThreshold::GpuMax,
                ];
                let threshold_values: [u32; 4] = thresholds.map(|threshold_type| {
                device.temperature_threshold(threshold_type).unwrap_or(0)});
                

                writeln!(
                    file,
                    "Time {} GPU {}: {} UUID={} Temperature={} Threshold={} {} {} {}",
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                    i, name, uuid, temperature, threshold_values[0], threshold_values[1], threshold_values[2], threshold_values[3]
                ).unwrap();
            }

            // Poll shutdown event.
            match shutdown_rx.recv_timeout(Duration::from_secs(1)) {
                // Break the loop either upon stop or channel disconnect
                Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => break,

                // Continue work if no events were received within the timeout
                Err(mpsc::RecvTimeoutError::Timeout) => (),
            };
        }

        // Tell the system that service has stopped.
        status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        Ok(())
    }
}
