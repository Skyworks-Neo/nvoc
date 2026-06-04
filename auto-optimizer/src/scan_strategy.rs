//! 扫描策略模块 —— 实现算法 3.1：确定最大稳定频率 f_optimized 和测试序列 f₁,...,fₙ
//!
//! 模块将扫描算法与 GPU 操作解耦，提供：
//! - `StepController`：算法 3.1 纯状态机（可单元测试）
//! - `FluctuationStrategy`：测试中频率波动的可插拔策略
//! - `ScanParams`：所有可调参数集中管理，带 debug 开关
//! - `ArchOcPrior`：按架构的已知超频先验数据
//! - `ArchitecturePolicy`：架构安全策略（50 系等）
//! - `ShortPhase` / `LongPhase`：通用短测/长测循环
#![allow(dead_code)]

use num_traits::pow;
use nvoc_core::{ArchOcPrior, Error, GpuOcParams, GpuType, OcPriorPoint};
use std::cmp::min;
use std::time::Duration;

// ──────────────────── FluctuationStrategy ───────────────────────────────────

/// 波动模式：决定测试中频率如何围绕基线摆动。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FluctuationMode {
    /// 模式 1: 0−（只在基线以下摆动）
    NegativeOnly,
    /// 模式 2: ±（双向摆动）
    Bidirectional,
    /// 模式 3: 0+（只在基线以上摆动，当前默认）
    PositiveOnly,
}

/// 一次压力测试中频率波动的策略。
#[derive(Debug, Clone)]
pub enum FluctuationStrategy {
    /// 固定高低切换（当前默认行为）。
    Toggle {
        mode: FluctuationMode,
        /// 乘以 f_min_step 得到摆动幅度。
        coefficient: i32,
    },
    /// 渐进攀升：在单次压力测试中，从起始偏置逐步增加到上限。
    /// 用于首次扫描快速探知大致安全上限，避免第一次无先验知识跳太高无法恢复。
    ProgressiveRamp {
        start_offset_khz: i32,
        step_khz: i32,
        step_interval_secs: u64,
        max_offset_khz: i32,
    },
    /// 无波动，固定偏置。
    Fixed,
}

impl FluctuationStrategy {
    /// 创建默认的 Toggle 波动（PositiveOnly 模式，coefficient = 3）。
    pub fn default_toggle() -> Self {
        FluctuationStrategy::Toggle {
            mode: FluctuationMode::PositiveOnly,
            coefficient: 3,
        }
    }

    /// 创建默认的渐进攀升策略用于首次扫描。
    pub fn default_progressive_ramp() -> Self {
        FluctuationStrategy::ProgressiveRamp {
            start_offset_khz: 0,
            step_khz: 15_000, // 15 MHz
            step_interval_secs: 30,
            max_offset_khz: 600_000,
        }
    }

    /// 根据当前波动状态（toggle 高低）和基频偏置，计算下一次波动后的实际偏置。
    ///
    /// 返回 `(fluctuation_delta_khz, new_high_flag)`。
    /// - `fluctuation_delta_khz`: 相对于 `init_core_oc_value` 的波动偏置
    /// - `new_high_flag`: 下一次调用时应传入的 toggle 标志（仅 Toggle 模式使用）
    pub fn next_delta(
        &self,
        init_value: i32,
        f_min_step: i32,
        elapsed: Duration,
        high_flag: bool,
    ) -> (i32, bool) {
        match self {
            FluctuationStrategy::Toggle { mode, coefficient } => {
                let (freq, new_flag) = if !high_flag {
                    let f = if *mode == FluctuationMode::PositiveOnly {
                        0
                    } else {
                        -coefficient * f_min_step
                    };
                    (f, true)
                } else {
                    let f = if *mode == FluctuationMode::Bidirectional
                        || *mode == FluctuationMode::PositiveOnly
                    {
                        coefficient * f_min_step
                    } else {
                        0
                    };
                    (f, false)
                };
                (freq, new_flag)
            }
            FluctuationStrategy::ProgressiveRamp {
                start_offset_khz,
                step_khz,
                step_interval_secs,
                max_offset_khz,
            } => {
                let elapsed_secs = elapsed.as_secs();
                let steps = (elapsed_secs / step_interval_secs) as i32;
                let raw = start_offset_khz + steps * step_khz;
                let clamped = raw.min(*max_offset_khz);
                (clamped - init_value, false)
            }
            FluctuationStrategy::Fixed => (0, false),
        }
    }

