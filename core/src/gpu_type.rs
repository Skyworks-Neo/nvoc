//! NVIDIA GPU 世代类型定义及其关联参数。
//!
//! 将散落在 `basic_func.rs` 和 `oc_get_set_function.rs` 中的 GpuType 枚举、
//! OC 扫描参数、电压限制探测参数、电压锁定参数统一管理于此文件。

use ::nvapi::hi::GpuInfo;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::error::Error;
use crate::{ClkVfDomainHint, ClkVfPointsPrivate, ClkVfSegmentKind};

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
    // ── Kepler (GK) — GeForce 600/700 系列 (2012) ──
    MobileKepler,
    DesktopKepler,
    // ── Fermi (GF) — GeForce 400/500 系列 (2010) ──
    MobileFermi,
    DesktopFermi,
    WorkstationBlackwell,
    WorkstationLovelace,
    WorkstationAmpere,
    WorkstationTuring,
    WorkstationPascal,
    // Kepler/Fermi 工作站 (Quadro K / Quadro 4000-6000 系列)
    WorkstationKepler,
    WorkstationFermi,
    ServerBlackwell,
    ServerHopper,
    ServerLovelace,
    ServerAmpere,
    ServerVolta,
    ServerPascal,
    ServerTuringTesla,
    // Kepler/Fermi 服务器 (Tesla K20/K40/K80 / Tesla M-class)
    ServerKepler,
    ServerFermi,
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
            GpuType::MobileKepler => write!(f, "Kepler series mobile detected"),
            GpuType::DesktopKepler => write!(f, "Kepler series desktop detected"),
            GpuType::MobileFermi => write!(f, "Fermi series mobile detected"),
            GpuType::DesktopFermi => write!(f, "Fermi series desktop detected"),
            GpuType::WorkstationBlackwell => {
                write!(f, "Blackwell series workstation card detected")
            }
            GpuType::WorkstationLovelace => write!(f, "Lovelace series workstation card detected"),
            GpuType::WorkstationAmpere => write!(f, "Ampere series workstation card detected"),
            GpuType::WorkstationTuring => write!(f, "Turing series workstation card detected"),
            GpuType::WorkstationPascal => write!(f, "Pascal series workstation card detected"),
            GpuType::WorkstationKepler => {
                write!(f, "Kepler series workstation card detected")
            }
            GpuType::WorkstationFermi => write!(f, "Fermi series workstation card detected"),
            GpuType::ServerBlackwell => write!(f, "Blackwell series server card detected"),
            GpuType::ServerHopper => write!(f, "Hopper series server card detected"),
            GpuType::ServerLovelace => write!(f, "Lovelace series server card detected"),
            GpuType::ServerAmpere => write!(f, "Ampere series server card detected"),
            GpuType::ServerVolta => write!(f, "Volta series server card detected"),
            GpuType::ServerPascal => write!(f, "Pascal series server card detected"),
            GpuType::ServerTuringTesla => {
                write!(f, "Turing Tesla series server card (e.g. T4) detected")
            }
            GpuType::ServerKepler => write!(f, "Kepler series server card detected"),
            GpuType::ServerFermi => write!(f, "Fermi series server card detected"),
            GpuType::Unknown => write!(f, "Unknown"),
        }
    }
}

// ─────────────────────────── 检测 / 构造 ─────────────────────────────────────

/// 根据 GPU 名称 + codename 字符串判定世代
/// Chip-family detection. The chip prefix (GB202 / GH100 / AD102 / GA102 /
/// TU104 / GP104 / GM204 / GK104 / GF108 / GV100 …) is a property of the
/// CODENAME, never of the product name — matching it against the product
/// name misclassified Tesla VRAM suffixes as chip generations (live P100:
/// "Tesla P100-PCIE-16GB" + "GP100GL-A" hit `contains("GB")` on the "16GB"
/// capacity → ServerBlackwell, wrongly enabling Turing+ gates like the XBAR
/// offset). `chip` is matched case-sensitively from the codename only;
/// `gpu_name` drives the product-keyword classification (Tesla / Laptop /
/// Quadro / RTX professional / server SKU numbers).
pub fn detect_gpu_type(gpu_name: &str, codename: &str) -> GpuType {
    // Both sources feed the keyword checks: the product name carries
    // "Tesla"/"Laptop"/"Quadro", the codename carries "GP100"-style SKU
    // roots — either legitimately identifies a server card.
    let combined = format!("{gpu_name}{codename}");
    let is_rtx_a = combined.contains("RTX A");
    let is_rtx_professional = combined.contains("RTX")
        && (combined.contains("2000")
            || combined.contains("3000")
            || combined.contains("4000")
            || combined.contains("5000")
            || combined.contains("6000"))
        && !combined.contains("GeForce");
    let is_quadro = combined.contains("Quadro");
    let is_tesla = combined.contains("Tesla");
    let is_server = is_tesla
        || combined.contains("H100")
        || combined.contains("H800")
        || combined.contains("A100")
        || combined.contains("A800")
        || combined.contains("B100")
        || combined.contains("B200")
        || combined.contains("V100")
        || combined.contains("P100")
        || combined.contains("L40")
        || combined.contains("L4");

    // Chip family: CODENAME prefix only (see the doc comment above).
    if codename.starts_with("GB") {
        if is_server {
            GpuType::ServerBlackwell
        } else if is_rtx_professional || is_quadro {
            GpuType::WorkstationBlackwell
        } else if combined.contains("Laptop") {
            GpuType::Mobile50Series
        } else {
            GpuType::Desktop50Series
        }
    } else if codename.starts_with("GH") {
        GpuType::ServerHopper
    } else if codename.starts_with("AD") {
        if is_server {
            GpuType::ServerLovelace // L40/L4 are Ada/Lovelace server cards
        } else if is_rtx_professional || is_quadro || is_rtx_a {
            GpuType::WorkstationLovelace
        } else if combined.contains("Laptop") {
            GpuType::Mobile40Series
        } else {
            GpuType::Desktop40Series
        }
    } else if codename.starts_with("GA") {
        if is_server {
            GpuType::ServerAmpere
        } else if is_rtx_professional || is_quadro || is_rtx_a {
            GpuType::WorkstationAmpere
        } else if combined.contains("Laptop") {
            GpuType::Mobile30Series
        } else {
            GpuType::Desktop30Series
        }
    } else if codename.starts_with("TU10") {
        if is_server {
            GpuType::ServerTuringTesla
        } else if is_rtx_professional || is_quadro {
            GpuType::WorkstationTuring
        } else if combined.contains("Laptop") {
            GpuType::Mobile20Series
        } else {
            GpuType::Desktop20Series
        }
    } else if codename.starts_with("TU11") {
        if combined.contains("Laptop") {
            GpuType::Mobile16Series
        } else {
            GpuType::Desktop16Series
        }
    } else if codename.starts_with("GP1") {
        // Do NOT mess up with 'GPU'
        if is_server {
            GpuType::ServerPascal
        } else if is_quadro {
            GpuType::WorkstationPascal
        } else if combined.contains("Laptop") {
            GpuType::Mobile10Series
        } else {
            GpuType::Desktop10Series
        }
    } else if codename.starts_with("GM") {
        if combined.contains("Laptop") {
            GpuType::Mobile9Series
        } else {
            GpuType::Desktop9Series
        }
    } else if codename.starts_with("GK") {
        // Kepler (GK): GeForce 600/700 系列 (含 GT 730 GK208/GK107 变体)。
        // Tesla K20/K40/K80 → server；Quadro K 系列 → workstation。
        if is_server {
            GpuType::ServerKepler
        } else if is_quadro {
            GpuType::WorkstationKepler
        } else if combined.contains("Laptop") {
            GpuType::MobileKepler
        } else {
            GpuType::DesktopKepler
        }
    } else if codename.starts_with("GF") {
        // Fermi (GF): GeForce 400/500 系列 (含 GT 730 GF108 变体)。
        // Tesla M-class → server；Quadro 4000/5000/6000 → workstation。
        if is_server {
            GpuType::ServerFermi
        } else if is_quadro {
            GpuType::WorkstationFermi
        } else if combined.contains("Laptop") {
            GpuType::MobileFermi
        } else {
            GpuType::DesktopFermi
        }
    } else if codename.starts_with("GV") {
        // Single Volta category. The old ComputationVolta split (Titan V /
        // Quadro GV100 "consumer Volta") is retired — every GV* part is
        // compute-class for nvoc's purposes; Titan V's turbo fan is handled
        // by the fan-count refinement on the frontend, not by a separate
        // classification.
        GpuType::ServerVolta
    } else {
        GpuType::Unknown
    }
}

