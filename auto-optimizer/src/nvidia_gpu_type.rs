//! NVIDIA GPU 世代类型定义及其关联参数。
//!
//! 将散落在 `basic_func.rs` 和 `oc_get_set_function.rs` 中的 GpuType 枚举、
//! OC 扫描参数、电压限制探测参数、电压锁定参数统一管理于此文件。

use nvapi_hi::GpuInfo;
use std::fmt;

use crate::error::Error;

// ─────────────────────────────── GpuType 枚举 ───────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuType {
    Mobile50Series,
    Desktop50Series,
    Mobile40Series,
    Desktop40Series,
    Mobile30Series,
    Desktop30Series,
    Mobile20Series,
    Desktop20Series,
    Mobile16Series,
    Desktop16Series,
    Mobile10Series,
    Desktop10Series,
    Mobile9Series,
    Desktop9Series,
    WorkstationBlackwell,
    WorkstationLovelace,
    WorkstationAmpere,
    WorkstationTuring,
    WorkstationPascal,
    ServerBlackwell,
    ServerHopper,
    ServerAmpere,
    ServerVolta,
    ServerPascal,
    ServerTuringTesla,
    ComputationVolta,
    Unknown,
}

// ─────────────────────────────── Display ─────────────────────────────────────

impl fmt::Display for GpuType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuType::Mobile50Series => write!(f, "50 series mobile detected"),
            GpuType::Desktop50Series => write!(f, "50 series desktop detected"),
            GpuType::Mobile40Series => write!(f, "40 series mobile detected"),
            GpuType::Desktop40Series => write!(f, "40 series desktop detected"),
            GpuType::Mobile30Series => write!(f, "30 series mobile detected"),
            GpuType::Desktop30Series => write!(f, "30 series desktop detected"),
            GpuType::Mobile20Series => write!(f, "20 series mobile detected"),
            GpuType::Desktop20Series => write!(f, "20 series desktop detected"),
            GpuType::Mobile16Series => write!(f, "16 series mobile detected"),
            GpuType::Desktop16Series => write!(f, "16 series desktop detected"),
            GpuType::Mobile10Series => write!(f, "10 series mobile detected"),
            GpuType::Desktop10Series => write!(f, "10 series desktop detected"),
            GpuType::Mobile9Series => write!(f, "9 series mobile detected"),
            GpuType::Desktop9Series => write!(f, "9 series desktop detected"),
            GpuType::ComputationVolta => write!(f, "Volta series computational card detected"),
            GpuType::WorkstationBlackwell => write!(f, "Blackwell series workstation card detected"),
            GpuType::WorkstationLovelace => write!(f, "Lovelace series workstation card detected"),
            GpuType::WorkstationAmpere => write!(f, "Ampere series workstation card detected"),
            GpuType::WorkstationTuring => write!(f, "Turing series workstation card detected"),
            GpuType::WorkstationPascal => write!(f, "Pascal series workstation card detected"),
            GpuType::ServerBlackwell => write!(f, "Blackwell series server card detected"),
            GpuType::ServerHopper => write!(f, "Hopper series server card detected"),
            GpuType::ServerAmpere => write!(f, "Ampere series server card detected"),
            GpuType::ServerVolta => write!(f, "Volta series server card detected"),
            GpuType::ServerPascal => write!(f, "Pascal series server card detected"),
            GpuType::ServerTuringTesla => write!(f, "Turing Tesla series server card (e.g. T4) detected"),
            GpuType::Unknown => write!(f, "Unknown"),
        }
    }
}

// ─────────────────────────── 检测 / 构造 ─────────────────────────────────────

