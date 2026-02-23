use std::sync::{Arc, Mutex};
use tiny_http::{Server, Response};
use serde::{Deserialize, Serialize};
use log::{info};
use tokio;

#[derive(Serialize, Deserialize, Clone, Debug)]  // 添加 Serialize
pub struct NVOCServiceConfig {
    pub vfp_lock_point: usize,
    // 其他参数...
}

pub struct NVOCServiceCmd {
    pub gpu_index: usize,
    pub cmd: String,
    pub over_freq: i32,
}

pub fn start_http_server(config: Arc<Mutex<NVOCServiceConfig>>, cmd_tx: tokio::sync::mpsc::Sender<NVOCServiceCmd>) {
    let server = Server::http("127.0.0.1:1145").unwrap();
    info!("HTTP config server listening on 127.0.0.1:1145");
    
    for request in server.incoming_requests() {
        let full_url = request.url();
        let path = full_url.split('?').next().unwrap_or(full_url);
        
        match path {
            "/config" => {
                // 获取当前配置
                let cfg = config.lock().unwrap();
                let json = serde_json::to_string(&*cfg).unwrap();
                let response = Response::from_string(json).with_status_code(200);
                request.respond(response).unwrap();
            }
            "/set_tem_wall_vfp" => {
                // 调试：打印完整 URL                
                if let Some(query) = request.url().split('?').nth(1) {                    
                    if query.starts_with("point=") {
                        let point_str = &query[6..];                        
                        match point_str.parse::<usize>() {
                            Ok(point) => {
                                let mut cfg = config.lock().unwrap();
                                cfg.vfp_lock_point = point;
                                info!("VFP lock point updated to {}", point);
                                
                                let response = Response::from_string("OK").with_status_code(200);
                                request.respond(response).unwrap();
                                continue;
                            }
                            Err(_e) => {
                            }
                        }
                    } 
                } 
                
                let response = Response::from_string("Bad request").with_status_code(400);
                request.respond(response).unwrap();
            }
            "/oc_global" => {
                if let Some(query) = request.url().split('?').nth(1) {                    
                    if query.starts_with("oc=") {
                        let freq = &query[3..];                        
                        match freq.parse::<i32>() {
                            Ok(freq_val) => {
                                let _ =cmd_tx.send(NVOCServiceCmd {
                                    gpu_index: 0, // 这里可以根据需要调整 GPU 索引
                                    cmd: "set_oc_global".to_string(),
                                    over_freq: freq_val,
                                });
                                info!("VFP over freq is {}", freq_val);
                                
                                let response = Response::from_string("OK").with_status_code(200);
                                request.respond(response).unwrap();
                                continue;
                            }
                            Err(_e) => {
                            }
                        }
                    }
                } 
                
                let response = Response::from_string("Bad request").with_status_code(400);
                request.respond(response).unwrap();
            }
            _ => {
                let response = Response::from_string("Not found").with_status_code(404);
                request.respond(response).unwrap();
            }
        }
    }
}