/// 从 `GpuInfo` 获取 GPU 世代类型
pub fn fetch_gpu_type(info: &GpuInfo) -> Result<GpuType, Error> {
    Ok(detect_gpu_type(&info.name, &info.codename))
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
    /// 是否需要在扫描期间以 MinLoadPulse 唤醒 GPU
    /// （仅 30/50 系笔记本端默认开启，Optimus 掉电唤醒用）
    pub wakeup_load_needed: bool,
    /// 电压点扫描步长（autoscan_gpuboostv3 的 testing_step）
    pub testing_step: usize,
    /// 核心频率扫描步长指数
    pub freq_step_exp_core: usize,
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
            wakeup_load_needed: false,
            testing_step: 3,
            freq_step_exp_core: 3,
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
                init_core_oc_value: 390000,
                safe_elasticity_per_cycle: 60000,
                fluctuation_coefficient: 3,
                is_50_series: true,
                wakeup_load_needed: true,
                testing_step: 5,
                freq_step_exp_core: 4,
            },
            GpuType::Desktop50Series => GpuOcParams {
                minimum_delta_core_freq_step: 7500,
                core_oc_safe_limit: 675000,
                init_core_oc_value: 390000,
                safe_elasticity_per_cycle: 60000,
                fluctuation_coefficient: 3,
                is_50_series: true,
                wakeup_load_needed: false,
                testing_step: 5,
                freq_step_exp_core: 4,
            },
            GpuType::Mobile40Series => GpuOcParams {
                minimum_delta_core_freq_step: 7500,
                core_oc_safe_limit: 360000,
                init_core_oc_value: 150000,
                safe_elasticity_per_cycle: 30000,
                fluctuation_coefficient: 1,
                is_50_series: false,
                wakeup_load_needed: false,
                testing_step: 5,
                freq_step_exp_core: 3,
            },
            GpuType::Desktop40Series => GpuOcParams {
                minimum_delta_core_freq_step: 7500,
                core_oc_safe_limit: 360000,
                init_core_oc_value: 150000,
                safe_elasticity_per_cycle: 30000,
                fluctuation_coefficient: 1,
                is_50_series: false,
                wakeup_load_needed: false,
                testing_step: 5,
                freq_step_exp_core: 3,
            },
            GpuType::Mobile30Series => GpuOcParams {
                minimum_delta_core_freq_step: 7500,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 30000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                wakeup_load_needed: true,
                testing_step: 5,
                freq_step_exp_core: 3,
            },
            GpuType::Desktop30Series => GpuOcParams {
                minimum_delta_core_freq_step: 7500,
                core_oc_safe_limit: 375000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 30000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                wakeup_load_needed: false,
                testing_step: 5,
                freq_step_exp_core: 3,
            },
            GpuType::Mobile20Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 30000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                wakeup_load_needed: false,
                testing_step: 3,
                freq_step_exp_core: 3,
            },
            GpuType::Desktop20Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 30000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                wakeup_load_needed: false,
                testing_step: 3,
                freq_step_exp_core: 3,
            },
            GpuType::Mobile16Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 30000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                wakeup_load_needed: false,
                testing_step: 3,
                freq_step_exp_core: 3,
            },
            GpuType::Desktop16Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 30000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                wakeup_load_needed: false,
                testing_step: 3,
                freq_step_exp_core: 3,
            },
            GpuType::Mobile10Series => GpuOcParams {
                minimum_delta_core_freq_step: 12500,
                core_oc_safe_limit: 435000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 30000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                wakeup_load_needed: false,
                testing_step: 3,
                freq_step_exp_core: 3,
            },
            GpuType::Desktop10Series => GpuOcParams {
                minimum_delta_core_freq_step: 12500,
                core_oc_safe_limit: 400000,
                init_core_oc_value: 90000,
                safe_elasticity_per_cycle: 30000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                wakeup_load_needed: false,
                testing_step: 3,
                freq_step_exp_core: 3,
            },
            GpuType::Desktop9Series => GpuOcParams {
                minimum_delta_core_freq_step: 12500,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 00000,
                safe_elasticity_per_cycle: 37500,
                fluctuation_coefficient: 1,
                is_50_series: false,
                wakeup_load_needed: false,
                testing_step: 3,
                freq_step_exp_core: 3,
            },
            GpuType::Mobile9Series => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 0,
                safe_elasticity_per_cycle: 37500,
                fluctuation_coefficient: 2,
                is_50_series: false,
                wakeup_load_needed: false,
                testing_step: 3,
                freq_step_exp_core: 3,
            },
            GpuType::WorkstationBlackwell
            | GpuType::WorkstationLovelace
            | GpuType::WorkstationAmpere
            | GpuType::WorkstationTuring
            | GpuType::WorkstationPascal
            | GpuType::WorkstationKepler
            | GpuType::WorkstationFermi
            | GpuType::ServerBlackwell
            | GpuType::ServerHopper
            | GpuType::ServerLovelace
            | GpuType::ServerAmpere
            | GpuType::ServerVolta
            | GpuType::ServerPascal
            | GpuType::ServerTuringTesla
            | GpuType::ServerKepler
            | GpuType::ServerFermi
            // Kepler/Fermi 消费端：legacy 电压架构，使用保守扫描参数
            // （与 Unknown/Workstation 同档：小步进、低上限、大弹性余量）。
            | GpuType::MobileKepler
            | GpuType::DesktopKepler
            | GpuType::MobileFermi
            | GpuType::DesktopFermi => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 0,
                safe_elasticity_per_cycle: 50000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                wakeup_load_needed: false,
                testing_step: 3,
                freq_step_exp_core: 3,
            },
            GpuType::Unknown => GpuOcParams {
                minimum_delta_core_freq_step: 15000,
                core_oc_safe_limit: 300000,
                init_core_oc_value: 0,
                safe_elasticity_per_cycle: 50000,
                fluctuation_coefficient: 2,
                is_50_series: false,
                wakeup_load_needed: false,
                testing_step: 3,
                freq_step_exp_core: 3,
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
    /// 10 系（Pascal）及以后 → false，使用 VoltRails boost。
    /// Quadro K/F 系工作站卡（WorkstationKepler/Fermi，如 GK106 的 K4000）
    /// 与同代 GeForce 同硅——GUI/TUI 的 BIOS VF 阶梯门控
    /// （vfcurve `_is_legacy_gpu`）依赖此旗标，漏列会导致 "No VF curve"。
    pub fn is_legacy_voltage(&self) -> bool {
        matches!(
            self,
            GpuType::Mobile9Series
                | GpuType::Desktop9Series
                | GpuType::MobileKepler
                | GpuType::DesktopKepler
                | GpuType::MobileFermi
                | GpuType::DesktopFermi
                | GpuType::WorkstationKepler
                | GpuType::WorkstationFermi
                | GpuType::Unknown
        )
    }

    /// 是否为 Max-Q / Blackwell 类需要动态 margin check 的世代（50 系）
    pub fn is_maxq(&self) -> bool {
        matches!(self, GpuType::Mobile50Series | GpuType::Desktop50Series)
    }

    /// 是否为 Blackwell 世代（消费 50 系 / 工作站 / 服务器）。
    ///
    /// ClkDomains WRITE 记录的槽位语义在 50 系整体平移（live 用户实测
    /// 2026-09-02，消费 50 系）：slot2 = 有符号频率偏移（对应 10~40 系的
    /// slot0），slot3 = V/F 曲线电压偏移 µV（对应 10~40 系的 slot1）。
    /// 私有 V/F 点记录同步变异：+0x64 从 current-MHz 变为带符号 µV 电压
    /// 偏移（−45 mV 实验回读 4294922296 = 2³² − 45000）。服务器
    /// Blackwell 未实测，按同代口径归入。
    pub fn is_blackwell(&self) -> bool {
        matches!(
            self,
            GpuType::Mobile50Series
                | GpuType::Desktop50Series
                | GpuType::WorkstationBlackwell
                | GpuType::ServerBlackwell
        )
    }

    /// 是否为 Pascal 世代（消费 10 系 / 工作站 / 服务器）。
    ///
    /// Pascal 私有 V/F 控制轴整体 2× 编码（1080 实测：公开超频 +f → 私有
    /// mode-0 读 2f；P100 实测：raw 129300 ↔ 真实 64.65 MHz；点值 raw 也是
    /// 2×，reader 已按 type-1 ÷2）。私有面的**读解码与写加倍**都以本判定
    /// 为准——消费卡日常用 public 路径写（无 2× 问题），但 private 路径
    /// （set-private-vftable-*、私有表读回）在消费卡上同样 2×。
    pub fn is_pascal(&self) -> bool {
        matches!(
            self,
            GpuType::Mobile10Series
                | GpuType::Desktop10Series
                | GpuType::WorkstationPascal
                | GpuType::ServerPascal
        )
    }

    /// 是否为 Ampere（30 系）之前的世代：消费 20/16/10/9 系 + Kepler/Fermi、
    /// Turing/Pascal 工作站、Volta/Pascal/TuringTesla/Kepler/Fermi 服务器。
    /// Unknown 一并归入（保守拒绝）。
    ///
    /// 0xAFFC2279 毒族 power-channel SET（ClientTgpWattSetStatus / compact
    /// 写核心）在这些世代上触发驱动故障：nvlddmkm 事件 14/153 错误波，
    /// 值先落地随后整张控制表回默认（2026-10-07 四机抓包 + 事件日志对齐：
    /// TU106 r610 桌面、GP100 r582 TCC 均复现；Ampere GA106 r590 返回 Ok
    /// 零故障）。set-pwr-cur-limit 的 NVAPI 路径默认按此拒绝，--force 放行
    /// 调试。本判定按名字/codename 推导；持有 nvapi-rs `Architecture` 的
    /// 调用方可用更权威的 `Architecture::is_pre_ampere`（按 GetArchInfo
    /// 架构 ID 判定）。
    pub fn is_pre_ampere(&self) -> bool {
        matches!(
            self,
            GpuType::Mobile20Series
                | GpuType::Desktop20Series
                | GpuType::Mobile16Series
                | GpuType::Desktop16Series
                | GpuType::Mobile10Series
                | GpuType::Desktop10Series
                | GpuType::Mobile9Series
                | GpuType::Desktop9Series
                | GpuType::MobileKepler
                | GpuType::DesktopKepler
                | GpuType::MobileFermi
                | GpuType::DesktopFermi
                | GpuType::WorkstationTuring
                | GpuType::WorkstationPascal
                | GpuType::WorkstationKepler
                | GpuType::WorkstationFermi
                | GpuType::ServerVolta
                | GpuType::ServerPascal
                | GpuType::ServerTuringTesla
                | GpuType::ServerKepler
                | GpuType::ServerFermi
                | GpuType::Unknown
        )
    }

    /// 是否为 Ada Lovelace 世代（消费 40 系 / 工作站）。
    ///
    /// 注意：本判定**不再**参与任何 fabric 补偿或命名——谁随谁动由驱动
    /// 自己的表给出（bank 的 vf_curve ext 槽 → `FabricTree`），按卡读、
    /// 不按世代断言；本方法目前无调用点。下面两段是 A/B 侧的历史观测，
    /// 作为那张表的旁证。
    ///
    /// Ada 上 slot-0 全位 A/B（RTX 4060 Laptop / R610，2026-08-31）：
    /// bit0=纯GPC、bit1 动 SYS+XBAR、bit2=显存 M、bit3=纯SYS、
    /// bit5=MSD、bit9=纯HOST；bit1 与 bit3 对 SYS 的效果叠加；
    /// bit4/7/8 在 GetAllClocks 无可观测反应、
    /// bit6 type-0x02 协议不搬运。
    ///
    /// 跨代汇总（2026-08-31 实测 Pascal10/GTX16/RTX20/Ampere30 + Ada）：
    /// 记录宇宙大小**不随代际单调增长**——GTX16 竟返回 10 条
    /// （0x3FF 被接受，有 MSD 与 bits 8/9），而更新的 RTX20/Ampere30
    /// 反而只有 8 条（0xFF，无 bits 8/9）。bit1 耦合轴：Ampere30+Ada
    /// 耦合（bit1 动 Sys+Xbar 且与 bit3 叠加），Pascal/GTX16/RTX20
    /// 不耦合（bit1 纯 Xbar）。MSD 轴：Pascal 无（bit5 SET 不支持），
    /// GTX16/Turing/Ampere/Ada 均有（bit5=Msd）。bit0/2/3/4/6/7 语义
    /// 五代逐位一致。50 系待测。
    ///
    /// 以上是 A/B 侧的观测。谁随谁动由**驱动自己的表**给出（bank 的
    /// vf_curve ext 槽 → `FabricTree`），按卡读、不按世代断言；本注释
    /// 的跨代观测是该表的旁证（Ada 的 ext0=SYS 正对应上面 bit1 动 SYS）。
    pub fn is_ada(&self) -> bool {
        matches!(
            self,
            GpuType::Mobile40Series | GpuType::Desktop40Series | GpuType::WorkstationLovelace
        )
    }

    /// 是否为移动端 GPU（20/30/40/50 系移动端 + Kepler/Fermi 移动端）
    pub fn is_mobile(&self) -> bool {
        matches!(
            self,
            GpuType::Mobile50Series
                | GpuType::Mobile40Series
                | GpuType::Mobile30Series
                | GpuType::Mobile20Series
                | GpuType::MobileKepler
                | GpuType::MobileFermi
        )
    }

    /// 是否为 Unknown 类型
    pub fn is_unknown(&self) -> bool {
        matches!(self, GpuType::Unknown)
    }

    /// 是否为服务器级 GPU(Tesla/数据中心被动散热卡:P100/A100/H100 …)。
    /// 这类卡绝大多数无板载可控风扇(NVML cooler count == 0),前端用该标志
    /// 同步灰化 Fan 面板,再由实际 fan count 纠错(例外:L40/L4 归入
    /// ServerLovelace 但带板载风扇,count ≥ 1 会重新点亮;Titan V/Quadro
    /// GV100 折入 ServerVolta,同理靠 fan count 重新点亮)。
    pub fn is_server(&self) -> bool {
        matches!(
            self,
            GpuType::ServerBlackwell
                | GpuType::ServerHopper
                | GpuType::ServerLovelace
                | GpuType::ServerAmpere
                | GpuType::ServerVolta
                | GpuType::ServerPascal
                | GpuType::ServerTuringTesla
                | GpuType::ServerKepler
                | GpuType::ServerFermi
        )
    }

    /// 移动端或未知 GPU 在 OC 写入前需要 GC6 唤醒
    pub fn needs_gc6_wake(&self) -> bool {
        self.is_mobile() || self.is_unknown()
    }

    /// XBAR ClockClient 域偏移（set-clk-domain-offset xbar / pynvoc
    /// set_clk_domain_offset）—— Pascal（10系）起所有架构放行：Pascal 经
    /// nvoc-cli 实测可用（2026-08-31），Volta（P100 与 T4 之间的服务器卡
    /// 系）一并放行。Kepler 及更旧、Unknown 不支持。
    /// workstation/server 卡一并放行（写入本身有 snapshot/
    /// readback/restore 保护，个别不支持会由驱动报错）。
    pub fn supports_xbar_offset(&self) -> bool {
        matches!(
            self,
            GpuType::Mobile50Series
                | GpuType::Desktop50Series
                | GpuType::Mobile40Series
                | GpuType::Desktop40Series
                | GpuType::Mobile30Series
                | GpuType::Desktop30Series
                | GpuType::Mobile20Series
                | GpuType::Desktop20Series
                | GpuType::Mobile16Series
                | GpuType::Desktop16Series
                | GpuType::Mobile10Series
                | GpuType::Desktop10Series
                | GpuType::WorkstationPascal
                | GpuType::ServerPascal
                | GpuType::ServerVolta
                | GpuType::WorkstationBlackwell
                | GpuType::WorkstationLovelace
                | GpuType::WorkstationAmpere
                | GpuType::WorkstationTuring
                | GpuType::ServerBlackwell
                | GpuType::ServerLovelace
                | GpuType::ServerAmpere
                | GpuType::ServerTuringTesla
        )
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

    /// 返回该 GPU 架构的已知超频先验数据。
    /// 扫描器可从这些数据点出发做简单单步进探测，而非从零开始指数搜索。
    pub fn arch_prior(&self) -> ArchOcPrior {
        match self {
            GpuType::Desktop50Series => ArchOcPrior {
                points: vec![
                    OcPriorPoint {
                        voltage_uv: 700_000,
                        expected_freq_khz: 1_950_000,
                    },
                    OcPriorPoint {
                        voltage_uv: 750_000,
                        expected_freq_khz: 2_130_000,
                    },
                    OcPriorPoint {
                        voltage_uv: 800_000,
                        expected_freq_khz: 2_310_000,
                    },
                    OcPriorPoint {
                        voltage_uv: 850_000,
                        expected_freq_khz: 2_490_000,
                    },
                    OcPriorPoint {
                        voltage_uv: 900_000,
                        expected_freq_khz: 2_700_000,
                    },
                    OcPriorPoint {
                        voltage_uv: 950_000,
                        expected_freq_khz: 2_880_000,
                    },
                    OcPriorPoint {
                        voltage_uv: 1_000_000,
                        expected_freq_khz: 3_030_000,
                    },
                ],
                probe_margin_khz: 150_000,
            },
            GpuType::Mobile50Series => ArchOcPrior {
                points: vec![
                    OcPriorPoint {
                        voltage_uv: 700_000,
                        expected_freq_khz: 1_800_000,
                    },
                    OcPriorPoint {
                        voltage_uv: 750_000,
                        expected_freq_khz: 2_010_000,
                    },
                    OcPriorPoint {
                        voltage_uv: 800_000,
                        expected_freq_khz: 2_190_000,
                    },
                    OcPriorPoint {
                        voltage_uv: 850_000,
                        expected_freq_khz: 2_370_000,
                    },
                    OcPriorPoint {
                        voltage_uv: 900_000,
                        expected_freq_khz: 2_550_000,
                    },
                    OcPriorPoint {
                        voltage_uv: 950_000,
                        expected_freq_khz: 2_700_000,
                    },
                    OcPriorPoint {
                        voltage_uv: 1_000_000,
                        expected_freq_khz: 2_850_000,
                    },
                ],
                probe_margin_khz: 150_000,
            },
            _ => ArchOcPrior::none(),
        }
    }
}

// ──────────────────── OcPriorPoint / ArchOcPrior ────────────────────────────

/// 一条先验数据点：(电压_μV, 已知稳定频率_kHz)
#[derive(Debug, Clone, Copy)]
pub struct OcPriorPoint {
    pub voltage_uv: u32,
    pub expected_freq_khz: i32,
}

/// 某个 GPU 架构的已知超频能力集合。
#[derive(Debug, Clone)]
pub struct ArchOcPrior {
    /// 已知 (电压, 频率) 数据点，按电压升序排列。
    pub points: Vec<OcPriorPoint>,
    /// 在先验基础上允许向上探测的冗余量 (kHz)。
    pub probe_margin_khz: i32,
}

impl ArchOcPrior {
    /// 查找 <= `voltage_uv` 的最接近先验点。
    pub fn lookup(&self, voltage_uv: u32) -> Option<OcPriorPoint> {
        self.points
            .iter()
            .rev()
            .find(|p| p.voltage_uv <= voltage_uv)
            .copied()
    }

    /// 无先验（空）。
    pub fn none() -> Self {
        ArchOcPrior {
            points: Vec::new(),
            probe_margin_khz: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Quadro K/F 系工作站卡（GK106 的 K4000 实机）与同代 GeForce 同为
    /// legacy 电压——GUI/TUI 的 BIOS VF 阶梯门控依赖此旗标；漏列会让
    /// K4000 在 vfcurve 停在 "No VF curve"（2026-09-18 实机回归）。
    #[test]
    fn workstation_kepler_fermi_are_legacy_voltage() {
        assert_eq!(
            detect_gpu_type("Quadro K4000", "GK106"),
            GpuType::WorkstationKepler
        );
        assert!(detect_gpu_type("Quadro K4000", "GK106").is_legacy_voltage());
        assert!(GpuType::WorkstationKepler.is_legacy_voltage());
        assert!(GpuType::WorkstationFermi.is_legacy_voltage());
        // 非 legacy 对照：Pascal 工作站 / 消费 10 系
        assert!(!GpuType::WorkstationPascal.is_legacy_voltage());
        assert!(!GpuType::Desktop10Series.is_legacy_voltage());
    }

    /// set-pwr-cur-limit 的 pre-Ampere 拒绝门（2026-10-07 定案：0xAFFC2279
    /// SET 在 30 系之前触发 nvlddmkm 14/153 故障波、值先落地后整表回默认；
    /// Ampere+ 返回 Ok 零故障）。四台实测机各占一行；Unknown 保守拒绝。
    #[test]
    fn pre_ampere_refusal_classification() {
        // 实测故障机：Turing 桌面（TU106）+ Pascal TCC（GP100）
        assert!(detect_gpu_type("NVIDIA GeForce RTX 2070", "TU106").is_pre_ampere());
        assert!(detect_gpu_type("Tesla P100-PCIE-16GB", "GP100").is_pre_ampere());
        // Ampere+ 对照：GA106 3060、AD107 4060 Laptop 不拒
        assert!(!detect_gpu_type("NVIDIA GeForce RTX 3060", "GA106").is_pre_ampere());
        assert!(!detect_gpu_type("NVIDIA GeForce RTX 4060 Laptop GPU", "AD107").is_pre_ampere());
        // 代际覆盖：20/16/10/9 系拒，30/40/50 系不拒；Unknown 保守拒
        assert!(GpuType::Desktop20Series.is_pre_ampere());
        assert!(GpuType::Mobile16Series.is_pre_ampere());
        assert!(GpuType::Mobile9Series.is_pre_ampere());
        assert!(GpuType::WorkstationTuring.is_pre_ampere());
        assert!(GpuType::ServerVolta.is_pre_ampere());
        assert!(!GpuType::Desktop30Series.is_pre_ampere());
        assert!(!GpuType::Mobile40Series.is_pre_ampere());
        assert!(!GpuType::Desktop50Series.is_pre_ampere());
        assert!(!GpuType::WorkstationAmpere.is_pre_ampere());
        assert!(!GpuType::ServerHopper.is_pre_ampere());
        assert!(GpuType::Unknown.is_pre_ampere());
    }
}

// ────────────────────── Fabric 域附着关系与净值求解 ──────────────────────
//
// Fabric-domain attachment: which ClkDomains WRITE-record offsets ride into
// which other domains, and how to resolve a **net** (sign-corrected) write.
//
// ## Where the relation comes from
//
// The driver declares the attachment in its private V/F table: each bank's
// main `vf_curve` block carries optional EXTENDED-section slots
// (`ClkVfPointPrivate::domain_freqs_mhz`, `+0x74+0x10*k`) holding the
// *derived* operating point of the fabric domains that have no main block of
// their own. `get-private-vftable` on an RTX 4060 (Ada/R610.74) reads
// `bank0 xbar vf_curve … ext0=SYS ext1=HOST` — XBAR is the parent, SYS and
// HOST its attached children: moving XBAR drags them, and their own offsets
// stack on top of the dragged value. The slot roster is
// `[XBAR, SYS, MSD, HOST]` minus every domain that owns a main `vf_curve`
// block **in this table** — a layout fact, never a generation table.
//
// Two live dumps agree on the rule, on two generations:
//
// * RTX 4060 Laptop (Ada / R610.74): `bank0 xbar vf_curve … ext0=SYS
//   ext1=HOST`. MSD owns a main block of its own on Ada, so the driver packs
//   only SYS/HOST into the xbar record. A/B: writing bit1 moves SYS and HOST;
//   it does NOT move MSD — which is exactly MSD's absence from the roster.
// * 30 系 (Ampere): the same record carries `ext0=SYS ext1=MSD ext2=HOST`.
//   MSD owns no main block there, and all three follow the XBAR offset (the
//   master→slave linkage was observed live on SYS/HOST/MSD alike).
//
// **Every edge the table declares is applied** — master/slave control follows
// the driver's own declaration. The one thing we must not do is guess when we
// cannot see it: a table that never arrived leaves
// [`FabricTree::table_available`] false and the front-ends refuse the fabric
// writes outright. (A table whose ext slots come back undecoded — Blackwell
// today — is a card we cannot yet see the relation *of*; it reads as a table
// declaring no edge and writes raw, see [`FabricTree::from_table`].)
//
// ## Net semantics
//
// `net(d) = own(d) + Σ own(parents of d)`. A row displays and writes the NET
// value; [`FabricTree::plan`] turns a set of net targets into the raw
// WRITE-record values that realize them, re-parking every non-target child of
// a moved parent so its net stays put, in topological order (parents first).
//
// 私有 V/F 表的 ext 槽解码 + 净值守恒求解器，见下。

/// Slots observed per (owner, ext slot) before the slot counts as an edge —
/// mirrors the ext-curve plausibility gate the GUI/TUI extractors use
/// (`_extract_ext_curves`), so a single stray record cannot invent a relation.
const MIN_SLOT_POINTS: usize = 4;

/// The fabric domains that can appear in the ext roster, in slot order.
///
/// **Universe warning**: these are ClkDomains **WRITE-record** bits (the
/// `clk_client_record_name` list), *not* the MEASURE/RTSS bit space and *not*
/// `GetAllClocks`'s `ClockDomainId`. The three are disjoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FabricDomain {
    Xbar,
    Sys,
    Msd,
    Host,
}

impl FabricDomain {
    /// Roster order = ext slot order.
    pub const POOL: [FabricDomain; 4] = [
        FabricDomain::Xbar,
        FabricDomain::Sys,
        FabricDomain::Msd,
        FabricDomain::Host,
    ];

    /// ClkDomains WRITE-record bit for this domain.
    pub fn bit(self) -> u8 {
        match self {
            FabricDomain::Xbar => 1,
            FabricDomain::Sys => 3,
            FabricDomain::Msd => 5,
            FabricDomain::Host => 9,
        }
    }

    pub fn from_bit(bit: u8) -> Option<Self> {
        match bit {
            1 => Some(FabricDomain::Xbar),
            3 => Some(FabricDomain::Sys),
            5 => Some(FabricDomain::Msd),
            9 => Some(FabricDomain::Host),
            _ => None,
        }
    }

    /// lowercase slug used in CLI/JSON payloads.
    pub fn slug(self) -> &'static str {
        match self {
            FabricDomain::Xbar => "xbar",
            FabricDomain::Sys => "sys",
            FabricDomain::Msd => "msd",
            FabricDomain::Host => "host",
        }
    }

    /// UPPERCASE name as it appears in the ext-slot roster legend.
    pub fn roster_name(self) -> &'static str {
        match self {
            FabricDomain::Xbar => "XBAR",
            FabricDomain::Sys => "SYS",
            FabricDomain::Msd => "MSD",
            FabricDomain::Host => "HOST",
        }
    }

    pub fn from_roster_name(name: &str) -> Option<Self> {
        FabricDomain::POOL
            .into_iter()
            .find(|d| d.roster_name().eq_ignore_ascii_case(name))
    }

    /// Map a `vf_curve` segment's empirical domain hint into this space.
    /// `Gpc`, the Pascal server dual-plane pair (`GpcPreOc`/`GpcOc` — both
    /// are GPC-physical planes of one curve), `Mem`/`Disp`/`Unknown` have
    /// no fabric WRITE bit of their own.
    pub fn from_hint(hint: ClkVfDomainHint) -> Option<Self> {
        match hint {
            ClkVfDomainHint::Xbar => Some(FabricDomain::Xbar),
            ClkVfDomainHint::Msd => Some(FabricDomain::Msd),
            _ => None,
        }
    }
}