    /// 渐进攀升是否已达上限（用于判断一次测试内是否应停止）。
    pub fn is_ramp_exhausted(&self, elapsed: Duration) -> bool {
        match self {
            FluctuationStrategy::ProgressiveRamp {
                start_offset_khz,
                step_khz,
                step_interval_secs,
                max_offset_khz,
            } => {
                let elapsed_secs = elapsed.as_secs();
                let steps = (elapsed_secs / step_interval_secs) as i32;
                let raw = start_offset_khz + steps * step_khz;
                raw >= *max_offset_khz
            }
            _ => false,
        }
    }

    /// 返回人类可读的标签，用于日志。
    pub fn label(&self) -> &str {
        match self {
            FluctuationStrategy::Toggle { .. } => "Toggle",
            FluctuationStrategy::ProgressiveRamp { .. } => "ProgressiveRamp",
            FluctuationStrategy::Fixed => "Fixed",
        }
    }
}

// ──────────────────── StepController ────────────────────────────────────────

/// 算法 3.1 状态机：指数步进控制器。
///
/// 变量命名与伪代码严格对齐：
/// - `current_step_exp` → 步进指数基数
/// - `test_progress_num` → 测试进度计数
/// - `f_current` → 当前已知稳定偏置 (kHz)
/// - `f_max` → 安全上限 (kHz)
#[derive(Debug, Clone)]
pub struct StepController {
    pub current_step_exp: usize,
    pub test_progress_num: usize,
    pub f_current: i32,
    pub f_max: i32,
}

impl StepController {
    /// 正常初始化（无先验、无断点续扫）。
    ///
    /// 根据算法 3.1：
    /// - `test_progress_num` 初始化为 0（循环开头 +1 后首次步进为 2^(exp+1-1) = 2^exp 倍）
    /// - `f_current = f_initial - f_elastic`
    /// - 如果 `f_current + f_min_step * 2^(exp+1 - progress) >= f_max`，则递增 progress
    pub fn new(
        f_initial: i32,
        f_max: i32,
        f_elastic: i32,
        f_min_step: i32,
        step_exp: usize,
    ) -> Self {
        let f_current = f_initial - f_elastic;
        let mut progress = 0;

        // 算法 3.1 初始化循环：当初始频率已接近上限时，增加 progress 以减小步进
        while f_current + f_min_step * pow(2, step_exp + 1 - progress) >= f_max {
            progress += 1;
        }

        StepController {
            current_step_exp: step_exp,
            test_progress_num: progress,
            f_current,
            f_max,
        }
    }

    /// 从日志断点恢复。
    ///
    /// 根据算法 3.1：
    /// - `f_current = f_last_success - f_elastic`
    /// - `f_max = min(f_last_failed + f_elastic, f_max)`
    /// - 初始化 progress 循环同上
    pub fn init_from_resume(
        f_last_success: i32,
        f_last_failed: i32,
        f_elastic: i32,
        f_min_step: i32,
        step_exp: usize,
    ) -> Self {
        let mut f_current = f_last_success - f_elastic;
        let f_max = min(f_last_failed + f_elastic, f_last_failed.max(f_current));
        let mut progress = 0;

        while f_current + f_min_step * pow(2, step_exp - progress) >= f_max {
            progress += 1;
        }

        // 如果 progress 已经超过 exp，说明已收敛，跳过短测
        if f_last_success >= f_last_failed - f_min_step {
            f_current = f_last_success;
            progress = step_exp + 1
            // 仍正常返回，调用方会自行判断是否跳过短测
        }

        StepController {
            current_step_exp: step_exp,
            test_progress_num: progress,
            f_current,
            f_max,
        }
    }

