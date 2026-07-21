//! autoscan_config.rs
//!
//! 统一的命令配置结构体。
//!
//! 在命令入口处一次性从 `clap::ArgMatches` 解析常用参数，避免扫描流程中
//! 反复读取字符串 key。Autoscan 参数按 common / stressor / mode 拆分，避免
//! legacy 路径携带 gpuboostv3 专属字段。

use super::platform::{
    default_minload_exe_path, default_test_exe_path, default_vfp_csv_path,
    default_vfp_init_csv_path, default_vfp_log_path, default_vfp_temp_csv_path,
};
use clap::ArgMatches;
use nvoc_core::ClockDomain;
use nvoc_core::Error;

// ---------------------------------------------------------------------------
// VFP export / fix_result 配置
// ---------------------------------------------------------------------------

/// handle_vfp_export 所需参数
#[derive(Debug, Clone)]
pub struct VfpExportConfig {
    /// 输出文件路径（"-" 表示 stdout）
    pub output: String,
    /// 是否执行动态 load 测量（--quick 取反）
    pub dynamic: bool,
    /// 是否跳过动态结果校验（--nocheck）
    pub dynamic_check: bool,
    /// 目标 VFP domain；默认 Graphics
    pub domain: ClockDomain,
}

fn vfp_domain_from_matches(matches: &ArgMatches) -> ClockDomain {
    if matches.get_flag("memory") {
        ClockDomain::Memory
    } else if matches.get_flag("processor") {
        ClockDomain::Processor
    } else if matches.get_flag("video") {
        ClockDomain::Video
    } else if matches.get_flag("undefined") {
        ClockDomain::Undefined
    } else {
        ClockDomain::Graphics
    }
}