/// Resolved relation for one GPU, read out of the driver's own table.
#[derive(Debug, Clone, Default)]
pub struct FabricTree {
    roster: Vec<FabricDomain>,
    edges: Vec<(FabricDomain, FabricDomain)>,
    table_available: bool,
}

impl FabricTree {
    /// Build the relation from the **unfiltered** private V/F table (both
    /// banks, every domain) — a `--bank/--domain` filtered view would distort
    /// the roster (a domain filtered out reappears in it and shifts every
    /// later slot) and must never be passed here.
    ///
    /// `None` (no table at all: read unsupported, or the call failed) = no
    /// roster, no edges, `table_available() == false`: the relation is
    /// UNKNOWN, and the front-ends refuse fabric writes rather than guess.
    ///
    /// A table whose ext slots do not come back decoded (Blackwell today —
    /// the reader gates that layout, see `nvapi-rs` `blackwell_layout`)
    /// arrives here as a table that declares no edge, so a 50-series write
    /// goes out raw, the way it did before this relation existed. Giving
    /// those cards their true edges is a separate change.
    pub fn from_table(table: Option<&ClkVfPointsPrivate>) -> Self {
        match table {
            Some(t) => {
                let (roster, edges) = derive_structure(t);
                Self {
                    roster,
                    edges: edges.into_iter().collect(),
                    table_available: true,
                }
            }
            None => Self::default(),
        }
    }

