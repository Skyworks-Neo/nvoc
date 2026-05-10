//! autoscan_config.rs
//!
//! 统一的扫描流程配置结构体。
//! 在 autoscan_gpuboostv3 / autoscan_legacy / handle_vfp_export / fix_result
//! 等函数的入口处一次性从 clap::ArgMatches 解析，之后只传 &AutoscanConfig，
//! 消除函数间层层透传 ArgMatches 以及散落的字符串 key 读取。

use crate::error::Error;
use crate::platform::{
    default_test_exe_path, default_vfp_csv_path, default_vfp_init_csv_path, default_vfp_log_path,
    default_vfp_temp_csv_path,
};
use clap::ArgMatches;

// ---------------------------------------------------------------------------
// VFP export / fix_result 配置
// ---------------------------------------------------------------------------

/// handle_vfp_export 所需参数
#[derive(Debug, Clone)]
pub struct VfpExportConfig {
    /// CSV 列分隔符（',' 或 '\t'）
    pub delimiter: u8,
    /// 输出文件路径（"-" 表示 stdout）
    pub output: String,
    /// 是否执行动态 load 测量（--quick 取反）
    pub dynamic: bool,
    /// 是否跳过动态结果校验（--nocheck）
    pub dynamic_check: bool,
    /// 是否导出显存 VF 曲线（--memory）；默认 false 表示导出核心曲线
    pub export_memory: bool,
}

impl VfpExportConfig {
    pub fn from_matches(matches: &ArgMatches) -> Self {
        VfpExportConfig {
            delimiter: if matches.get_flag("tabs") {
                b'\t'
            } else {
                b','
            },
            output: matches
                .get_one::<String>("output")
                .cloned()
                .unwrap_or_else(|| "-".to_string()),
            dynamic: !matches.get_flag("quick"),
            dynamic_check: !matches.get_flag("nocheck"),
            export_memory: matches.get_flag("memory"),
        }
    }
}

// ---------------------------------------------------------------------------
// fix_result 配置
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FixResultConfig {
    pub is_ultrafast: bool,
    /// 临时 CSV 路径（autoscan 写出的带 margin 列文件）
    pub vfpath: String,
    /// 最终输出 CSV 路径
    pub output: String,
    /// 全局偏移 bin 数（整数，可为负）
    pub minus_bin: i32,
}

impl FixResultConfig {
    pub fn from_matches(matches: &ArgMatches) -> Result<Self, Error> {
        let minus_bin = *matches
            .get_one::<i32>("minus_bin")
            .ok_or_else(|| Error::from("missing --minus_bin argument"))?;
        Ok(FixResultConfig {
            is_ultrafast: matches.get_flag("ultrafast"),
            vfpath: matches
                .get_one::<String>("tempcsv")
                .cloned()
                .unwrap_or_else(|| default_vfp_temp_csv_path().to_string()),
            output: matches
                .get_one::<String>("outputcsv")
                .cloned()
                .unwrap_or_else(|| default_vfp_csv_path().to_string()),
            minus_bin,
        })
    }
}

// ---------------------------------------------------------------------------
// autoscan 公共配置（gpuboostv3 和 legacy 共用）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
#[allow(dead_code)] // 部分字段为 oc_scanner 后续完整迁移预留，暂时未全部读取
pub struct AutoscanConfig {
    /// 压力测试可执行文件路径
    pub test_exe: String,
    /// 扫描日志文件路径
    pub log: String,
    /// 单轮超时循环次数
    pub timeout_loops: u32,
    /// BSOD 恢复策略：true = aggressive，false = traditional
    /// None 表示未指定，由调用方根据 GPU 世代决定默认值
    pub recovery_method: Option<bool>,
    // ---- gpuboostv3 专属 ----
    /// 是否启用 ultrafast（gpuboostv3）
    pub is_ultrafast: bool,
    /// 点序列（ultrafast 模式下的自定义扫描序列，"-" 表示自动）
    pub point_seq: String,
    /// autoscan 输出 CSV（每点保存）
    pub output_csv: String,
    /// 参考初始 VFP CSV 路径
    pub init_csv: String,
    /// 是否扫描显存电压
    pub vmem_scan: bool,
    /// CUDA device ordinal for CUDA_VISIBLE_DEVICES; None = let the stressor pick.
    pub cuda_device: Option<u32>,
    /// Extra arguments appended verbatim to each stressor invocation
    /// (e.g. ["--platform-index", "0", "--device-index", "1"] for OpenCL GPU selection).
    pub stressor_extra_args: Vec<String>,
}

impl AutoscanConfig {
    /// 从 autoscan（gpuboostv3）子命令的 ArgMatches 解析
    pub fn from_autoscan_matches(matches: &ArgMatches) -> Result<Self, Error> {
        let timeout_loops = matches
            .get_one::<u32>("timeout_loops")
            .copied()
            .unwrap_or(30);

        let recovery_method = matches
            .get_one::<String>("bsod_recovery")
            .map(|v| v.as_str() == "aggressive");

        Ok(AutoscanConfig {
            test_exe: matches
                .get_one::<String>("test_exe")
                .cloned()
                .unwrap_or_else(|| default_test_exe_path().to_string()),
            log: matches
                .get_one::<String>("log")
                .cloned()
                .unwrap_or_else(|| default_vfp_log_path().to_string()),
            timeout_loops,
            recovery_method,
            is_ultrafast: matches.get_flag("ultrafast"),
            point_seq: matches
                .get_one::<String>("point_seq")
                .cloned()
                .unwrap_or_else(|| "-".to_string()),
            output_csv: matches
                .get_one::<String>("output")
                .cloned()
                .unwrap_or_else(|| default_vfp_temp_csv_path().to_string()),
            init_csv: matches
                .get_one::<String>("initcsv")
                .cloned()
                .unwrap_or_else(|| default_vfp_init_csv_path().to_string()),
            vmem_scan: matches.get_flag("Vmem_scan_switch"),
            cuda_device: matches.get_one::<u32>("cuda_device").copied(),
            stressor_extra_args: matches
                .get_many::<String>("stressor_extra_args")
                .map(|v| v.cloned().collect())
                .unwrap_or_default(),
        })
    }

    /// 从 autoscan_legacy 子命令的 ArgMatches 解析
    /// legacy 没有 ultrafast / point_seq / output_csv / initcsv / vmem_scan，填默认值
    pub fn from_legacy_matches(matches: &ArgMatches) -> Result<Self, Error> {
        let timeout_loops = matches
            .get_one::<u32>("timeout_loops")
            .copied()
            .unwrap_or(30);

        let recovery_method = matches
            .get_one::<String>("bsod_recovery")
            .map(|v| v.as_str() == "aggressive");

        Ok(AutoscanConfig {
            test_exe: matches
                .get_one::<String>("test_exe")
                .cloned()
                .unwrap_or_else(|| default_test_exe_path().to_string()),
            log: matches
                .get_one::<String>("log")
                .cloned()
                .unwrap_or_else(|| default_vfp_log_path().to_string()),
            timeout_loops,
            recovery_method,
            // legacy 无以下字段，填空/默认
            is_ultrafast: false,
            point_seq: "-".to_string(),
            output_csv: default_vfp_temp_csv_path().to_string(),
            init_csv: default_vfp_init_csv_path().to_string(),
            vmem_scan: false,
            cuda_device: matches.get_one::<u32>("cuda_device").copied(),
            stressor_extra_args: matches
                .get_many::<String>("stressor_extra_args")
                .map(|v| v.cloned().collect())
                .unwrap_or_default(),
        })
    }
}