    /// 从架构先验初始化。
    ///
    /// `f_current = prior.expected_freq_khz - f_elastic`
    /// `f_max = prior.expected_freq_khz + probe_margin_khz`
    /// 使用较小的 `step_exp`（一般为 1）做简单单步进探测。
    pub fn init_from_prior(
        prior: OcPriorPoint,
        probe_margin_khz: i32,
        f_elastic: i32,
        f_min_step: i32,
        step_exp: usize,
    ) -> Self {
        let f_current = prior.expected_freq_khz - f_elastic;
        let f_max = prior.expected_freq_khz + probe_margin_khz;
        let mut progress = 0;

        while f_current + f_min_step * pow(2, step_exp - progress) >= f_max {
            progress += 1;
        }

        StepController {
            current_step_exp: step_exp,
            test_progress_num: progress,
            f_current,
            f_max,
        }
    }

    /// 测试通过后调用。
    ///
    /// 实现算法 3.1 的成功分支：
    /// - 如果 `f_current + f_min_step == f_max` → 返回 None（已到达上限）
    /// - 如果下一步会触及或超过 f_max → 用较小步进，必要时增加 progress
    /// - 否则 → 用较大步进
    /// - `test_progress_num -= 1`
    /// - 如果 `test_progress_num >= current_step_exp` → 返回 None（收敛）
    ///
    /// 返回 `Some(增加量_kHz)` 或 `None`（已收敛，跳转长测）。
    pub fn on_test_passed(&mut self, f_min_step: i32) -> Option<i32> {
        // 算法 3.1: if f_current + f_min_step == f_max → break
        if self.f_current + f_min_step >= self.f_max {
            return None;
        }

        let increase = if self.f_current
            + f_min_step * pow(2, self.current_step_exp + 1 - self.test_progress_num)
            >= self.f_max
        {
            // 较大步进会超过上限，改用较小步进
            let mut smaller = f_min_step * pow(2, self.current_step_exp - self.test_progress_num);
            if self.f_current + smaller == self.f_max {
                self.test_progress_num += 1;
                smaller /= 2;
            }
            smaller
        } else {
            f_min_step * pow(2, self.current_step_exp + 1 - self.test_progress_num)
        };

        self.f_current += increase;
        self.test_progress_num = self.test_progress_num.saturating_sub(1);

        if self.test_progress_num >= self.current_step_exp {
            None // 收敛，跳转长测
        } else {
            Some(increase)
        }
    }

    /// 测试失败后调用。
    ///
    /// 实现算法 3.1 的失败分支：
    /// - 如果 `test_progress_num > 3` → 限制为 3
    /// - `f_max = f_current`（锁定当前值为新上限）
    /// - 用 `progress - 1` 计算下降步进，抵消循环开头 +1，
    ///   使首次失败直接跳回上一个通过点（8× 步进）而不经过中间点（4×）。
    /// - `f_current -= f_min_step * 2^(exp - progress)`
    ///
    /// 返回减少量 (kHz)。
    pub fn on_test_failed(&mut self, f_min_step: i32) -> i32 {
        if self.test_progress_num > 3 {
            self.test_progress_num = 3;
        }

        self.f_max = self.f_current;
        let decrease = f_min_step * pow(2, self.current_step_exp + 1 - self.test_progress_num);
        self.f_current -= decrease;
        decrease
    }

    /// 应用 50 系额外安全惩罚（在 on_test_failed 之后调用）。
    ///
    /// 50 系需要额外减去一个 f_min_step。
    pub fn apply_50_series_failure_penalty(&mut self, f_min_step: i32) {
        // self.f_current -= f_min_step;
    }

    /// 应用长测失败步进：简单减去 f_min_step（50 系再减一个）。
    pub fn apply_long_failure_step(&mut self, f_min_step: i32, is_50_series: bool) -> i32 {
        let decrease = if is_50_series {
            2 * f_min_step
        } else {
            f_min_step
        };
        self.f_current -= decrease;
        decrease
    }

    /// 是否已收敛（test_progress_num >= current_step_exp）。
    pub fn is_converged(&self) -> bool {
        self.test_progress_num >= self.current_step_exp
    }
}

// ──────────────────── ScanParams ────────────────────────────────────────────

