// You can install and uninstall this service using other example programs.
// All commands mentioned below shall be executed in Command Prompt with Administrator privileges.
//
// Service installation: `install_service.exe`
// Service uninstallation: `uninstall_service.exe`
//
// extern crate windows_service;
// extern crate nvml_wrapper;
// extern crate nvml_wrapper_sys;

#[path = "../websrv.rs"]
mod websrv;

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
        fs::{OpenOptions},
        // io::Write,
        ffi::OsString, time::Duration,
        env,
        sync::{Arc, Mutex},
        thread,
        cmp::{min, max},
        time::Instant,
    };
    use futures_util::StreamExt;
    use windows_service::{
        define_windows_service,
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher, Result,
    };
    use nvapi_hi::Gpu;
    use nvml_wrapper::{Nvml, enum_wrappers::device::{TemperatureThreshold, TemperatureSensor}};
    use nvoc_auto_optimizer::{handle_lock_vfp, handle_unlock_vfp, handle_global_oc_offset_subcommand, find_matching_vfp_point};
    use clap;
    use log::{info, error, LevelFilter};
    use gag::Redirect;
    use nvapi_hi::nvapi::ClockFrequencyType;

    const SERVICE_NAME: &str = "nvoc_service";
    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

    pub fn run() -> Result<()> {
        // Register generated `ffi_service_main` with the system and start the service, blocking
        // this thread until the service is stopped.
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
    }

    // Generate the Windows service boilerplate.
    // The boilerplate contains the low-level service entry function (ffi_service_main) that parses
    // incoming service arguments into Vec<OsString> and passes them to user defined service
    // entry (my_service_main).
    define_windows_service!(ffi_service_main, my_service_main);

    // Service entry function which is called on background thread by the system with service
    // parameters. There is no stdout or stderr at this point so make sure to configure the log
    // output to file if needed.
    pub fn my_service_main(_arguments: Vec<OsString>) {
        let exe_dir = env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let log_dir = exe_dir.parent()  // 第一级
            .and_then(|p| p.parent())    // 第二级
            .unwrap_or(&exe_dir)         // 如果失败就用 exe_dir
            .join("logs");                // 加上 logs 目录
        std::fs::create_dir_all(&log_dir).unwrap();

        let log_path = log_dir.join(format!("canbedel-{}-output.log", SERVICE_NAME));
        let log_path_for_log2 = log_path.clone();
        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .expect("Failed to open log file");
        
        // 重定向 stdout 和 stderr 到同一个文件
        let _stdout_redirect = Redirect::stdout(log_file.try_clone().unwrap())
            .expect("Failed to redirect stdout");
        let _stderr_redirect = Redirect::stderr(log_file)
            .expect("Failed to redirect stderr");

        // 2. 初始化 log2（提供轮转和日志级别）
        let _logger = log2::open(log_path_for_log2.to_str().unwrap())
            .size(100 * 1024 * 1024)  // 100MB
            .rotate(2)                 // 保留1个备份
            // .tee(true)                  // 同时输出到终端
            .level(LevelFilter::Info)  
            .start();

        let config = Arc::new(Mutex::new(crate::websrv::NVOCServiceConfig {
            vfp_lock_point: 70,
            temp_limit: 60,
        }));
        let http_config = config.clone();
        let (cmd_tx, cmd_rx) = flume::unbounded();
        let http_tx = cmd_tx.clone();

        // Create a channel to be able to poll a stop event from the service worker loop.
        let (shutdown_tx, shutdown_rx) = flume::unbounded();

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
        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler).unwrap();

        // Tell the system that service is running
        let _ = status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });

        thread::spawn(move || {
            crate::websrv::start_http_server(http_config, http_tx);
        });

        let _ = compio::runtime::RuntimeBuilder::new()
        .build().unwrap()
        .block_on(run_service(config, shutdown_rx, cmd_rx));

        // Tell the system that service has stopped.
        let _ = status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });
    }

    async fn run_service(config: Arc<Mutex<crate::websrv::NVOCServiceConfig>>, shutdown_rx: flume::Receiver<()>, cmd_rx: flume::Receiver<crate::websrv::NVOCServiceCmd>) -> Result<()> {
        
        let mut stopc = shutdown_rx.into_stream().skip(1);
        let mut cmdc = cmd_rx.into_stream();

        // Nvml initialization is done here to ensure that the service can be stopped even if Nvml fails to initialize.
        let nvml = Nvml::init().unwrap();
        // let temperature_softwall_offset = 25;
        let gpus = Gpu::enumerate().unwrap();
        let vfp_lowest_lock_point = 40;
        let vfp_highest_lock_point = 100;
        // 每张 GPU 的动态温控锁定点，初始为最高（即未限制）
        let mut gpu_dynamic_lock_point: Vec<usize> = vec![vfp_highest_lock_point; gpus.len()];

        let start_interval: Mutex<Option<humantime::Duration>> = Mutex::new(Some(humantime::Duration::from(Duration::from_secs(5))));
        let interval: Option<humantime::Duration> = start_interval.lock().unwrap().as_ref().cloned();
        let timer = create_timer(interval).fuse();
        let mut timer = std::pin::pin!(timer);

        info!("NVOC Service Start!!");

        loop {
            futures_util::select!{
                _ = stopc.next() => {
                    break;
                }

                cmd = cmdc.next() => {
                    if let Some(cmd) = cmd {
                        info!("Received command: {}", cmd.cmd);
                        match cmd.cmd.as_str() {
                            "set_oc_global" => {
                                // 处理设置全局超频频率的命令
                                let i = cmd.gpu_index;
                                let freq_val = cmd.over_freq;
                                let freq_str = freq_val.to_string();
                                let freq_str_static: &'static str = Box::leak(freq_str.into_boxed_str());
                                let pseudo_matches = clap::Command::new("")
                                    .arg(clap::Arg::new("delta").default_value(freq_str_static))
                                    .arg(clap::Arg::new("pstate").default_value("P0"))
                                    .arg(clap::Arg::new("clock").default_value("graphics"))
                                    .get_matches_from(vec![""]);

                                let mut gpu_result = Vec::new();
                                if let Some(g) = gpus.get(i) {
                                    gpu_result.push(g);
                                }

                                match handle_global_oc_offset_subcommand(&gpu_result, &pseudo_matches) {
                                    Ok(_) => info!("OC set to {} for GPU {}", freq_str_static, i),
                                    Err(e) => error!("Failed to set OC for GPU {}: {:?}", i, e),
                                }
                                // 这里可以调用相应的函数来设置全局超频频率
                                // 例如：set_oc_global_frequency(cmd.over_freq);
                            }

                            _ => {
                                // 处理其他命令
                            }
                        }
                    }
                }
                
                _ = timer.next() => {
                    let cfg = config.lock().unwrap();
                    let vfp_low_lock_point = min(max(cfg.vfp_lock_point, vfp_lowest_lock_point), vfp_highest_lock_point);
                    let temperature_softwall = cfg.temp_limit;
                    let count = nvml.device_count().unwrap_or(0);
                    info!("Detected {} GPUs via NVML", count);

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
                        

                        info!(
                            "Time {} GPU {}: {} UUID={} Temperature={} Threshold={} {} {} {}",
                            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                            i, name, uuid, temperature, threshold_values[0], threshold_values[1], threshold_values[2], threshold_values[3]
                        );

                        let mut gpu_result = Vec::new();
                        if let Some(g) = gpus.get(i as usize) {
                            gpu_result.push(g);
                            // 读取传感器电压/频率，并反推当前工作点写入 gpu_dynamic_lock_point
                            let sensor_v = g.inner().core_voltage()
                                .map_err(|e| error!("GPU {} core_voltage: {:?}", i, e)).ok();
                            let sensor_f = g.inner().clock_frequencies(ClockFrequencyType::Current)
                                .map_err(|e| error!("GPU {} clock_frequencies: {:?}", i, e)).ok();
                            if let (Some(sensor_v), Some(sensor_f)) = (sensor_v, sensor_f) {
                                info!("GPU {}: voltage={}, freq={:?}", i, sensor_v, sensor_f);
                                match g.status().map(|s| s.vfp) {
                                    Ok(Some(vfp)) => match find_matching_vfp_point(&vfp.graphics, sensor_v) {
                                        Some((idx, pt)) => {
                                            info!(
                                                "GPU {} Working VfpPoint Inferred: Index={}, Voltage={:?}, Frequency={:?}",
                                                i, idx, pt.voltage, pt.frequency
                                            );
                                            // 仅在未处于降频保护时更新动态点，避免覆盖温控收紧的值
                                            if gpu_dynamic_lock_point[i as usize] >= vfp_highest_lock_point {
                                                gpu_dynamic_lock_point[i as usize] = *idx;
                                            }
                                        },
                                        None => info!("GPU {}: no matching VfpPoint found", i),
                                    },
                                    Ok(None)   => info!("GPU {}: VFP unsupported", i),
                                    Err(e)     => error!("GPU {} status: {:?}", i, e),
                                }
                            }
                        }
                        let pseudo_matches = clap::ArgMatches::default();

                        if temperature >= temperature_softwall {
                            // 超温：每周期降低一个工作点（收紧），不低于最低限制
                            let current = gpu_dynamic_lock_point[i as usize];
                            let next = current.saturating_sub(1).max(vfp_lowest_lock_point);
                            gpu_dynamic_lock_point[i as usize] = next;
                            match handle_lock_vfp(&gpu_result, &pseudo_matches, next, true) {
                                Ok(_) => info!("GPU {}: over-temp, stepped down to VFP lock point {}", i, next),
                                Err(e) => error!("GPU {}: failed to lock VFP: {:?}", i, e),
                            }
                        } else {
                            // 温度正常：每周期放开一个工作点（松弛），不超过用户配置上限
                            let current = gpu_dynamic_lock_point[i as usize];
                            if current < vfp_low_lock_point {
                                let next = (current + 1).min(vfp_low_lock_point);
                                gpu_dynamic_lock_point[i as usize] = next;
                                match handle_lock_vfp(&gpu_result, &pseudo_matches, next, true) {
                                    Ok(_) => info!("GPU {}: temp normal, relaxed to VFP lock point {}", i, next),
                                    Err(e) => error!("GPU {}: failed to relax VFP: {:?}", i, e),
                                }
                            } else {
                                // 已回到正常上限，完全解锁
                                gpu_dynamic_lock_point[i as usize] = vfp_highest_lock_point;
                                match handle_unlock_vfp(&gpu_result) {
                                    Ok(_) => info!("GPU {}: temp normal, VFP fully unlocked", i),
                                    Err(e) => error!("GPU {}: failed to unlock VFP: {:?}", i, e),
                                }
                            }
                        }
                    } // end for i in 0..count
                    drop(cfg); // 释放锁
                }
            }

        }
        Ok(())
    }

    pub fn create_timer(interval: Option<humantime::Duration>) -> impl futures_util::Stream<Item = Instant> {
    if let Some(d) = interval {
        futures_util::future::Either::Left(async_stream::stream! {
            let mut interval = compio::time::interval(*d);
            loop {
                yield interval.tick().await;
            }
        })
    } else {
        futures_util::future::Either::Right(futures_util::stream::pending())
    }
}

}