/// 根据 GPU 名称 + codename 字符串判定世代
pub fn detect_gpu_type(gpu_name: &str) -> GpuType {
    let is_rtx_a = gpu_name.contains("RTX A");
    let is_rtx_professional = gpu_name.contains("RTX") && (gpu_name.contains("2000") || gpu_name.contains("3000") || gpu_name.contains("4000") || gpu_name.contains("5000") || gpu_name.contains("6000")) && !gpu_name.contains("GeForce");
    let is_quadro = gpu_name.contains("Quadro");
    let is_tesla = gpu_name.contains("Tesla");
    let is_server = is_tesla || gpu_name.contains("H100") || gpu_name.contains("H800") || gpu_name.contains("A100") || gpu_name.contains("A800") || gpu_name.contains("B100") || gpu_name.contains("B200") || gpu_name.contains("V100") || gpu_name.contains("P100") || gpu_name.contains("L40") || gpu_name.contains("L4");

    if gpu_name.contains("GB") {
        if is_server {
            GpuType::ServerBlackwell
        } else if is_rtx_professional || is_quadro {
            GpuType::WorkstationBlackwell
        } else if gpu_name.contains("Laptop") {
            GpuType::Mobile50Series
        } else {
            GpuType::Desktop50Series
        }
    } else if gpu_name.contains("GH") {
        GpuType::ServerHopper
    } else if gpu_name.contains("AD") {
        if is_server || gpu_name.contains("L40") || gpu_name.contains("L4") {
            GpuType::ServerBlackwell // L40/L4 are Ada but often categorized for servers, using Blackwell slot if Hopper/Ada server not explicit
        } else if is_rtx_professional || is_quadro || is_rtx_a {
            GpuType::WorkstationLovelace
        } else if gpu_name.contains("Laptop") {
            GpuType::Mobile40Series
        } else {
            GpuType::Desktop40Series
        }
    } else if gpu_name.contains("GA") {
        if is_server || gpu_name.contains("A100") {
            GpuType::ServerAmpere
        } else if is_rtx_professional || is_quadro || is_rtx_a {
            GpuType::WorkstationAmpere
        } else if gpu_name.contains("Laptop") {
            GpuType::Mobile30Series
        } else {
            GpuType::Desktop30Series
        }
    } else if gpu_name.contains("TU10") {
        if is_server || gpu_name.contains("T4") {
            GpuType::ServerTuringTesla
        } else if is_rtx_professional || is_quadro {
            GpuType::WorkstationTuring
        } else if gpu_name.contains("Laptop") {
            GpuType::Mobile20Series
        } else {
            GpuType::Desktop20Series
        }
    } else if gpu_name.contains("TU11") {
        if gpu_name.contains("Laptop") {
            GpuType::Mobile16Series
        } else {
            GpuType::Desktop16Series
        }
    } else if gpu_name.contains("GP1") {
        // Do NOT mess up with 'GPU'
        if is_server || gpu_name.contains("P100") {
            GpuType::ServerPascal
        } else if is_quadro {
            GpuType::WorkstationPascal
        } else if gpu_name.contains("Laptop") {
            GpuType::Mobile10Series
        } else {
            GpuType::Desktop10Series
        }
    } else if gpu_name.contains("GM") {
        if gpu_name.contains("Laptop") {
            GpuType::Mobile9Series
        } else {
            GpuType::Desktop9Series
        }
    } else if gpu_name.contains("GV") {
        if is_server || gpu_name.contains("V100") {
            GpuType::ServerVolta
        } else {
            GpuType::ComputationVolta
        }
    } else {
        GpuType::Unknown
    }
}

/// 从 `GpuInfo` 获取 GPU 世代类型
pub fn fetch_gpu_type(info: &GpuInfo) -> Result<GpuType, Error> {
    let criteria = format!("{}{}", info.name, info.codename);
    Ok(detect_gpu_type(&criteria))
}

// ─────────────────────── GpuOcParams: OC 扫描参数 ────────────────────────────

/// GPU 世代专属的超频扫描固定参数
/// 单位均为 kHz（与 nvapi 内部单位保持一致）
#[derive(Debug, Clone, Copy)]
pub struct GpuOcParams {
    /// 每步核心频率最小步进
    pub minimum_delta_core_freq_step: i32,
    /// 核心频率偏移安全上限
    pub core_oc_safe_limit: i32,
    /// 核心频率偏移初始值（扫描起点）
    pub init_core_oc_value: i32,
    /// 每轮扫描的弹性余量
    pub safe_elasticity_per_cycle: i32,
    /// 波动系数（影响 BSOD 恢复策略）
    pub fluctuation_coefficient: i32,
    /// 是否为 50 系架构（影响 recovery 默认策略）
    pub is_50_series: bool,
    /// 电压点扫描步长（autoscan_gpuboostv3 的 testing_step）
    pub testing_step: usize,
}

/// 未知/不支持超频型号的保守默认值
impl Default for GpuOcParams {
    fn default() -> Self {
        GpuOcParams {
            minimum_delta_core_freq_step: 15000,
            core_oc_safe_limit: 300000,
            init_core_oc_value: 0,
            safe_elasticity_per_cycle: 50000,
            fluctuation_coefficient: 2,
            is_50_series: false,
            testing_step: 3,
        }
    }
}