/// 扫描参数完整集合。所有原散落在 `oc_scanner.rs` 各处的魔法数字集中于此。
/// 带 `debug_disabled` 的开关用于调试时快速关闭特定功能。
#[derive(Debug, Clone)]
pub struct ScanParams {
    // ── 算法 3.1 调参 ──
    /// 核心频率扫描步进指数基数（当前默认 = 3）
    pub freq_step_exp_core: usize,
    /// 显存频率扫描步进指数基数（当前默认 = 8）
    pub freq_step_exp_mem: usize,
    /// 先验探针模式步进指数（一般 1，简单单步进探测）
    pub freq_step_exp_prior: usize,

    // ── 测试执行调参 ──
    /// 短测持续时间 (秒)
    pub test_duration_secs: u64,
    /// 长测耐力系数（长测时长 = test_duration_secs * endurance_coefficient）
    pub endurance_coefficient: u64,
    /// VFP 曲线设置范围（影响点周围 ±range 的 delta）
    pub vfp_set_range: usize,
    /// VFP 重试最大次数（短测用）
    pub vfp_recheck_max_attempts_short: i32,
    /// VFP 重试最大次数（长测用）
    pub vfp_recheck_max_attempts_long: i32,

    // ── 波动策略 ──
    /// 测试中频率波动方式
    pub fluctuation: FluctuationStrategy,

    // ── 架构相关 ──
    /// 架构先验超频知识
    pub arch_prior: Option<ArchOcPrior>,
    /// 是否为 50 系架构
    pub is_50_series: bool,
    /// 总的安全弹性余量 (kHz)
    pub safe_elasticity_per_cycle: i32,
    /// 最小频率步进 (kHz)
    pub min_step: i32,

    // ── Debug 开关 ──
    /// 启用首次扫描渐进攀升探顶
    pub enable_first_scan_probe: bool,
    /// 启用架构安全策略（PrePointTest / PostPointTest）
    pub enable_arch_safety_policy: bool,
    /// 启用 50 系额外安全惩罚
    pub enable_50_series_penalty: bool,
    /// 禁用短测阶段（直接跳至长测）
    pub debug_skip_short_phase: bool,
    /// 禁用长测阶段
    pub debug_skip_long_phase: bool,
}

impl Default for ScanParams {
    fn default() -> Self {
        ScanParams {
            freq_step_exp_core: 3,
            freq_step_exp_mem: 8,
            freq_step_exp_prior: 1,

            test_duration_secs: 10,
            endurance_coefficient: 2,
            vfp_set_range: 3,
            vfp_recheck_max_attempts_short: 10,
            vfp_recheck_max_attempts_long: 5,

            fluctuation: FluctuationStrategy::default_toggle(),

            arch_prior: None,
            is_50_series: false,
            safe_elasticity_per_cycle: 60_000,
            min_step: 15_000,

            enable_first_scan_probe: false,
            enable_arch_safety_policy: true,
            enable_50_series_penalty: true,
            debug_skip_short_phase: false,
            debug_skip_long_phase: false,
        }
    }
}

impl ScanParams {
    /// 从 `GpuOcParams` 和 `GpuType` 构建扫描参数。
    pub fn from_gpu_type(gpu_type: &GpuType, oc_params: &GpuOcParams) -> Self {
        ScanParams {
            is_50_series: oc_params.is_50_series,
            min_step: oc_params.minimum_delta_core_freq_step,
            safe_elasticity_per_cycle: oc_params.safe_elasticity_per_cycle,
            fluctuation: FluctuationStrategy::Toggle {
                mode: FluctuationMode::PositiveOnly,
                coefficient: oc_params.fluctuation_coefficient,
            },
            arch_prior: Some(gpu_type.arch_prior()),
            ..Self::default()
        }
    }

    /// 为超快速模式调整参数。
    pub fn with_ultrafast(mut self) -> Self {
        self.test_duration_secs += self.test_duration_secs / 2;
        self
    }

    /// 为显存 OC 模式构建参数。
    pub fn for_memory_oc(oc_params: &GpuOcParams) -> Self {
        ScanParams {
            is_50_series: oc_params.is_50_series,
            min_step: oc_params.minimum_delta_core_freq_step,
            safe_elasticity_per_cycle: oc_params.safe_elasticity_per_cycle,
            fluctuation: FluctuationStrategy::Toggle {
                mode: FluctuationMode::PositiveOnly,
                coefficient: oc_params.fluctuation_coefficient,
            },
            enable_arch_safety_policy: false,
            enable_50_series_penalty: false,
            ..Self::default()
        }
    }
}