impl VfpExportConfig {
    pub fn from_matches(matches: &ArgMatches) -> Self {
        VfpExportConfig {
            output: matches
                .get_one::<String>("output")
                .cloned()
                .unwrap_or_else(|| "-".to_string()),
            dynamic: !matches.get_flag("quick"),
            dynamic_check: !matches.get_flag("nocheck"),
            domain: vfp_domain_from_matches(matches),
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
        let minus_bin = matches.get_one::<i32>("minus_bin").copied().unwrap_or(1);
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

#[derive(Debug, Clone)]
pub struct StressorConfig {
    // Keep stressor process selection separate from autoscan mode settings so
    // legacy and GPU Boost scans share CUDA/OpenCL wrapper behavior.
    /// CUDA device ordinal for CUDA_VISIBLE_DEVICES; None = let the stressor pick.
    pub cuda_device: Option<u32>,
    /// Extra arguments appended verbatim to each stressor invocation
    /// (e.g. ["--platform-index", "0", "--device-index", "1"] for OpenCL GPU selection).
    pub extra_args: Vec<String>,
    /// Embedded bundled profile name; "auto" resolves from target VRAM.
    pub profile: String,
    /// Optional custom stressor TOML file.
    pub config: Option<String>,
}

impl StressorConfig {
    fn from_matches(matches: &ArgMatches) -> Self {
        let cuda_device = matches.get_one::<u32>("cuda_device").copied().or_else(|| {
            // Auto-derive from --gpu when it's a single numeric index so that
            // CUDA_VISIBLE_DEVICES (set with CUDA_DEVICE_ORDER=PCI_BUS_ID) matches
            // the NVAPI/NVML PCI-bus GPU selection without a separate --cuda-device flag.
            let specs: Vec<&String> = matches
                .get_many::<String>("gpu")
                .map(|v| v.collect())
                .unwrap_or_default();
            if specs.len() == 1 {
                specs[0].parse::<u32>().ok().filter(|&n| n < 256)
            } else {
                None
            }
        });

        StressorConfig {
            cuda_device,
            extra_args: matches
                .get_many::<String>("stressor_extra_args")
                .map(|v| v.cloned().collect())
                .unwrap_or_default(),
            profile: matches
                .try_get_one::<String>("stressor_profile")
                .ok()
                .flatten()
                .cloned()
                .unwrap_or_else(|| "auto".to_string()),
            config: matches
                .try_get_one::<String>("stressor_config")
                .ok()
                .flatten()
                .cloned(),
        }
    }
}

// ---------------------------------------------------------------------------
// autoscan 公共配置（gpuboostv3 和 legacy 共用）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AutoscanCommonConfig {
    /// 压力测试可执行文件路径
    pub test_exe: String,
    /// Min-load Vulkan 可执行文件路径（唤醒 Optimus 笔记本上的 GPU）
    pub minload_exe: String,
    /// 扫描日志文件路径
    pub log: String,
    /// 单轮超时循环次数
    #[allow(dead_code)]
    // parsed for CLI compatibility; scan phases currently keep their fixed durations
    pub timeout_loops: u32,
    /// BSOD 恢复策略：true = aggressive，false = traditional
    /// None 表示未指定，由调用方根据 GPU 世代决定默认值
    pub recovery_method: Option<bool>,
    pub stressor: StressorConfig,
}

impl AutoscanCommonConfig {
    fn from_matches(matches: &ArgMatches) -> Self {
        // These flags are accepted by both autoscan subcommands; parse them
        // once so mode-specific configs only carry mode-specific fields.
        AutoscanCommonConfig {
            test_exe: matches
                .try_get_one::<String>("test_exe")
                .ok()
                .flatten()
                .cloned()
                .unwrap_or_else(|| default_test_exe_path().to_string()),
            minload_exe: matches
                .try_get_one::<String>("minload_exe")
                .ok()
                .flatten()
                .cloned()
                .unwrap_or_else(|| default_minload_exe_path().to_string()),
            log: matches
                .get_one::<String>("log")
                .cloned()
                .unwrap_or_else(|| default_vfp_log_path().to_string()),
            timeout_loops: matches
                .get_one::<u32>("timeout_loops")
                .copied()
                .unwrap_or(30),
            recovery_method: matches
                .get_one::<String>("bsod_recovery")
                .map(|v| v.as_str() == "aggressive"),
            stressor: StressorConfig::from_matches(matches),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GpuBoostAutoscanConfig {
    pub common: AutoscanCommonConfig,
    // Fields below only exist on autoscan-vfp. Keeping them out of the legacy
    // config prevents silent reliance on placeholder defaults.
    /// 是否启用 ultrafast（gpuboostv3）
    pub is_ultrafast: bool,
    /// 点序列（ultrafast 模式下的自定义扫描序列，"-" 表示自动）
    #[allow(dead_code)]
    // parsed for CLI compatibility; current scan sequence logic is log/key-point driven
    pub point_seq: String,
    /// autoscan 输出 CSV（每点保存）
    #[allow(dead_code)]
    // export_single_point still reads the existing clap output argument directly
    pub output_csv: String,
    /// 参考初始 VFP CSV 路径
    pub init_csv: String,
    /// 是否扫描显存电压
    pub vmem_scan: bool,
}

impl GpuBoostAutoscanConfig {
    /// 从 autoscan（gpuboostv3）子命令的 ArgMatches 解析
    pub fn from_autoscan_matches(matches: &ArgMatches) -> Result<Self, Error> {
        Ok(GpuBoostAutoscanConfig {
            common: AutoscanCommonConfig::from_matches(matches),
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
        })
    }
}

#[derive(Debug, Clone)]
pub struct LegacyAutoscanConfig {
    pub common: AutoscanCommonConfig,
}

impl LegacyAutoscanConfig {
    /// 从 autoscan_legacy 子命令的 ArgMatches 解析
    pub fn from_legacy_matches(matches: &ArgMatches) -> Result<Self, Error> {
        Ok(LegacyAutoscanConfig {
            common: AutoscanCommonConfig::from_matches(matches),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arg_help;

    fn subcommand_matches(args: &[&str]) -> ArgMatches {
        let matches = arg_help::get_arguments()
            .try_get_matches_from(args)
            .expect("valid test arguments");
        matches.subcommand().expect("subcommand").1.clone()
    }

    #[test]
    fn autoscan_defaults_match_cli_defaults() {
        let matches = subcommand_matches(&["nvoc-auto-optimizer", "autoscan-vfp"]);
        let cfg = GpuBoostAutoscanConfig::from_autoscan_matches(&matches).unwrap();

        assert_eq!(cfg.common.test_exe, default_test_exe_path());
        assert_eq!(cfg.common.minload_exe, default_minload_exe_path());
        assert_eq!(cfg.common.log, default_vfp_log_path());
        assert_eq!(cfg.common.timeout_loops, 30);
        assert_eq!(cfg.common.recovery_method, None);
        assert_eq!(cfg.common.stressor.cuda_device, None);
        assert!(cfg.common.stressor.extra_args.is_empty());
        assert_eq!(cfg.common.stressor.profile, "auto");
        assert_eq!(cfg.common.stressor.config, None);
        assert!(!cfg.is_ultrafast);
        assert_eq!(cfg.point_seq, "-");
        assert_eq!(cfg.output_csv, default_vfp_temp_csv_path());
        assert_eq!(cfg.init_csv, default_vfp_init_csv_path());
        assert!(!cfg.vmem_scan);
    }

    #[test]
    fn legacy_config_only_carries_common_autoscan_fields() {
        let matches = subcommand_matches(&["nvoc-auto-optimizer", "autoscan-vfp-legacy"]);
        let cfg = LegacyAutoscanConfig::from_legacy_matches(&matches).unwrap();

        assert_eq!(cfg.common.test_exe, default_test_exe_path());
        assert_eq!(cfg.common.minload_exe, default_minload_exe_path());
        assert_eq!(cfg.common.log, default_vfp_log_path());
        assert_eq!(cfg.common.timeout_loops, 30);
        assert_eq!(cfg.common.recovery_method, None);
        assert_eq!(cfg.common.stressor.cuda_device, None);
        assert!(cfg.common.stressor.extra_args.is_empty());
        assert_eq!(cfg.common.stressor.profile, "auto");
        assert_eq!(cfg.common.stressor.config, None);
    }

    #[test]
    fn single_numeric_gpu_derives_cuda_device() {
        let matches = subcommand_matches(&["nvoc-auto-optimizer", "autoscan-vfp", "--gpu", "2"]);
        let cfg = GpuBoostAutoscanConfig::from_autoscan_matches(&matches).unwrap();

        assert_eq!(cfg.common.stressor.cuda_device, Some(2));
    }

    #[test]
    fn explicit_cuda_device_overrides_derived_gpu() {
        let matches = subcommand_matches(&[
            "nvoc-auto-optimizer",
            "autoscan-vfp",
            "--gpu",
            "2",
            "--cuda-device",
            "4",
        ]);
        let cfg = GpuBoostAutoscanConfig::from_autoscan_matches(&matches).unwrap();

        assert_eq!(cfg.common.stressor.cuda_device, Some(4));
    }

    #[test]
    fn stressor_extra_args_preserve_hyphenated_values() {
        let matches = subcommand_matches(&[
            "nvoc-auto-optimizer",
            "autoscan-vfp",
            "--stressor-extra-args",
            "--platform-index",
            "0",
            "--device-index",
            "1",
        ]);
        let cfg = GpuBoostAutoscanConfig::from_autoscan_matches(&matches).unwrap();

        assert_eq!(
            cfg.common.stressor.extra_args,
            ["--platform-index", "0", "--device-index", "1"]
        );
    }

    #[test]
    fn export_vfp_log_defaults_to_jsonl_and_initcsv() {
        let matches = subcommand_matches(&["nvoc-auto-optimizer", "export-vfp-log"]);

        assert_eq!(
            matches.get_one::<String>("log").unwrap(),
            default_vfp_log_path()
        );
        assert_eq!(
            matches.get_one::<String>("initcsv").unwrap(),
            default_vfp_init_csv_path()
        );
    }

    #[test]
    fn fix_result_defaults_minus_bin_to_one() {
        let matches = subcommand_matches(&["nvoc-auto-optimizer", "fix-vfp-result"]);
        let cfg = FixResultConfig::from_matches(&matches).unwrap();

        assert_eq!(cfg.minus_bin, 1);
    }

    #[test]
    fn fix_result_accepts_explicit_minus_bin() {
        let matches = subcommand_matches(&["nvoc-auto-optimizer", "fix-vfp-result", "-m", "-2"]);
        let cfg = FixResultConfig::from_matches(&matches).unwrap();

        assert_eq!(cfg.minus_bin, -2);
    }
}