// ──────────── GpuVoltageLimitParams: 电压限制探测参数 ────────────────────────

/// `handle_test_voltage_limits` 中按 GPU 世代决定的电压探测参数
#[derive(Debug, Clone, Copy)]
pub struct GpuVoltageLimitParams {
    /// VFP 上限初始探测点
    pub upper_init_point: usize,
    /// VFP 下限初始探测点
    pub lower_init_point: usize,
    /// 是否启用严格递增模式（平坦曲线修正）
    pub vfp_strict_inc_flag: bool,
    /// 是否启用 margin threshold 检查（50 系）
    pub margin_threshold_check: bool,
}

impl Default for GpuVoltageLimitParams {
    fn default() -> Self {
        GpuVoltageLimitParams {
            upper_init_point: 70,
            lower_init_point: 60,
            vfp_strict_inc_flag: false,
            margin_threshold_check: false,
        }
    }
}

// ──────────── GpuVoltageLockParams: 电压锁定参数 ────────────────────────────

/// `handle_lock_vfp` 中按 GPU 世代决定的电压锁定参数
#[derive(Debug, Clone, Copy)]
pub struct GpuVoltageLockParams {
    /// 是否启用 skew rate 延迟（50 系）
    pub skew_rate_enabled: bool,
    /// 电压锁定容差（mV）
    pub crit_volt_margin: i32,
}

impl Default for GpuVoltageLockParams {
    fn default() -> Self {
        GpuVoltageLockParams {
            skew_rate_enabled: false,
            crit_volt_margin: 4,
        }
    }
}

// ─────────────────────── GpuType impl ────────────────────────────────────────