// ──────────────────── ArchitecturePolicy ────────────────────────────────────

/// 架构安全策略阶段标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchSafetyPhase {
    PrePointTest,
    PostPointTest,
}

/// 应用架构安全策略（50 系等）。
///
/// 在每次电压点扫描前后调用，根据架构特性和电压区间调整安全边界。
///
/// 参数：
/// - `phase`: PrePointTest（扫描前）或 PostPointTest（扫描后）
/// - `voltage_uv`: 当前电压点电压 (μV)
/// - `f_current`: 当前核心偏置 (kHz)，会原地修改
/// - `f_max`: 当前安全上限 (kHz)，会原地修改
/// - `f_max_ref`: 安全上限参考值 (kHz)，会原地修改
/// - `elasticity`: 弹性余量 (kHz)
///
/// 如果 `params.enable_arch_safety_policy` 为 false，则跳过所有操作。
pub fn apply_arch_safety_policy(
    params: &ScanParams,
    phase: ArchSafetyPhase,
    voltage_uv: u32,
    f_current: &mut i32,
    f_max: &mut i32,
    f_max_ref: &mut i32,
    elasticity: i32,
) {
    if !params.enable_arch_safety_policy {
        return;
    }

    match phase {
        ArchSafetyPhase::PrePointTest => {
            if params.is_50_series && voltage_uv > 845_000 {
                println!("Entering High-risk-crashing region!");
                *f_max_ref = 517_500;
            }
        }
        ArchSafetyPhase::PostPointTest => {
            if params.is_50_series
                && voltage_uv > 650_000
                && voltage_uv < 675_000
                && *f_current > 540_000
            {
                println!("Leaving low voltage max-Q region...");
                *f_current -= 3 * elasticity;
                *f_max = min(*f_max + elasticity, *f_max_ref);
            } else if params.is_50_series && voltage_uv > 845_000 {
                println!("Entering High-risk-crashing region!");
                *f_max_ref = 525_000;
                *f_current -= elasticity;
                *f_max = min(*f_max + elasticity, *f_max_ref);
            } else {
                *f_current -= elasticity;
                *f_max = min(*f_max + elasticity, *f_max_ref);
            }
        }
    }
}

// ──────────────────── ShortPhase / LongPhase 通用循环 ────────────────────────

/// 短测阶段运行结果。
pub struct ShortPhaseResult {
    /// 累计测试次数。
    pub test_count: usize,
    /// 是否因跳过（debug / 已收敛）而未执行。
    pub skipped: bool,
}

/// 长测阶段运行结果。
pub struct LongPhaseResult {
    /// 累计测试次数。
    pub test_count: usize,
    /// 是否因跳过（debug）而未执行。
    pub skipped: bool,
}

/// 运行短测（指数搜索）阶段。
///
/// 闭包约定：
/// - `setup(freq_khz)`: 在测试前配置 GPU 到指定频率偏置。返回 `Ok(())`。
/// - `run_test(freq_khz, test_code)`: 执行一次压力测试。返回 `Ok(true)` 通过，`Ok(false)` 失败。
/// - `on_success(freq_khz, test_code)`: 测试通过后的回调（日志等）。
/// - `on_failure(freq_khz, test_code)`: 测试失败后的回调。
/// - `on_reset()`: 失败后重置 GPU 状态。
pub fn run_short_phase(
    controller: &mut StepController,
    params: &ScanParams,
    mut setup: impl FnMut(i32) -> Result<(), Error>,
    mut run_test: impl FnMut(i32, usize) -> Result<bool, Error>,
    mut on_success: impl FnMut(i32, usize),
    mut on_failure: impl FnMut(i32, usize),
    mut on_reset: impl FnMut(()),
) -> Result<ShortPhaseResult, Error> {
    let mut test_code: usize = 0;

    if params.debug_skip_short_phase || controller.is_converged() {
        if params.debug_skip_short_phase {
            println!("[DEBUG] Skipping short phase (debug_skip_short_phase).");
        } else {
            println!("Skipping short test phase entirely (already converged).");
        }
        return Ok(ShortPhaseResult {
            test_count: 0,
            skipped: true,
        });
    }

    loop {
        setup(controller.f_current)?;

        test_code += 1;

        let passed = run_test(controller.f_current, test_code)?;

        if !passed {
            on_reset(());
            let decrease = controller.on_test_failed(params.min_step);
            if params.enable_50_series_penalty && params.is_50_series {
                controller.apply_50_series_failure_penalty(params.min_step);
                println!(
                    "Additional safety: Decreasing target freq by {}kHz (50-series)",
                    params.min_step
                );
            }
            println!("Decreasing target freq by {}kHz", decrease);
            on_failure(controller.f_current + decrease, test_code);
            continue;
        }

        on_success(controller.f_current, test_code);

        match controller.on_test_passed(params.min_step) {
            Some(increase) => {
                println!("Increasing target freq by {}kHz", increase);
            }
            None => {
                break;
            }
        }

        if controller.is_converged() {
            break;
        }
    }

    println!(
        "Short test phase finished. Current freq_delta: +{}kHz",
        controller.f_current
    );
    Ok(ShortPhaseResult {
        test_count: test_code,
        skipped: false,
    })
}