    pub fn table_available(&self) -> bool {
        self.table_available
    }

    /// Roster-minus-owners as read from this table (empty when unavailable).
    pub fn roster(&self) -> &[FabricDomain] {
        &self.roster
    }

    /// Every attachment the driver's table declares: `(master, slave)`.
    pub fn edges(&self) -> &[(FabricDomain, FabricDomain)] {
        &self.edges
    }

    /// Parents (masters) whose offset rides into `child`. Empty = the driver
    /// attaches nothing to it — the row writes raw.
    pub fn parents(&self, child: FabricDomain) -> Vec<FabricDomain> {
        self.edges
            .iter()
            .filter(|(_, c)| *c == child)
            .map(|(p, _)| *p)
            .collect()
    }

    pub fn children(&self, parent: FabricDomain) -> Vec<FabricDomain> {
        self.edges
            .iter()
            .filter(|(p, _)| *p == parent)
            .map(|(_, c)| *c)
            .collect()
    }

    /// Does this relation attach anything at all? Front-ends use it to skip
    /// the net path (and its extra reads) on a card where nothing does.
    pub fn has_edges(&self) -> bool {
        !self.edges.is_empty()
    }

    /// NET offset (kHz) of `d`: its own raw value plus every parent's.
    pub fn net_khz(&self, own_now: &BTreeMap<u8, i64>, d: FabricDomain) -> i64 {
        let mut total = own_now.get(&d.bit()).copied().unwrap_or(0);
        for p in self.parents(d) {
            total += own_now.get(&p.bit()).copied().unwrap_or(0);
        }
        total
    }