impl GpuType {
    /// 返回该 GPU 世代对应的超频扫描固定参数。
    pub fn oc_params(&self) -> GpuOcParams {
        match self {
            GpuType::Mobile50Series => GpuOcParams {
                minimum_delta_core_freq_step: 7500,
                core_oc_safe_limit: 675000,
                init_core_oc_value: 330000,
                safe_elasticity_per_cycle: 60000,
                fluctuation_coefficient: 3,
                is_50_series: true,
                testing_step: 5,
            },
            GpuType::Desktop50Series => GpuOcParams {
                minimum_delta_core_freq_step: 7500,
                core_oc_safe_limit: 600000,
                init_core_oc_value: 195000,
                safe_elasticity_per_cycle: 60000,
                fluctuation_coefficient: 3,
                is_50_series: true,
                testing_step: 5,
            },
            GpuType::Mobile40Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 360000,
                init_core_oc_value: 150000,
                safe_elasticity_per_cycle: 60000,
                fluctuation_coefficient: 1,
                is_50_series: false,
                testing_step: 5,
            },
            GpuType::Desktop40Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 360000,
                init_core_oc_value: 150000,
                safe_elasticity_per_cycle: 60000,
                fluctuation_coefficient: 1,
                is_50_series: false,
                testing_step: 5,
            },
            GpuType::Mobile30Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 60000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                testing_step: 5,
            },
            GpuType::Desktop30Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 375000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 60000,
                fluctuation_coefficient: 3,
                is_50_series: false,
                testing_step: 5,
            },
            GpuType::Mobile20Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 60000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                testing_step: 3,
            },
            GpuType::Desktop20Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 60000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                testing_step: 3,
            },
            GpuType::Mobile16Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 60000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                testing_step: 3,
            },
            GpuType::Desktop16Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 60000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                testing_step: 3,
            },
            GpuType::Mobile10Series => GpuOcParams {
                minimum_delta_core_freq_step: 12500,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 50000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                testing_step: 3,
            },
            GpuType::Desktop10Series => GpuOcParams {
                minimum_delta_core_freq_step: 12500,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 50000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                testing_step: 3,
            },
            GpuType::Desktop9Series => GpuOcParams {
                minimum_delta_core_freq_step: 12500,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 00000,
                safe_elasticity_per_cycle: 37500,
                fluctuation_coefficient: 1,
                is_50_series: false,
                testing_step: 3,
            },
            GpuType::Mobile9Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 0,
                safe_elasticity_per_cycle: 50000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                testing_step: 3,
            },
            GpuType::ComputationVolta
            | GpuType::WorkstationBlackwell
            | GpuType::WorkstationLovelace
            | GpuType::WorkstationAmpere
            | GpuType::WorkstationTuring
            | GpuType::WorkstationPascal
            | GpuType::ServerBlackwell
            | GpuType::ServerHopper
            | GpuType::ServerAmpere
            | GpuType::ServerVolta
            | GpuType::ServerPascal
            | GpuType::ServerTuringTesla => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 0,
                safe_elasticity_per_cycle: 50000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                testing_step: 3,
            },
            GpuType::Unknown => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 0,
                safe_elasticity_per_cycle: 50000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                testing_step: 3,
            },
        }
    }

    /// 返回该 GPU 世代对应的电压限制探测参数。
    pub fn voltage_limit_params(&self) -> GpuVoltageLimitParams {
        match self {
            GpuType::Mobile50Series => GpuVoltageLimitParams {
                upper_init_point: 75,
                lower_init_point: 60,
                vfp_strict_inc_flag: false,
                margin_threshold_check: true,
            },
            GpuType::Desktop50Series => GpuVoltageLimitParams {
                upper_init_point: 85,
                lower_init_point: 78,
                vfp_strict_inc_flag: false,
                margin_threshold_check: true,
            },
            GpuType::Mobile40Series
            | GpuType::Mobile30Series
            | GpuType::Mobile20Series
            | GpuType::Mobile16Series => GpuVoltageLimitParams {
                upper_init_point: 75,
                lower_init_point: 60,
                vfp_strict_inc_flag: true,
                margin_threshold_check: false,
            },
            GpuType::Desktop40Series
            | GpuType::Desktop30Series
            | GpuType::Desktop20Series
            | GpuType::Desktop16Series => GpuVoltageLimitParams {
                upper_init_point: 85,
                lower_init_point: 78,
                vfp_strict_inc_flag: true,
                margin_threshold_check: false,
            },
            GpuType::Mobile10Series => GpuVoltageLimitParams {
                upper_init_point: 45,
                lower_init_point: 40,
                vfp_strict_inc_flag: true,
                margin_threshold_check: false,
            },
            GpuType::Desktop10Series => GpuVoltageLimitParams {
                upper_init_point: 48,
                lower_init_point: 40,
                vfp_strict_inc_flag: true,
                margin_threshold_check: false,
            },
            // 9 系、Volta、Unknown 使用默认值
            _ => GpuVoltageLimitParams::default(),
        }
    }

    /// 返回该 GPU 世代对应的电压锁定参数。
    pub fn voltage_lock_params(&self) -> GpuVoltageLockParams {
        match self {
            GpuType::Mobile50Series | GpuType::Desktop50Series => GpuVoltageLockParams {
                skew_rate_enabled: true,
                crit_volt_margin: 5,
            },
            _ => GpuVoltageLockParams::default(),
        }
    }

    /// 900 系（Maxwell，GM 代号）及更早 → true，需使用 SetPstates20 写 baseVoltage delta
    /// 10 系（Pascal）及以后 → false，使用 VoltRails boost
    pub fn is_legacy_voltage(&self) -> bool {
        matches!(
            self,
            GpuType::Mobile9Series | GpuType::Desktop9Series | GpuType::Unknown
        )
    }

    /// 是否为 Max-Q / Blackwell 类需要动态 margin check 的世代（50 系）
    pub fn is_maxq(&self) -> bool {
        matches!(self, GpuType::Mobile50Series | GpuType::Desktop50Series)
    }

    /// 核心频率步进（kHz），供 handle_vfp_export / fix_result 使用
    /// 直接委托 oc_params()，保持单一数据源
    pub fn minimum_freq_step_khz(&self) -> i32 {
        self.oc_params().minimum_delta_core_freq_step
    }

    /// 10~20系的 vfp表缺少 default_frequency项，30系后才有
    pub fn is_legacy_vfp(&self) -> bool {
        matches!(
            self,
            GpuType::Mobile20Series
                | GpuType::Desktop20Series
                | GpuType::Mobile16Series
                | GpuType::Desktop16Series
                | GpuType::Mobile10Series
                | GpuType::Desktop10Series
                | GpuType::WorkstationTuring
                | GpuType::ServerTuringTesla
                | GpuType::WorkstationPascal
                | GpuType::ServerPascal
        )
    }

    /// VFP 曲线点数（用于 core_reset_vfp 回退路径）
    pub fn vfp_point_range(&self) -> usize {
        match self {
            GpuType::Mobile10Series | GpuType::Desktop10Series => 79,
            _ => 126,
        }
    }
}