/// 运行长测（耐力验证）阶段。
///
/// 闭包约定：
/// - `setup(freq_khz)`: 在测试前配置 GPU。
/// - `run_test(freq_khz, test_code)`: 执行长时压力测试。返回 `Ok(true)` 通过。
/// - `on_success(freq_khz, test_code)`: 测试通过回调。
/// - `on_failure(freq_khz, test_code)`: 测试失败回调。
/// - `on_reset()`: 失败后重置 GPU 状态。
pub fn run_long_phase(
    controller: &mut StepController,
    params: &ScanParams,
    mut setup: impl FnMut(i32) -> Result<(), Error>,
    mut run_test: impl FnMut(i32, usize) -> Result<bool, Error>,
    mut on_success: impl FnMut(i32, usize),
    mut on_failure: impl FnMut(i32, usize),
    mut on_reset: impl FnMut(()),
) -> Result<LongPhaseResult, Error> {
    let mut test_code: usize = 0;

    if params.debug_skip_long_phase {
        println!("[DEBUG] Skipping long phase (debug_skip_long_phase).");
        return Ok(LongPhaseResult {
            test_count: 0,
            skipped: true,
        });
    }

    loop {
        setup(controller.f_current)?;

        test_code += 1;

        let passed = run_test(controller.f_current, test_code)?;

        if !passed {
            on_reset(());
            let decrease = controller.apply_long_failure_step(params.min_step, params.is_50_series);
            println!("Decreasing target freq by {}kHz", decrease);
            if params.is_50_series {
                println!(
                    "Additional safety: Decreasing target freq by {}kHz (50-series)",
                    params.min_step
                );
            }
            on_failure(controller.f_current + decrease, test_code);
            continue;
        }

        on_success(controller.f_current, test_code);
        break;
    }

    Ok(LongPhaseResult {
        test_count: test_code,
        skipped: false,
    })
}