    /// Resolve net targets into raw WRITE-record values (kHz).
    ///
    /// `own_now` is the current raw frequency-plane value per WRITE bit
    /// (missing = 0). Returns `(bit, kHz)` writes for the frequency plane in
    /// topological order (parents first), **omitting no-ops** so an unchanged
    /// target re-applies cleanly. Every non-target child of a moved parent is
    /// re-parked to hold its net constant.
    pub fn plan(
        &self,
        own_now: &BTreeMap<u8, i64>,
        targets: &BTreeMap<FabricDomain, i64>,
    ) -> Vec<(u8, i64)> {
        let mut nodes: BTreeSet<FabricDomain> = targets.keys().copied().collect();
        for d in FabricDomain::POOL {
            if own_now.contains_key(&d.bit()) {
                nodes.insert(d);
            }
        }
        for &(parent, child) in &self.edges {
            nodes.insert(parent);
            nodes.insert(child);
        }
        let order = self.topological(&nodes);
        let mut new_own: BTreeMap<u8, i64> = own_now.clone();
        let mut writes: Vec<(u8, i64)> = Vec::new();
        for d in order {
            let parents = self.parents(d);
            let old = own_now.get(&d.bit()).copied().unwrap_or(0);
            let upstream_moved = parents.iter().any(|p| {
                new_own.get(&p.bit()).copied().unwrap_or(0)
                    != own_now.get(&p.bit()).copied().unwrap_or(0)
            });
            let target = targets.get(&d).copied();
            if target.is_none() && !upstream_moved {
                continue; // untouched and nothing above it moved
            }
            let parent_sum: i64 = parents
                .iter()
                .map(|p| new_own.get(&p.bit()).copied().unwrap_or(0))
                .sum();
            // A target's own := target − Σ parents. A dragged non-target keeps
            // the net it had: own := net_old − Σ new parents.
            let new = match target {
                Some(t) => t - parent_sum,
                None => {
                    let net_old: i64 = old
                        + parents
                            .iter()
                            .map(|p| own_now.get(&p.bit()).copied().unwrap_or(0))
                            .sum::<i64>();
                    net_old - parent_sum
                }
            };
            if new != old {
                writes.push((d.bit(), new));
            }
            new_own.insert(d.bit(), new);
        }
        writes
    }

    /// Parents before children; a cycle (never expected — the driver's fabric
    /// is a forest) is broken by dropping the back edge rather than looping.
    fn topological(&self, nodes: &BTreeSet<FabricDomain>) -> Vec<FabricDomain> {
        let mut out = Vec::new();
        let mut done: BTreeSet<FabricDomain> = BTreeSet::new();
        let mut visiting: BTreeSet<FabricDomain> = BTreeSet::new();
        for &d in nodes {
            self.visit(d, nodes, &mut out, &mut done, &mut visiting);
        }
        out
    }

    fn visit(
        &self,
        d: FabricDomain,
        nodes: &BTreeSet<FabricDomain>,
        out: &mut Vec<FabricDomain>,
        done: &mut BTreeSet<FabricDomain>,
        visiting: &mut BTreeSet<FabricDomain>,
    ) {
        if done.contains(&d) || visiting.contains(&d) {
            return; // already emitted, or a back edge in a cycle
        }
        visiting.insert(d);
        for p in self.parents(d) {
            if nodes.contains(&p) {
                self.visit(p, nodes, out, done, visiting);
            }
        }
        visiting.remove(&d);
        if done.insert(d) {
            out.push(d);
        }
    }
}

/// Read the attachment structure out of an unfiltered private V/F table:
/// `(roster, edges)`.
///
/// Roster = `[XBAR, SYS, MSD, HOST]` minus every domain owning a main
/// `vf_curve` block **in this table** (positions, not generations). A
/// `vf_curve` segment is the owner of the points inside its index range; a
/// slot `k` populated on ≥ [`MIN_SLOT_POINTS`] of them turns `owner → roster[k]`
/// into an edge.
fn derive_structure(
    table: &ClkVfPointsPrivate,
) -> (Vec<FabricDomain>, BTreeSet<(FabricDomain, FabricDomain)>) {
    let segments: Vec<_> = table
        .segments
        .iter()
        .filter(|s| s.kind == ClkVfSegmentKind::VfCurve)
        .collect();
    let owners: BTreeSet<FabricDomain> = segments
        .iter()
        .filter_map(|s| FabricDomain::from_hint(s.domain_hint))
        .collect();
    let roster: Vec<FabricDomain> = FabricDomain::POOL
        .into_iter()
        .filter(|d| !owners.contains(d))
        .collect();
    let mut counts: BTreeMap<(FabricDomain, usize), usize> = BTreeMap::new();
    for p in &table.points {
        let owner = segments
            .iter()
            .find(|s| {
                s.bank == p.bank
                    && s.start_index as usize <= p.index as usize
                    && p.index as usize <= s.end_index as usize
            })
            .and_then(|s| FabricDomain::from_hint(s.domain_hint));
        let Some(owner) = owner else { continue };
        for k in 0..roster.len().min(4).min(p.domain_freqs_mhz.len()) {
            if p.domain_freqs_mhz[k] > 0 && p.domain_volts_uV[k] > 0 {
                *counts.entry((owner, k)).or_default() += 1;
            }
        }
    }
    let edges = counts
        .into_iter()
        .filter(|&(_, n)| n >= MIN_SLOT_POINTS)
        .map(|((owner, k), _)| (owner, roster[k]))
        .collect();
    (roster, edges)
}

#[cfg(test)]
mod fabric_tests {
    use super::*;
    use crate::{ClkVfPointPrivate, ClkVfSegment, ClkVfSegmentKind};