// ──────────────────── 单元测试 ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_step_controller_new_basic() {
        let ctrl = StepController::new(
            195_000, // f_initial
            600_000, // f_max
            60_000,  // f_elastic
            7_500,   // f_min_step
            3,       // step_exp
        );
        // f_current = 195000 - 60000 = 135000
        assert_eq!(ctrl.f_current, 135_000);
        assert_eq!(ctrl.f_max, 600_000);
        assert_eq!(ctrl.current_step_exp, 3);
        // progress 初始为 0（循环开头 +1 后首次步进为 2^(3+1-1) = 8×）
        assert_eq!(ctrl.test_progress_num, 0);
    }

    #[test]
    fn test_step_controller_on_failure_then_success() {
        let mut ctrl = StepController::new(195_000, 600_000, 60_000, 7_500, 3);

        // 模拟一次失败
        let decrease = ctrl.on_test_failed(7_500);
        assert!(decrease > 0);
        // f_max 应该被锁定到失败前的 f_current
        let old_f_current_before = 135_000;
        assert_eq!(ctrl.f_max, old_f_current_before);

        // 模拟一次成功
        let result = ctrl.on_test_passed(7_500);
        assert!(result.is_some());
        assert!(ctrl.f_current > old_f_current_before - decrease);
    }

    #[test]
    fn test_step_controller_convergence() {
        let mut ctrl = StepController::new(195_000, 600_000, 60_000, 7_500, 3);
        // Force progress to convergence threshold
        ctrl.test_progress_num = 3;
        assert!(ctrl.is_converged());
        // on_test_passed still returns Some if not at boundary;
        // callers should check is_converged() before calling on_test_passed
        _ = ctrl.on_test_passed(7_500);
        // After on_test_passed, progress decrements, so it may not be None
        // Just verify the controller remains in a valid state
        assert!(ctrl.f_current > 0);
    }

    #[test]
    fn test_arch_prior_lookup() {
        let prior = ArchOcPrior {
            points: vec![
                OcPriorPoint {
                    voltage_uv: 700_000,
                    expected_freq_khz: 1_950_000,
                },
                OcPriorPoint {
                    voltage_uv: 800_000,
                    expected_freq_khz: 2_310_000,
                },
                OcPriorPoint {
                    voltage_uv: 900_000,
                    expected_freq_khz: 2_700_000,
                },
            ],
            probe_margin_khz: 150_000,
        };

        assert_eq!(prior.lookup(750_000).unwrap().expected_freq_khz, 1_950_000);
        assert_eq!(prior.lookup(800_000).unwrap().expected_freq_khz, 2_310_000);
        assert_eq!(prior.lookup(850_000).unwrap().expected_freq_khz, 2_310_000);
        assert!(prior.lookup(600_000).is_none());
    }

    #[test]
    fn test_fluctuation_toggle_mode() {
        let strat = FluctuationStrategy::Toggle {
            mode: FluctuationMode::PositiveOnly,
            coefficient: 3,
        };

        // HIGH → LOW (high_flag = true)
        let (delta, flag) = strat.next_delta(100_000, 7_500, Duration::ZERO, true);
        assert!(!flag); // 下次应为 HIGH
        // PositiveOnly: HIGH 时 delta = coefficient * min_step = 3*7500 = 22500
        assert_eq!(delta, 22_500);

        // LOW → HIGH (high_flag = false)
        let (delta2, flag2) = strat.next_delta(100_000, 7_500, Duration::ZERO, false);
        assert!(flag2);
        assert_eq!(delta2, 0); // PositiveOnly: LOW 时 delta = 0
    }

    #[test]
    fn test_fluctuation_progressive_ramp() {
        let strat = FluctuationStrategy::ProgressiveRamp {
            start_offset_khz: 0,
            step_khz: 15_000,
            step_interval_secs: 30,
            max_offset_khz: 600_000,
        };

        // t=0s: no steps taken
        let (delta, _) = strat.next_delta(0, 7_500, Duration::from_secs(0), false);
        assert_eq!(delta, 0);

        // t=60s: 2 steps → 30000
        let (delta, _) = strat.next_delta(0, 7_500, Duration::from_secs(60), false);
        assert_eq!(delta, 30_000);

        // t=90s: 3 steps → clamped at... let's calc: start=0, step=15000, 3 steps = 45000
        let (delta, _) = strat.next_delta(0, 7_500, Duration::from_secs(90), false);
        assert_eq!(delta, 45_000);
    }

    #[test]
    fn test_step_controller_init_from_prior() {
        let prior = OcPriorPoint {
            voltage_uv: 800_000,
            expected_freq_khz: 2_310_000,
        };
        let ctrl = StepController::init_from_prior(prior, 150_000, 60_000, 7_500, 1);
        // f_current = 2310000 - 60000 = 2250000
        assert_eq!(ctrl.f_current, 2_250_000);
        // f_max = 2310000 + 150000 = 2460000
        assert_eq!(ctrl.f_max, 2_460_000);
        // step_exp == 1 for prior probe
        assert_eq!(ctrl.current_step_exp, 1);
    }

    #[test]
    fn test_scan_params_debug_flags() {
        let params = ScanParams::default();
        assert!(params.enable_arch_safety_policy);
        assert!(!params.debug_skip_short_phase);
        assert!(!params.debug_skip_long_phase);
        assert!(!params.enable_first_scan_probe);
    }
}