    fn point(bank: u8, index: u16, slots: &[(usize, u32, u32)]) -> ClkVfPointPrivate {
        let mut p = ClkVfPointPrivate {
            bank,
            index,
            record_type: 8,
            ..Default::default()
        };
        for &(k, f, v) in slots {
            p.domain_freqs_mhz[k] = f;
            p.domain_volts_uV[k] = v;
        }
        p
    }

    fn seg(bank: u8, hint: ClkVfDomainHint, start: u16, end: u16) -> ClkVfSegment {
        ClkVfSegment {
            bank,
            record_type: 8,
            kind: ClkVfSegmentKind::VfCurve,
            domain_hint: hint,
            start_index: start,
            end_index: end,
            count: end - start + 1,
            ..Default::default()
        }
    }

    /// Ada/R610.74 shape: gpc + xbar + msd all have main blocks in bank 0, the
    /// xbar block carries ext0=SYS, ext1=HOST (live 4060 dump).
    fn ada_table() -> ClkVfPointsPrivate {
        let mut t = ClkVfPointsPrivate {
            segments: vec![
                seg(0, ClkVfDomainHint::Gpc, 0, 126),
                seg(0, ClkVfDomainHint::Xbar, 127, 253),
                seg(0, ClkVfDomainHint::Msd, 254, 380),
            ],
            ..Default::default()
        };
        for i in 0..8u16 {
            t.points.push(point(
                0,
                127 + i,
                &[(0, 2400 + i as u32, 1_000_000), (1, 900, 800_000)],
            ));
        }
        t
    }

    /// 30 系 live dump shape: only gpc + xbar own main blocks, and the xbar
    /// block fills all three slots (`ext0=SYS ext1=MSD ext2=HOST`).
    fn ampere_table() -> ClkVfPointsPrivate {
        let mut t = ClkVfPointsPrivate {
            segments: vec![
                seg(0, ClkVfDomainHint::Gpc, 0, 126),
                seg(0, ClkVfDomainHint::Xbar, 127, 253),
            ],
            ..Default::default()
        };
        for i in 0..8u16 {
            t.points.push(point(
                0,
                127 + i,
                &[(0, 2400, 1_000_000), (1, 1900, 900_000), (2, 1600, 850_000)],
            ));
        }
        t
    }

    /// Turing shape: the gpc block is the only main block, 4 slots = the
    /// whole pool.
    fn turing_table() -> ClkVfPointsPrivate {
        let mut t = ClkVfPointsPrivate {
            segments: vec![seg(0, ClkVfDomainHint::Gpc, 0, 126)],
            ..Default::default()
        };
        for i in 0..8u16 {
            t.points.push(point(
                0,
                i,
                &[
                    (0, 2400, 1_000_000),
                    (1, 1900, 900_000),
                    (2, 1600, 850_000),
                    (3, 1300, 800_000),
                ],
            ));
        }
        t
    }

    fn own(pairs: &[(u8, i64)]) -> BTreeMap<u8, i64> {
        pairs.iter().copied().collect()
    }

    fn targets(pairs: &[(FabricDomain, i64)]) -> BTreeMap<FabricDomain, i64> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn ada_roster_and_edges_match_the_live_dump() {
        let tree = FabricTree::from_table(Some(&ada_table()));
        assert!(tree.table_available());
        assert_eq!(tree.roster(), [FabricDomain::Sys, FabricDomain::Host]);
        assert_eq!(
            tree.edges(),
            [
                (FabricDomain::Xbar, FabricDomain::Sys),
                (FabricDomain::Xbar, FabricDomain::Host)
            ]
        );
        // MSD owns a main block of its own on Ada — it is off the roster, and
        // no edge may point at it (the A/B agrees: bit1 does not move MSD).
        assert_eq!(tree.parents(FabricDomain::Msd), vec![]);
        assert!(tree.has_edges());
    }

    #[test]
    fn ampere_attaches_every_slot_the_driver_packs() {
        // 30 系 live dump: ext0=SYS ext1=MSD ext2=HOST — all three follow the
        // XBAR offset, so all three compensate.
        let tree = FabricTree::from_table(Some(&ampere_table()));
        assert_eq!(
            tree.roster(),
            [FabricDomain::Sys, FabricDomain::Msd, FabricDomain::Host]
        );
        assert_eq!(
            tree.children(FabricDomain::Xbar),
            vec![FabricDomain::Sys, FabricDomain::Msd, FabricDomain::Host]
        );
        for child in [FabricDomain::Sys, FabricDomain::Msd, FabricDomain::Host] {
            assert_eq!(tree.parents(child), vec![FabricDomain::Xbar], "{child:?}");
        }
        assert_eq!(tree.parents(FabricDomain::Xbar), vec![]);
    }

    #[test]
    fn turing_gpc_slots_are_not_fabric_edges() {
        // The only main block is GPC's and GPC has no fabric WRITE bit: the
        // whole pool is roster, nothing is attached, and a Core offset never
        // re-parks a fabric record.
        let tree = FabricTree::from_table(Some(&turing_table()));
        assert_eq!(
            tree.roster(),
            [
                FabricDomain::Xbar,
                FabricDomain::Sys,
                FabricDomain::Msd,
                FabricDomain::Host
            ]
        );
        assert!(!tree.has_edges());
        let plan = tree.plan(
            &own(&[(1, 50_000)]),
            &targets(&[(FabricDomain::Sys, 30_000)]),
        );
        assert_eq!(plan, vec![(3, 30_000)]);
    }

    #[test]
    fn no_table_means_no_relation_at_all() {
        // A read that never produced a table (unsupported arch, failed call):
        // empty roster, no edges, `table_available` false — the front-ends
        // read that as UNKNOWN and refuse a fabric write instead of guessing
        // one.
        let tree = FabricTree::from_table(None);
        assert!(!tree.table_available());
        assert!(tree.roster().is_empty());
        assert!(!tree.has_edges());
        assert_eq!(tree.parents(FabricDomain::Sys), vec![]);
        assert_eq!(tree.parents(FabricDomain::Xbar), vec![]);
    }

    // ── plan() must reproduce the shipped single-edge formulas exactly ──

    fn ada_tree() -> FabricTree {
        FabricTree::from_table(Some(&ada_table()))
    }

    #[test]
    fn xbar_write_parks_the_child_cancels_and_holds_both_nets() {
        let tree = ada_tree();
        // Pre-write: no Xbar offset yet, SYS net −40 MHz, HOST net −10 MHz.
        // Writing Xbar = +200 MHz must leave both nets exactly where they were.
        let now = own(&[(1, 0), (3, -40_000), (9, -10_000)]);
        let plan = tree.plan(&now, &targets(&[(FabricDomain::Xbar, 200_000)]));
        assert_eq!(plan, vec![(1, 200_000), (3, -240_000), (9, -210_000)]);
        let after: BTreeMap<u8, i64> = now
            .iter()
            .map(|(k, v)| (*k, *v))
            .chain(plan.iter().copied())
            .collect();
        assert_eq!(tree.net_khz(&after, FabricDomain::Sys), -40_000);
        assert_eq!(tree.net_khz(&after, FabricDomain::Host), -10_000);
    }

    #[test]
    fn ampere_master_move_parks_msd_and_host_too() {
        // 30 系: MSD rides XBAR exactly like SYS/HOST (live linkage), so a
        // master move must hold all three nets.
        let tree = FabricTree::from_table(Some(&ampere_table()));
        let now = own(&[(1, 0), (3, -40_000), (5, 20_000), (9, -10_000)]);
        let plan = tree.plan(&now, &targets(&[(FabricDomain::Xbar, 100_000)]));
        assert_eq!(
            plan,
            vec![(1, 100_000), (3, -140_000), (5, -80_000), (9, -110_000)]
        );
        let after: BTreeMap<u8, i64> = now
            .iter()
            .map(|(k, v)| (*k, *v))
            .chain(plan.iter().copied())
            .collect();
        assert_eq!(tree.net_khz(&after, FabricDomain::Sys), -40_000);
        assert_eq!(tree.net_khz(&after, FabricDomain::Msd), 20_000);
        assert_eq!(tree.net_khz(&after, FabricDomain::Host), -10_000);
    }

    #[test]
    fn xbar_write_is_idempotent() {
        let tree = ada_tree();
        let now = own(&[(1, 200_000), (3, -40_000), (9, -150_000)]);
        assert_eq!(
            tree.plan(&now, &targets(&[(FabricDomain::Xbar, 200_000)])),
            vec![]
        );
    }

    #[test]
    fn sys_write_resolves_the_net_against_the_parent() {
        let tree = ada_tree();
        let now = own(&[(1, 120_000), (3, 30_000), (9, 0)]);
        // net SYS target +80 → bit3 := 80_000 − 120_000 = −40_000
        assert_eq!(
            tree.plan(&now, &targets(&[(FabricDomain::Sys, 80_000)])),
            vec![(3, -40_000)]
        );
    }

    #[test]
    fn host_write_is_now_net_aware_on_ada() {
        let tree = ada_tree();
        let now = own(&[(1, 100_000), (3, 0), (9, 0)]);
        assert_eq!(
            tree.plan(&now, &targets(&[(FabricDomain::Host, 50_000)])),
            vec![(9, -50_000)]
        );
    }

    #[test]
    fn resetting_a_row_zeroes_its_net_and_keeps_the_siblings() {
        let tree = ada_tree();
        // Xbar ↺: bit1 := 0; SYS net (−100_000+? no: 100_000−100_000=0) and HOST
        // net 140_000 both survive — bit3/bit9 absorb the parent's departure.
        let now = own(&[(1, 100_000), (3, -100_000), (9, 40_000)]);
        assert_eq!(
            tree.plan(&now, &targets(&[(FabricDomain::Xbar, 0)])),
            vec![(1, 0), (3, 0), (9, 140_000)]
        );
        let after = own(&[(1, 0), (3, 0), (9, 140_000)]);
        assert_eq!(tree.net_khz(&after, FabricDomain::Sys), 0);
        assert_eq!(tree.net_khz(&after, FabricDomain::Host), 140_000);
        // Sys ↺ with a live SYS net: bit3 := −bit1; XBAR/HOST untouched. This
        // is the write today's `_reset_sys_domain_action` performs, and it is
        // now the *net* zero (old code also wrote bit3 := 0 → net became bit1).
        let now = own(&[(1, 100_000), (3, 30_000), (9, 40_000)]);
        assert_eq!(
            tree.plan(&now, &targets(&[(FabricDomain::Sys, 0)])),
            vec![(3, -100_000)]
        );
        // …and an already-zero SYS net needs no write at all (idempotent ↺).
        let now = own(&[(1, 100_000), (3, -100_000), (9, 40_000)]);
        assert_eq!(tree.plan(&now, &targets(&[(FabricDomain::Sys, 0)])), vec![]);
    }

    #[test]
    fn joint_targets_solve_in_parent_first_order() {
        let tree = ada_tree();
        let now = own(&[(1, 0), (3, 0), (9, 0)]);
        let plan = tree.plan(
            &now,
            &targets(&[(FabricDomain::Xbar, 100_000), (FabricDomain::Sys, 30_000)]),
        );
        // bit1 := 100; bit3 := 30 − 100 = −70; HOST keeps net 0 → bit9 := −100
        assert_eq!(plan, vec![(1, 100_000), (3, -70_000), (9, -100_000)]);
    }

    #[test]
    fn zeroing_every_own_zeroes_every_net() {
        // "reset all domains" with a complete read of the four records.
        let tree = ada_tree();
        let now = own(&[(1, 100_000), (3, -100_000), (5, 70_000), (9, 40_000)]);
        let all: BTreeMap<FabricDomain, i64> =
            FabricDomain::POOL.into_iter().map(|d| (d, 0)).collect();
        let plan = tree.plan(&now, &all);
        assert_eq!(plan, vec![(1, 0), (3, 0), (5, 0), (9, 0)]);
        let after: BTreeMap<u8, i64> = plan.iter().copied().collect();
        assert_eq!(tree.net_khz(&after, FabricDomain::Sys), 0);
        assert_eq!(tree.net_khz(&after, FabricDomain::Host), 0);
    }

    #[test]
    fn missing_records_read_as_zero_and_still_hold_their_net() {
        // `own_now` is the caller's read of each record; an absent bit is 0,
        // not "skip me" — so a parent move still parks that record's believed
        // net instead of silently dropping it. (Front-ends must therefore
        // hand in every record they claim to manage: the four fabric rows all
        // come from one `get-private-freq-domain-info` read.)
        let tree = ada_tree();
        let now = own(&[(1, 100_000), (9, 40_000)]); // bit3 never read → 0
        assert_eq!(tree.net_khz(&now, FabricDomain::Sys), 100_000);
        assert_eq!(
            tree.plan(&now, &targets(&[(FabricDomain::Xbar, 0)])),
            vec![(1, 0), (3, 100_000), (9, 140_000)]
        );
    }

    #[test]
    fn net_khz_sums_every_parent() {
        let tree = ada_tree();
        let now = own(&[(1, 100_000), (3, 20_000), (5, 70_000), (9, -30_000)]);
        assert_eq!(tree.net_khz(&now, FabricDomain::Sys), 120_000);
        assert_eq!(tree.net_khz(&now, FabricDomain::Host), 70_000);
        // MSD owns a main block on Ada → nothing rides into it
        assert_eq!(tree.net_khz(&now, FabricDomain::Msd), 70_000);
        assert_eq!(tree.net_khz(&now, FabricDomain::Xbar), 100_000);
    }

    #[test]
    fn a_single_stray_record_does_not_invent_an_edge() {
        // One msd-block point carrying a slot-0 value is below
        // MIN_SLOT_POINTS — it must not turn into an `msd → sys` attachment.
        let mut t = ada_table();
        t.points.push(point(0, 260, &[(0, 900, 800_000)]));
        let tree = FabricTree::from_table(Some(&t));
        assert_eq!(tree.roster(), [FabricDomain::Sys, FabricDomain::Host]);
        assert_eq!(
            tree.edges(),
            [
                (FabricDomain::Xbar, FabricDomain::Sys),
                (FabricDomain::Xbar, FabricDomain::Host)
            ]
        );
    }

    #[test]
    fn domains_map_to_the_write_record_bits() {
        assert_eq!(FabricDomain::from_bit(9), Some(FabricDomain::Host));
        assert_eq!(
            FabricDomain::from_roster_name("host"),
            Some(FabricDomain::Host)
        );
        assert_eq!(FabricDomain::from_hint(ClkVfDomainHint::Gpc), None);
        assert_eq!(
            FabricDomain::from_hint(ClkVfDomainHint::Xbar),
            Some(FabricDomain::Xbar)
        );
        assert_eq!(FabricDomain::Xbar.bit(), 1);
        assert_eq!(FabricDomain::Sys.bit(), 3);
        assert_eq!(FabricDomain::Msd.bit(), 5);
        assert_eq!(FabricDomain::Host.bit(), 9);
    }
}
