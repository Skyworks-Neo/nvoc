use anstream::eprintln;
#[cfg(any(feature = "cuda", feature = "vulkan", feature = "video"))]
use anstream::println;
use clap::Parser;
#[cfg(feature = "cuda")]
use clap::{CommandFactory, FromArgMatches, parser::ValueSource};
#[cfg(feature = "cuda")]
use std::collections::HashMap;
#[cfg(feature = "cuda")]
use std::fs;
use style::stylize;
#[cfg(feature = "cuda")]
use style::stylize_config;
#[cfg(feature = "cuda")]
use style::stylize_title;

#[cfg(feature = "cuda")]
use cli_stressor_rs::parse_int_list;

#[cfg(feature = "cuda")]
use cli_stressor_rs::{
    Backend, DeviceInfo, KernelParamOverride, KernelType, PrecisionKind, PrecisionMixtureEntry,
    PrecisionSpec, StressResult, StressRunConfig, VerdictClass, VerifyConfig, classify,
    parse_kernel_mixture, parse_kernel_param_overrides, parse_kernel_type, parse_kernel_type_list,
    parse_precision_list, parse_stream_mode, run_stress_mixed, validate_intalu_precision_overrides,
};
#[cfg(feature = "cuda")]
use serde::Deserialize;

pub mod style;

#[cfg(feature = "cuda")]
use cli_stressor_rs::cuda_backend;

#[cfg(feature = "cuda")]
use cuda_backend::{
    enumerate_cuda_devices, resolve_device_index_by_pci_bus, resolve_device_index_by_sorted_index,
    resolve_device_index_by_uuid,
};

// Stressor Vulkan engine (optional). Only compiled when the crate is built with
// --features "vulkan" in addition to "cuda".
#[cfg(feature = "vulkan")]
use cli_stressor_rs::vulkan_gfx_stressor::{
    VulkanDeviceSelection, VulkanGraphicsEngine, VulkanRenderConfig,
};

// Stressor video engine (optional): NVENC→NVDEC closed loop over the
// driver-shipped libraries, only compiled with --features video.
#[cfg(feature = "video")]
use cli_stressor_rs::{VideoConfig, VideoEngine, VideoStats};

#[cfg(feature = "video")]
fn build_video_config(
    args: &Args,
    device_index: u32,
    device_is_pci_sorted: bool,
) -> Result<VideoConfig, String> {
    let preset = VideoConfig::parse_preset(&args.video_preset)?;
    Ok(VideoConfig {
        device_index,
        device_is_pci_sorted,
        width: args.video_width,
        height: args.video_height,
        validate: args.video_validate,
        bitrate_bps: args.video_bitrate,
        preset,
        fps: 60,
    })
}

/// Video-only mode: build the pipeline, run it for the duration, print the
/// summary and (optionally) persist a verdict. Returns the process exit code:
/// 0 clean, 1 unavailable/failed, and CRC mismatches fail the run.
#[cfg(feature = "video")]
fn run_video_for_duration(duration_s: f64, config: VideoConfig, json_out: Option<&String>) -> i32 {
    println!(
        "{}",
        stylize(
            &format!("Video-only mode: running NVENC->NVDEC engine for {duration_s:.1}s"),
            false
        )
    );
    let mut engine = match VideoEngine::new(config) {
        Ok(engine) => engine,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("[Video] video engine unavailable: {err}"), true)
            );
            return 1;
        }
    };
    let stats: VideoStats = match engine.run_for_duration(duration_s) {
        Ok(stats) => stats,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("[FATAL] Video engine failed: {err}"), true)
            );
            return 1;
        }
    };
    println!(
        "{}",
        stylize(
            &format!(
                "[Video] summary: encoded={} decoded={} crc_mismatches={} encode={:.1} Mbps decode={:.1} fps elapsed={:.1}s",
                stats.encoded_frames,
                stats.decoded_frames,
                stats.crc_mismatches,
                stats.encode_mbps,
                stats.decode_fps,
                stats.elapsed_s
            ),
            false
        )
    );
    let passed = stats.crc_mismatches == 0;
    let verdict = serde_json::json!({
        "stressor": "cli-stressor-rs",
        "result": if passed { "pass" } else { "fail" },
        "classification": if passed { "none" } else { "data_error" },
        "duration_s": duration_s,
        "video": stats,
    });
    println!("{}", stylize(&format!("VERDICT_JSON: {verdict}"), false));
    if let Some(path) = json_out {
        match serde_json::to_string_pretty(&verdict) {
            Ok(pretty) => {
                if let Err(err) = std::fs::write(path, pretty + "\n") {
                    eprintln!(
                        "{}",
                        stylize(&format!("Failed to write --json-out: {err}"), true)
                    );
                }
            }
            Err(err) => {
                eprintln!(
                    "{}",
                    stylize(&format!("Failed to serialize verdict JSON: {err}"), true)
                );
            }
        }
    }
    if passed { 0 } else { 1 }
}

#[cfg(feature = "vulkan")]
fn run_vulkan_for_duration(
    duration_s: f64,
    selection: VulkanDeviceSelection,
    render_config: VulkanRenderConfig,
) -> i32 {
    println!(
        "{}",
        stylize(
            &format!(
                "Vulkan-only mode: running Vulkan engine for {:.1}s",
                duration_s
            ),
            false
        )
    );

    let mut eng = VulkanGraphicsEngine::with_selection(selection, render_config);
    if let Err(e) = eng.start_stress_thread() {
        eprintln!(
            "{}",
            stylize(
                &format!("Failed to start VulkanGraphicsEngine: {}", e),
                true
            )
        );
        return 1;
    }

    let err_flag = eng.get_error_flag_arc();
    let started = std::time::Instant::now();
    while started.elapsed().as_secs_f64() < duration_s {
        if err_flag.load(std::sync::atomic::Ordering::SeqCst) {
            eprintln!(
                "{}",
                stylize("[FATAL] Vulkan engine reported an error; exiting", true)
            );
            let _ = eng.stop();
            return 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    if let Err(e) = eng.stop() {
        eprintln!(
            "{}",
            stylize(&format!("[FATAL] Vulkan engine stop failed: {}", e), true)
        );
        return 1;
    }
    println!("{}", stylize("Vulkan-only run finished.", false));
    0
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
enum PulseModeArg {
    #[value(name = "off")]
    Off,
    #[value(name = "fixed")]
    Fixed,
    #[value(name = "sweep")]
    Sweep,
}

#[cfg(feature = "vulkan")]
impl From<PulseModeArg> for cli_stressor_rs::PulseMode {
    fn from(mode: PulseModeArg) -> Self {
        match mode {
            PulseModeArg::Off => cli_stressor_rs::PulseMode::Off,
            PulseModeArg::Fixed => cli_stressor_rs::PulseMode::Fixed,
            PulseModeArg::Sweep => cli_stressor_rs::PulseMode::Sweep,
        }
    }
}

#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
enum MemModeArg {
    /// Resident pattern block sized from the reported L2 (0.75×L2, 8–128 MiB)
    #[value(name = "auto")]
    Auto,
    /// Explicit spelling of auto
    #[value(name = "resident")]
    Resident,
    /// Enable the address-walk slab: pattern windows sweep VRAM beyond L2
    #[value(name = "stream")]
    Stream,
}

#[cfg(feature = "cuda")]
impl From<MemModeArg> for cli_stressor_rs::MemMode {
    fn from(mode: MemModeArg) -> Self {
        match mode {
            MemModeArg::Auto => cli_stressor_rs::MemMode::Auto,
            MemModeArg::Resident => cli_stressor_rs::MemMode::Resident,
            MemModeArg::Stream => cli_stressor_rs::MemMode::Stream,
        }
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "cli-stressor-rs",
    about = "GPU core-domain stressor (Rust): mixed kernel-path stress + validation sidecar"
)]
struct Args {
    #[arg(
        long = "nvoc-stressor-cuda-rs-worker",
        hide = true,
        default_value_t = false
    )]
    _nvoc_stressor_worker: bool,

    #[arg(
        long,
        help = "Optional TOML config file. Supports [kernel_params.<kernel>] for per-kernel precisions/precision_mixture/matrix_sizes/warmup_iters/burst_iters/transpose_prob/minor_mixture_rate"
    )]
    config: Option<String>,

    #[arg(
        long,
        value_parser = ["standard", "low-vram", "40-50", "dynamic-export"],
        conflicts_with = "config",
        help = "Use an embedded NVOC stress profile"
    )]
    profile: Option<String>,

    #[arg(long, default_value_t = 90.0)]
    duration: f64,

    #[arg(long, default_value = "2049,4096,4097,8192,8193,16384")]
    matrix_sizes: String,

    #[arg(long, default_value = "2048,4096")]
    fp64_matrix_sizes: String,

    #[arg(
        long,
        default_value = "fp16,bf16",
        help = "Comma-separated precision list: fp64,fp32,tf32,fp16,bf16,fp8,int8,int16,int32 (aliases i8/i16/i32 accepted)"
    )]
    precisions: String,

    #[arg(long, default_value_t = 3)]
    warmup_iters: u32,

    #[arg(long, default_value_t = 6)]
    burst_iters: u32,

    #[arg(
        long,
        default_value_t = 5.0,
        help = "Periodic validation interval in seconds; set to 0 to disable validation"
    )]
    validate_interval: f64,

    #[arg(long, default_value_t = 1024)]
    validate_size: usize,

    /// Output edge length up to which the full-coverage GEMM recompute runs
    /// (100% of elements; larger sizes use sampled cross-checks)
    #[arg(long, default_value_t = 1024)]
    gemm_full_check_max_size: usize,

    /// Keep running after a detector fault, accumulating SDC statistics
    /// (for error-rate vs frequency/temperature mapping)
    #[arg(long, default_value_t = false)]
    verify_continue_on_error: bool,

    /// Seconds between resident-block pattern checks (0 = off)
    #[arg(long, default_value_t = 2.0)]
    verify_resident_interval: f64,

    /// Disable the ride-on-load detectors (pattern compare / GEMM scan /
    /// sampled cross-checks / IntAlu reference)
    #[arg(long, default_value_t = false)]
    no_verify: bool,

    /// Skip the detector injection self-test gate
    #[arg(long, default_value_t = false)]
    skip_self_test: bool,

    /// Write the SDC event stream (fault records + telemetry cadence rows)
    /// as JSONL to this path. Requires NVML for the telemetry context.
    #[arg(long, value_name = "PATH")]
    sdc_log: Option<std::path::PathBuf>,

    /// Write the machine-readable verdict JSON to this path (also printed as
    /// a `VERDICT_JSON:` line on stdout)
    #[arg(long)]
    json_out: Option<String>,

    #[arg(long, default_value_t = 0.5)]
    transpose_prob: f64,

    #[arg(long, default_value_t = 0.15)]
    minor_mixture_rate: f64,

    #[arg(long, default_value_t = 12345)]
    seed: u64,

    #[arg(
        long,
        default_value = "gemm,memcpy,memset,transpose,elementwise,reduction,atomic,intalu",
        help = "Enabled kernel paths (comma-separated): gemm,memcpy,memset,transpose,elementwise,reduction,atomic,intalu"
    )]
    kernel_types: String,

    #[arg(
        long,
        default_value = "",
        help = "Kernel mixture weights as type:weight pairs, e.g. gemm:0.5,memcpy:0.3,reduction:0.2 (empty = equal weights)"
    )]
    kernel_mixture: String,

    #[arg(
        long,
        default_value = "",
        help = "Per-kernel overrides, e.g. 'gemm:precisions=fp16|bf16,precision_mixture=fp16:0.7|bf16:0.3,matrix_sizes=2049|4096,warmup=4,burst=8;memcpy:matrix_sizes=8192|16384,burst=64'"
    )]
    kernel_params: String,

    #[arg(
        long,
        default_value = "single",
        help = "Submission stream mode: single|dual|triple"
    )]
    stream_mode: String,

    #[arg(long)]
    disable_fp8: bool,
    /// Run Vulkan-only stress (skip CUDA workload)
    #[arg(long, default_value_t = false)]
    vulkan_only: bool,

    /// Vulkan render stressor (FurMark-style graphics load; the primary
    /// graphics stress). Renders to a headless offscreen target unless
    /// --vulkan-window is passed.
    #[arg(long, default_value_t = false, alias = "vulkan-heavy")]
    vulkan: bool,

    /// Generate stress buffers on the GPU (NVRTC fill kernels) instead of
    /// host-side parallel RNG + H2D copy; cuts the non-compute gap between
    /// bursts (default off)
    #[arg(long, default_value_t = false)]
    gpu_generate: bool,

    /// Render width (offscreen target or window)
    #[arg(long, default_value_t = 1280, alias = "vulkan-heavy-width")]
    vulkan_width: u32,

    /// Render height
    #[arg(long, default_value_t = 720, alias = "vulkan-heavy-height")]
    vulkan_height: u32,

    /// MSAA sample count (1 = off, 2/4/8)
    #[arg(long, default_value_t = 1, alias = "vulkan-heavy-msaa")]
    vulkan_msaa: u32,

    /// Fragment MUFU/FMA loop iterations per pixel
    #[arg(long, default_value_t = 128, alias = "vulkan-heavy-iters")]
    vulkan_iters: u32,

    /// Instanced shell count (layered overdraw)
    #[arg(long, default_value_t = 16, alias = "vulkan-heavy-shells")]
    vulkan_shells: u32,

    /// Show a window and present the swapchain (default: headless offscreen)
    #[arg(long, default_value_t = false)]
    vulkan_window: bool,

    /// Animate torus rotation (dynamic tiles/Z/interp inputs; disable for
    /// the static-mesh A/B baseline)
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set, alias = "vulkan-heavy-rotate")]
    vulkan_rotate: bool,

    /// Compute->graphics particle pool size (0 = off)
    #[arg(long, default_value_t = 262144, alias = "vulkan-heavy-particles")]
    vulkan_particles: u32,

    /// Deprecated no-op: offscreen rendering is now the default; pass
    /// --vulkan-window for a window.
    #[arg(long, hide = true)]
    vulkan_heavy_offscreen: bool,

    /// Enable the address-walk slab (VRAM pool whose windows slide across
    /// physical pages; NVOC_VERIFY_SLAB_PCT overrides the share, default
    /// 25%). Without it the verify loop still runs pattern rotation and the
    /// write-density cadence on dedicated buffers.
    #[arg(long, default_value_t = false)]
    verify_slab: bool,

    /// Memory-pressure mode for the ride-on-load verify engine: `auto` sizes
    /// the resident pattern block from the reported L2 cache (0.75×L2,
    /// 8–128 MiB), `resident` is the explicit spelling, `stream` enables the
    /// address-walk slab so checks sweep VRAM beyond L2 (MODS VkStressFb's
    /// big-L2 / small-L2 split).
    #[arg(long, value_enum, default_value_t = MemModeArg::Auto)]
    mem_mode: MemModeArg,

    /// Drop the Vulkan render queue to LOW global priority
    /// (VK_EXT_global_priority) so background/in-process work wins when
    /// scheduling is contended.
    #[arg(long, default_value_t = false)]
    vulkan_queue_priority_low: bool,

    /// Duty-cycle power pulsing for the Vulkan renderer (MODS VkStressPulse
    /// borrow): `fixed` holds the low frequency, `sweep` climbs a log-octave
    /// ladder toward the ceiling.
    #[arg(long, value_enum, default_value_t = PulseModeArg::Off)]
    pulse_mode: PulseModeArg,

    /// Percent of each pulse period spent bursting (0-100)
    #[arg(long, default_value_t = 30.0)]
    pulse_duty: f32,

    /// Pulse frequency floor in Hz (50 Hz = the RM power-capping period)
    #[arg(long, default_value_t = 50.0)]
    pulse_low_hz: f32,

    /// Sweep ceiling in Hz (frames longer than the period saturate duty)
    #[arg(long, default_value_t = 1000.0)]
    pulse_high_hz: f32,

    /// Log2-octave climb rate for `--pulse-mode sweep`
    #[arg(long, default_value_t = 3.0)]
    pulse_octaves: f32,

    /// Spin-wait through pulse idle windows (reaches the kHz range, burns a
    /// core; default is a sleep-then-spin hybrid)
    #[arg(long, default_value_t = false)]
    pulse_spin_idle: bool,

    /// Run the NVENC→NVDEC video-engine loop alongside the CUDA load
    /// (synthetic vhash32 NV12 frames, H264 all-I encode, hardware decode)
    #[arg(long, default_value_t = false)]
    video: bool,

    /// Run video-engine-only stress (skip CUDA workload; needs --features video)
    #[arg(long, default_value_t = false)]
    video_only: bool,

    /// Validate every decoded frame (CRC-stability baseline over the
    /// deterministic all-I encode; forces CONSTQP instead of CBR).
    /// Default on; `--video-validate false` disables.
    #[arg(
        long,
        default_value_t = true,
        default_missing_value = "true",
        num_args = 0..=1,
        action = clap::ArgAction::Set
    )]
    video_validate: bool,

    /// Synthetic frame width in pixels (rounded down to even)
    #[arg(long, default_value_t = 1920)]
    video_width: u32,

    /// Synthetic frame height in pixels (rounded down to even)
    #[arg(long, default_value_t = 1080)]
    video_height: u32,

    /// H264 CBR bitrate in bits/s (only used with --video-validate off;
    /// validation pins a deterministic constant-QP encode)
    #[arg(long, value_name = "BPS")]
    video_bitrate: Option<u32>,

    /// NVENC preset P1 (fastest) .. P7 (best quality)
    #[arg(
        long,
        default_value = "p3",
        value_parser = ["p1", "p2", "p3", "p4", "p5", "p6", "p7"]
    )]
    video_preset: String,

    /// CUDA GPU index in PCI-bus-sorted order (0-based)
    #[arg(
        long,
        value_name = "INDEX",
        help = "CUDA GPU index in PCI-bus-sorted order (0-based, default: 0)"
    )]
    gpu_index: Option<u32>,

    /// CUDA GPU PCI bus address (e.g., 0001:01:00.0)
    #[arg(
        long,
        value_name = "DOMAIN:BUS:DEVICE.FUNCTION",
        help = "CUDA GPU PCI bus address (e.g., 0001:01:00.0)"
    )]
    pci_bus: Option<String>,

    /// CUDA GPU UUID (128-bit hex string)
    #[arg(
        long,
        value_name = "UUID",
        help = "CUDA GPU UUID (32 hex digits or space/dash separated)"
    )]
    gpu_uuid: Option<String>,

    /// List CUDA GPUs (PCI-sorted index, CUDA index, PCI bus, UUID) and exit
    #[arg(long, default_value_t = false)]
    list_gpus: bool,
}

#[cfg(feature = "cuda")]
#[derive(Debug, Default, Deserialize)]
struct FileVerifyConfig {
    enabled: Option<bool>,
    self_test: Option<bool>,
    full_check_max_size: Option<usize>,
    continue_on_error: Option<bool>,
    resident_interval_s: Option<f64>,
    memcpy_every: Option<u32>,
    memset_every: Option<u32>,
    gemm_every: Option<u32>,
    gemm_samples: Option<u32>,
    intalu_samples: Option<u32>,
    int8_validate_size: Option<usize>,
}

/// Whether the Vulkan render stressor should run.
#[cfg(feature = "vulkan")]
fn vulkan_stress_enabled(args: &Args) -> bool {
    args.vulkan
}

/// Assemble the render config from the `--vulkan-*` CLI flags.
#[cfg(feature = "vulkan")]
fn build_vulkan_render_config(args: &Args) -> VulkanRenderConfig {
    VulkanRenderConfig {
        width: args.vulkan_width,
        height: args.vulkan_height,
        msaa: args.vulkan_msaa,
        iters: args.vulkan_iters,
        shells: args.vulkan_shells,
        offscreen: !args.vulkan_window,
        rotate: args.vulkan_rotate,
        particles: args.vulkan_particles,
        queue_priority_low: args.vulkan_queue_priority_low,
        pulse: cli_stressor_rs::PulseConfig {
            mode: args.pulse_mode.into(),
            duty_pct: args.pulse_duty,
            low_hz: args.pulse_low_hz,
            high_hz: args.pulse_high_hz,
            octaves_per_second: args.pulse_octaves,
            spin_idle: args.pulse_spin_idle,
        },
    }
}

/// Pure-Vulkan device selection from the `--gpu-uuid`/`--pci-bus`/
/// `--gpu-index` CLI hints; no CUDA context involved. Resolution precedence
/// (in `VulkanDeviceSelection::resolve`): UUID > PCI bus > sorted index,
/// first enumerated device when no hint is given.
#[cfg(feature = "vulkan")]
fn build_vulkan_device_selection(args: &Args) -> Result<VulkanDeviceSelection, String> {
    let uuid = match &args.gpu_uuid {
        Some(raw) => parse_uuid_string(raw)?,
        None => [0u8; 16],
    };
    let pci_bus = match &args.pci_bus {
        Some(raw) => Some(parse_pci_bus_string(raw)?),
        None => None,
    };
    Ok(VulkanDeviceSelection {
        uuid,
        pci_bus,
        index: args.gpu_index,
    })
}

#[cfg(feature = "cuda")]
#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    duration: Option<f64>,
    matrix_sizes: Option<Vec<usize>>,
    fp64_matrix_sizes: Option<Vec<usize>>,
    precisions: Option<Vec<String>>,
    warmup_iters: Option<u32>,
    burst_iters: Option<u32>,
    validate_interval: Option<f64>,
    validate_size: Option<usize>,
    gemm_full_check_max_size: Option<usize>,
    transpose_prob: Option<f64>,
    minor_mixture_rate: Option<f64>,
    seed: Option<u64>,
    kernel_types: Option<Vec<String>>,
    kernel_mixture: Option<KernelMixtureConfig>,
    stream_mode: Option<String>,
    disable_fp8: Option<bool>,
    vulkan: Option<bool>,
    #[serde(alias = "vulkan-only")]
    vulkan_only: Option<bool>,
    vulkan_window: Option<bool>,
    vulkan_width: Option<u32>,
    vulkan_height: Option<u32>,
    vulkan_msaa: Option<u32>,
    vulkan_iters: Option<u32>,
    vulkan_shells: Option<u32>,
    vulkan_rotate: Option<bool>,
    vulkan_particles: Option<u32>,
    kernel_params: Option<HashMap<String, FileKernelParam>>,
    verify: Option<FileVerifyConfig>,
    /// Generate stress buffers on the GPU via NVRTC fill kernels (default
    /// off; host generation remains the conservative path).
    gpu_generate: Option<bool>,
    gpu_index: Option<u32>,
    pci_bus: Option<String>,
    gpu_uuid: Option<String>,
    list_gpus: Option<bool>,
}

#[cfg(feature = "cuda")]
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum KernelMixtureConfig {
    Text(String),
    Map(HashMap<String, f64>),
}

#[cfg(feature = "cuda")]
#[derive(Debug, Default, Deserialize)]
struct FileKernelParam {
    precisions: Option<Vec<String>>,
    #[serde(alias = "precision_weights")]
    precision_weight: Option<Vec<f64>>,
    precision_mixture: Option<HashMap<String, f64>>,
    matrix_sizes: Option<Vec<usize>>,
    warmup_iters: Option<u32>,
    burst_iters: Option<u32>,
    transpose_prob: Option<f64>,
    minor_mixture_rate: Option<f64>,
}

#[cfg(feature = "cuda")]
fn precision_mixture_from_weights(
    precisions: &[PrecisionSpec],
    weights: &[f64],
    context: &str,
) -> Result<Vec<PrecisionMixtureEntry>, String> {
    if precisions.is_empty() {
        return Err(format!("{context} precisions cannot be empty"));
    }
    if precisions.len() != weights.len() {
        return Err(format!(
            "{context} precision_weight length ({}) must match precisions ({})",
            weights.len(),
            precisions.len()
        ));
    }
    let mut entries = Vec::with_capacity(precisions.len());
    for (spec, weight) in precisions.iter().zip(weights.iter()) {
        if !weight.is_finite() || *weight < 0.0 {
            return Err(format!(
                "{context} precision_weight must be finite and >= 0: {weight}"
            ));
        }
        entries.push(PrecisionMixtureEntry {
            spec: *spec,
            weight: *weight,
        });
    }
    Ok(entries)
}

#[cfg(feature = "cuda")]
fn precision_mixture_from_map(
    map: &HashMap<String, f64>,
    context: &str,
) -> Result<Vec<PrecisionMixtureEntry>, String> {
    if map.is_empty() {
        return Err(format!("{context} precision_mixture cannot be empty"));
    }
    let mut entries = Vec::with_capacity(map.len());
    for (name, weight) in map {
        if !weight.is_finite() || *weight < 0.0 {
            return Err(format!(
                "{context} precision_mixture must be finite and >= 0: {weight}"
            ));
        }
        let specs = parse_precision_list(name)?;
        if specs.len() != 1 {
            return Err(format!(
                "{context} expected a single precision, got: {name}"
            ));
        }
        let spec = specs[0];
        entries.push(PrecisionMixtureEntry {
            spec,
            weight: *weight,
        });
    }
    Ok(entries)
}

#[cfg(feature = "cuda")]
fn load_file_config(path: &str) -> Result<FileConfig, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("failed to read config: {e}"))?;
    toml::from_str(&raw).map_err(|e| format!("invalid TOML: {e}"))
}

#[cfg(feature = "cuda")]
fn load_builtin_profile(name: &str) -> Result<FileConfig, String> {
    // include_str! places these TOML files in the executable at compile time.
    // Bundled workers therefore do not need a profiles directory at runtime.
    let raw = match name {
        "standard" => include_str!("../profiles/standard.toml"),
        "low-vram" => include_str!("../profiles/low-vram.toml"),
        "40-50" => include_str!("../profiles/40-50.toml"),
        "dynamic-export" => include_str!("../profiles/dynamic-export.toml"),
        other => return Err(format!("unknown embedded profile: {other}")),
    };
    toml::from_str(raw).map_err(|e| format!("invalid embedded profile {name}: {e}"))
}

#[cfg(feature = "cuda")]
fn load_kernel_overrides_from_config(
    parsed: &FileConfig,
) -> Result<Vec<KernelParamOverride>, String> {
    let mut out = Vec::new();
    if let Some(kernel_params) = &parsed.kernel_params {
        for (name, item) in kernel_params {
            let kind = parse_kernel_type(name)?;
            if let Some(sizes) = &item.matrix_sizes
                && sizes.is_empty()
            {
                return Err(format!("kernel_params.{name}.matrix_sizes cannot be empty"));
            }
            if item.precision_mixture.is_some() && item.precision_weight.is_some() {
                return Err(format!(
                    "kernel_params.{name} cannot set both precision_mixture and precision_weight"
                ));
            }
            let precisions = if let Some(list) = &item.precisions {
                if list.is_empty() {
                    None
                } else {
                    let joined = list.join(",");
                    Some(parse_precision_list(&joined)?)
                }
            } else {
                None
            };
            let precision_mixture = if let Some(map) = &item.precision_mixture {
                Some(precision_mixture_from_map(
                    map,
                    &format!("kernel_params.{name}.precision_mixture"),
                )?)
            } else if let Some(weights) = &item.precision_weight {
                let specs = precisions.as_ref().ok_or_else(|| {
                    format!("kernel_params.{name}.precision_weight requires precisions")
                })?;
                Some(precision_mixture_from_weights(
                    specs,
                    weights,
                    &format!("kernel_params.{name}.precision_weight"),
                )?)
            } else {
                None
            };
            out.push(KernelParamOverride {
                kind,
                precisions,
                precision_mixture,
                matrix_sizes: item.matrix_sizes.clone(),
                warmup_iters: item.warmup_iters,
                burst_iters: item.burst_iters,
                transpose_prob: item.transpose_prob,
                minor_mixture_rate: item.minor_mixture_rate,
            });
        }
    }
    Ok(out)
}

#[cfg(feature = "cuda")]
fn merge_kernel_overrides(
    base: Vec<KernelParamOverride>,
    cli: Vec<KernelParamOverride>,
) -> Vec<KernelParamOverride> {
    let mut merged = base;
    for item in cli {
        if let Some(idx) = merged.iter().position(|v| v.kind == item.kind) {
            if item.precisions.is_some() {
                merged[idx].precisions = item.precisions;
            }
            if item.precision_mixture.is_some() {
                merged[idx].precision_mixture = item.precision_mixture;
            }
            if item.matrix_sizes.is_some() {
                merged[idx].matrix_sizes = item.matrix_sizes;
            }
            if item.warmup_iters.is_some() {
                merged[idx].warmup_iters = item.warmup_iters;
            }
            if item.burst_iters.is_some() {
                merged[idx].burst_iters = item.burst_iters;
            }
            if item.transpose_prob.is_some() {
                merged[idx].transpose_prob = item.transpose_prob;
            }
            if item.minor_mixture_rate.is_some() {
                merged[idx].minor_mixture_rate = item.minor_mixture_rate;
            }
        } else {
            merged.push(item);
        }
    }
    merged
}

#[cfg(feature = "cuda")]
fn parse_args_with_cli_sources() -> (Args, std::collections::HashSet<&'static str>) {
    let cmd = Args::command();
    let matches = cmd.get_matches();
    let args = Args::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let mut cli_set = std::collections::HashSet::new();
    for id in [
        "config",
        "duration",
        "matrix_sizes",
        "fp64_matrix_sizes",
        "precisions",
        "warmup_iters",
        "burst_iters",
        "validate_interval",
        "validate_size",
        "gemm_full_check_max_size",
        "verify_continue_on_error",
        "verify_resident_interval",
        "transpose_prob",
        "minor_mixture_rate",
        "seed",
        "kernel_types",
        "kernel_mixture",
        "kernel_params",
        "vulkan",
        "gpu_generate",
        "vulkan_only",
        "vulkan_window",
        "vulkan_width",
        "vulkan_height",
        "vulkan_msaa",
        "vulkan_iters",
        "vulkan_shells",
        "vulkan_rotate",
        "vulkan_particles",
        "stream_mode",
        "disable_fp8",
        "gpu_index",
        "pci_bus",
        "gpu_uuid",
        "list_gpus",
    ] {
        if matches.value_source(id) == Some(ValueSource::CommandLine) {
            cli_set.insert(id);
        }
    }
    (args, cli_set)
}

#[cfg(feature = "cuda")]
fn apply_file_config_to_args(
    args: &mut Args,
    cli_set: &std::collections::HashSet<&'static str>,
    parsed: Option<&FileConfig>,
) -> Result<(), String> {
    let Some(parsed) = parsed else {
        return Ok(());
    };

    if let (true, Some(v)) = (!cli_set.contains("duration"), parsed.duration) {
        args.duration = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("matrix_sizes"), &parsed.matrix_sizes) {
        args.matrix_sizes = v
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(",");
    }
    if let (true, Some(v)) = (
        !cli_set.contains("fp64_matrix_sizes"),
        &parsed.fp64_matrix_sizes,
    ) {
        args.fp64_matrix_sizes = v
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(",");
    }
    if let (true, Some(v)) = (!cli_set.contains("precisions"), &parsed.precisions) {
        args.precisions = v.join(",");
    }
    if let (true, Some(v)) = (!cli_set.contains("warmup_iters"), parsed.warmup_iters) {
        args.warmup_iters = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("burst_iters"), parsed.burst_iters) {
        args.burst_iters = v;
    }
    if let (true, Some(v)) = (
        !cli_set.contains("validate_interval"),
        parsed.validate_interval,
    ) {
        args.validate_interval = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("validate_size"), parsed.validate_size) {
        args.validate_size = v;
    }
    if let (true, Some(v)) = (
        !cli_set.contains("gemm_full_check_max_size"),
        parsed.gemm_full_check_max_size,
    ) {
        args.gemm_full_check_max_size = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("transpose_prob"), parsed.transpose_prob) {
        args.transpose_prob = v;
    }
    if let (true, Some(v)) = (
        !cli_set.contains("minor_mixture_rate"),
        parsed.minor_mixture_rate,
    ) {
        args.minor_mixture_rate = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("seed"), parsed.seed) {
        args.seed = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("kernel_types"), &parsed.kernel_types) {
        args.kernel_types = v.join(",");
    }
    if let (true, Some(v)) = (!cli_set.contains("kernel_mixture"), &parsed.kernel_mixture) {
        args.kernel_mixture = match v {
            KernelMixtureConfig::Text(s) => s.clone(),
            KernelMixtureConfig::Map(m) => {
                let mut parts = Vec::with_capacity(m.len());
                for (k, w) in m {
                    parts.push(format!("{k}:{w}"));
                }
                parts.join(",")
            }
        };
    }
    if let (true, Some(v)) = (!cli_set.contains("stream_mode"), &parsed.stream_mode) {
        args.stream_mode = v.clone();
    }
    if let (true, Some(v)) = (!cli_set.contains("gpu_generate"), parsed.gpu_generate) {
        args.gpu_generate = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("disable_fp8"), parsed.disable_fp8) {
        args.disable_fp8 = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("vulkan"), parsed.vulkan) {
        args.vulkan = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("vulkan_window"), parsed.vulkan_window) {
        args.vulkan_window = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("vulkan_width"), parsed.vulkan_width) {
        args.vulkan_width = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("vulkan_height"), parsed.vulkan_height) {
        args.vulkan_height = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("vulkan_msaa"), parsed.vulkan_msaa) {
        args.vulkan_msaa = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("vulkan_iters"), parsed.vulkan_iters) {
        args.vulkan_iters = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("vulkan_shells"), parsed.vulkan_shells) {
        args.vulkan_shells = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("vulkan_rotate"), parsed.vulkan_rotate) {
        args.vulkan_rotate = v;
    }
    if let (true, Some(v)) = (
        !cli_set.contains("vulkan_particles"),
        parsed.vulkan_particles,
    ) {
        args.vulkan_particles = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("vulkan_only"), parsed.vulkan_only) {
        args.vulkan_only = v;
    }
    if let (true, Some(v)) = (!cli_set.contains("gpu_index"), parsed.gpu_index) {
        args.gpu_index = Some(v);
    }
    if let (true, Some(v)) = (!cli_set.contains("pci_bus"), &parsed.pci_bus) {
        args.pci_bus = Some(v.clone());
    }
    if let (true, Some(v)) = (!cli_set.contains("gpu_uuid"), &parsed.gpu_uuid) {
        args.gpu_uuid = Some(v.clone());
    }
    if let (true, Some(v)) = (!cli_set.contains("list_gpus"), parsed.list_gpus) {
        args.list_gpus = v;
    }
    Ok(())
}

/// Print the machine-readable verdict as a greppable one-liner and optionally
/// persist it to `--json-out`. The optimizer's output forwarding strips ANSI,
/// so the `VERDICT_JSON:` prefix survives end-to-end.
#[cfg(feature = "cuda")]
fn emit_verdict(verdict: serde_json::Value, json_out: &Option<String>) {
    println!("{}", stylize(&format!("VERDICT_JSON: {verdict}"), false));
    if let Some(path) = json_out {
        match serde_json::to_string_pretty(&verdict) {
            Ok(pretty) => {
                if let Err(err) = fs::write(path, pretty + "\n") {
                    eprintln!(
                        "{}",
                        stylize(&format!("Failed to write --json-out: {err}"), true)
                    );
                }
            }
            Err(err) => {
                eprintln!(
                    "{}",
                    stylize(&format!("Failed to serialize verdict JSON: {err}"), true)
                );
            }
        }
    }
}

#[cfg(feature = "cuda")]
fn resolve_gpu_device_index(args: &Args) -> Result<u32, String> {
    // Count how many selection methods are provided
    let has_gpu_index = args.gpu_index.is_some();
    let has_pci_bus = args.pci_bus.is_some();
    let has_gpu_uuid = args.gpu_uuid.is_some();

    let count = [has_gpu_index, has_pci_bus, has_gpu_uuid]
        .iter()
        .filter(|x| **x)
        .count();

    if count > 1 {
        return Err(
            "only one of --gpu-index, --pci-bus, or --gpu-uuid can be specified".to_string(),
        );
    }

    if let Some(index) = args.gpu_index {
        // gpu-index is interpreted as index in PCI-bus-sorted CUDA device list.
        println!(
            "{}",
            stylize(
                &format!("[GPU] Using PCI-sorted CUDA index: {}", index),
                false
            )
        );
        return resolve_device_index_by_sorted_index(index);
    }

    if let Some(ref pci_str) = args.pci_bus {
        println!(
            "{}",
            stylize(&format!("[GPU] Using PCI bus ID: {}", pci_str), false)
        );
        let pci = parse_pci_bus_string(pci_str)?;
        return resolve_device_index_by_pci_bus(pci);
    }

    if let Some(ref uuid_str) = args.gpu_uuid {
        println!(
            "{}",
            stylize(&format!("[GPU] Using UUID: {}", uuid_str), false)
        );
        let uuid = parse_uuid_string(uuid_str)?;
        return resolve_device_index_by_uuid(uuid);
    }

    // Default: use index 0 in PCI-sorted order.
    println!(
        "{}",
        stylize("[GPU] Using default PCI-sorted CUDA index: 0", false)
    );
    resolve_device_index_by_sorted_index(0)
}

#[cfg(any(feature = "cuda", feature = "vulkan"))]
fn parse_pci_bus_string(s: &str) -> Result<cli_stressor_rs::PciBusAddress, String> {
    let s = s.trim();
    let (domain_raw, rest) = s
        .split_once(':')
        .ok_or_else(|| format!("invalid PCI bus format: {s}"))?;
    let (bus_raw, rest) = rest
        .split_once(':')
        .ok_or_else(|| format!("invalid PCI bus format: {s}"))?;
    let (device_raw, function_raw) = rest
        .split_once('.')
        .ok_or_else(|| format!("invalid PCI bus format: {s}"))?;

    let domain = u32::from_str_radix(domain_raw, 16)
        .map_err(|_| format!("invalid PCI domain: {domain_raw}"))?;
    let bus =
        u32::from_str_radix(bus_raw, 16).map_err(|_| format!("invalid PCI bus: {bus_raw}"))?;
    let device = u32::from_str_radix(device_raw, 16)
        .map_err(|_| format!("invalid PCI device: {device_raw}"))?;
    let function = u32::from_str_radix(function_raw, 16)
        .map_err(|_| format!("invalid PCI function: {function_raw}"))?;

    Ok(cli_stressor_rs::PciBusAddress {
        domain,
        bus,
        device,
        function,
    })
}

#[cfg(any(feature = "cuda", feature = "vulkan"))]
fn parse_uuid_string(s: &str) -> Result<[u8; 16], String> {
    let s = s.trim();

    // Remove spaces and dashes, then parse as continuous hex pairs
    let hex_str: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();

    if hex_str.len() != 32 {
        return Err(format!("UUID must be 32 hex digits, got {}", hex_str.len()));
    }

    let mut uuid = [0u8; 16];
    for i in 0..16 {
        let hex_pair = &hex_str[i * 2..(i + 1) * 2];
        uuid[i] = u8::from_str_radix(hex_pair, 16)
            .map_err(|_| format!("invalid UUID hex pair: {hex_pair}"))?;
    }

    Ok(uuid)
}

#[cfg(feature = "cuda")]
fn format_uuid_hex(uuid: &[u8; 16]) -> String {
    uuid.iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(feature = "cuda")]
fn print_cuda_gpu_list() -> Result<(), String> {
    let mut devices =
        enumerate_cuda_devices().map_err(|e| format!("failed to enumerate devices: {e}"))?;
    devices.sort_by(|a, b| match (a.pci_bus, b.pci_bus) {
        (Some(pa), Some(pb)) => (pa.domain, pa.bus, pa.device, pa.function).cmp(&(
            pb.domain,
            pb.bus,
            pb.device,
            pb.function,
        )),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.device_index.cmp(&b.device_index),
    });

    println!("{}", stylize_title("CUDA GPUs (sorted by PCI bus):"));
    println!(
        "{}",
        stylize(
            &format!(
                "{:>5} {:>6} {:<14} {:<32} {}",
                "s_idx", "cuda", "pci_bus", "uuid", "name"
            ),
            false
        )
    );
    for (sorted_idx, dev) in devices.iter().enumerate() {
        let pci = dev
            .pci_bus
            .map(|p| {
                format!(
                    "{:04X}:{:02X}:{:02X}.{}",
                    p.domain, p.bus, p.device, p.function
                )
            })
            .unwrap_or_else(|| "<none>".to_string());
        println!(
            "{}",
            stylize(
                &format!(
                    "{:>5} {:>6} {:<14} {:<32} {}",
                    sorted_idx,
                    dev.device_index,
                    pci,
                    format_uuid_hex(&dev.uuid),
                    dev.device_name
                ),
                false
            )
        );
    }
    Ok(())
}

#[cfg(feature = "cuda")]
fn filter_atomic_for_sm(kernel_types: &mut Vec<KernelType>, info: &DeviceInfo) {
    let sm_major = info.compute_capability.map(|(major, _)| major);
    if sm_major.unwrap_or(0) >= 8 {
        return;
    }
    if kernel_types.contains(&KernelType::Atomic) {
        kernel_types.retain(|k| *k != KernelType::Atomic);
        println!(
            "{}",
            stylize(
                &format!(
                    "Atomic path disabled: current GPU is below SM80 (detected {:?})",
                    info.compute_capability
                ),
                false
            )
        );
    }
}

/// Warn (do not disable) when INT8 GEMM is requested on a GPU without INT8
/// tensor cores. cuBLAS still runs a scalar INT8 GEMM on such hardware, which
/// is valid integer-compute stress — just slower. IMMA tensor cores require
/// SM >= 7.5 (Turing); DP4A dot units require SM >= 6.1 (Pascal).
#[cfg(feature = "cuda")]
fn warn_int8_tensor_op(precisions: &[cli_stressor_rs::PrecisionSpec], info: &DeviceInfo) {
    let wants_int8 = precisions
        .iter()
        .any(|s| s.kind == cli_stressor_rs::PrecisionKind::INT8);
    if !wants_int8 {
        return;
    }
    let sm = info.compute_capability.unwrap_or((0, 0));
    let has_imma = sm.0 > 7 || (sm.0 == 7 && sm.1 >= 5);
    if !has_imma {
        println!(
            "{}",
            stylize(
                &format!(
                    "INT8 GEMM tensor cores (IMMA) require SM>=7.5 (detected SM{}.{}); \
                     cuBLAS will use a scalar/DP4A INT8 path (slower, but valid stress)",
                    sm.0, sm.1
                ),
                false
            )
        );
    }
}

#[cfg(feature = "cuda")]
fn print_device_info(info: &DeviceInfo) {
    println!(
        "{}",
        stylize_title(&format!("Testing Device: {}", info.name))
    );
    if let Some((major, minor)) = info.compute_capability {
        println!(
            "{}",
            stylize(&format!("Compute Capability: SM{}.{}", major, minor), false)
        );
    }
    if let Some(mem) = info.total_mem_gb {
        println!(
            "{}",
            stylize(&format!("Video Memory: {:.1} GB", mem), false)
        );
    }
}

#[cfg(feature = "cuda")]
fn print_summary(results: &[StressResult], info: &DeviceInfo) {
    println!("\n{}", "=".repeat(72));
    println!("{}", stylize_title("Phase 1 core stability summary"));
    println!(
        "{}",
        stylize_title(&format!("Testing Device: {}", info.name))
    );
    if let Some(mem) = info.total_mem_gb {
        println!(
            "{}",
            stylize(&format!("Video Memory: {:.1} GB", mem), false)
        );
    }

    let mut overall_ok = true;
    for r in results {
        let status = if !r.supported {
            "SKIP"
        } else if r.first_error.is_none() && r.validation_failures == 0 {
            "OK"
        } else {
            "FAIL"
        };
        if status == "FAIL" {
            overall_ok = false;
        }
        let eff = if r.elapsed_s > 0.0 {
            r.compute_s / r.elapsed_s * 100.0
        } else {
            0.0
        };
        println!(
            "{}",
            stylize(
                &format!(
                    "{:<12} {:<4} | iters={:8} | wall={:7.1}s | compute={:6.1}s | eff={:4.0}% | {:8.2} TFLOPS | val_fail={:3} | max_abs={:.3e} | max_rel={:.3e}",
                    r.precision,
                    status,
                    r.iterations,
                    r.elapsed_s,
                    r.compute_s,
                    eff,
                    r.tflops,
                    r.validation_failures,
                    r.max_abs_error,
                    r.max_rel_error
                ),
                false
            )
        );
        if let Some(err) = &r.first_error {
            println!(
                "{}",
                stylize(&format!("{:12}      first_error: {}", "", err), false)
            );
            if let Some(at) = r.first_error_at_s {
                println!(
                    "{}",
                    stylize(&format!("{:12}      at: {:.1}s", "", at), false)
                );
            }
        }
        let d = &r.detectors;
        if d.ops_checked > 0 {
            let bits = d.bit_summary();
            println!(
                "{}",
                stylize(
                    &format!(
                        "{:12}      detectors: ops={} checked={} errors={} mismatch={} nonfinite={} events={}{}",
                        "",
                        d.ops_checked,
                        d.elements_checked,
                        d.total_errors,
                        d.mismatches,
                        d.nonfinite,
                        d.fault_events,
                        if bits.is_empty() {
                            String::new()
                        } else {
                            format!(" bits=[{bits}]")
                        }
                    ),
                    false
                )
            );
        }
    }

    println!("{}", "=".repeat(72));
    println!("{}", stylize(" Result:", false));
    if overall_ok {
        println!(
            "{}",
            stylize(
                "- No obvious computation errors or validation failures were observed in the current test window.",
                false
            )
        );
    } else {
        println!(
            "{}",
            stylize(
                "- At least one precision mode reported an error or validation failure.",
                true
            )
        );
        std::process::exit(1);
    }
    println!("{}", "=".repeat(72));
}

/// Grouped root help for bare `cli-stressor-rs` (no arguments), mirroring the
/// nvoc-cli convention: engines as families with colored group headings
/// instead of clap's flat flag wall. clap still renders full `--help`.
fn render_grouped_help() -> String {
    use nvoc_cli_common::color::{stylize_group_heading, stylize_help_command};
    let mut text = String::new();
    text.push_str(
        "GPU core-domain stressor (Rust): CUDA compute + Vulkan graphics + video engines, with a validation sidecar\n\n",
    );
    text.push_str("Usage: cli-stressor-rs [OPTIONS]\n\n");

    let mut section = |key: &str, title: &str, flags: &[(&str, &str)]| {
        text.push_str(&format!(
            "  {}\n",
            stylize_group_heading(key, &format!("{title} ({} flags)", flags.len()))
        ));
        for (flag, about) in flags {
            text.push_str(&format!("    {}  {}\n", stylize_help_command(flag), about));
        }
        text.push('\n');
    };

    section(
        "cuda",
        "CUDA COMPUTE",
        &[
            ("--duration <S>", "stress wall time [default: 90]"),
            (
                "--precisions <LIST>",
                "fp32/tf32/fp16/bf16/int8/int16/int32/fp64 mix",
            ),
            (
                "--kernel-types <LIST>",
                "kernel-path mixture (gemm, memset, ...)",
            ),
            ("--stream-mode <MODE>", "single/dual/triple CUDA streams"),
            (
                "--mem-mode <MODE>",
                "verify working set: auto/resident (L2-sized) / stream (slab)",
            ),
            ("--no-verify", "disable the ride-on-load SDC detectors"),
            (
                "--verify-continue-on-error",
                "count faults instead of aborting (rate mapping)",
            ),
            (
                "--sdc-log <PATH>",
                "JSONL SDC event stream + telemetry cadence",
            ),
            ("--gpu-generate", "device-side stress-buffer generation"),
        ],
    );
    section(
        "vulkan",
        "VULKAN GRAPHICS",
        &[
            (
                "--vulkan",
                "run the offscreen render stress alongside the CUDA load",
            ),
            ("--vulkan-only", "pure Vulkan run (no CUDA required)"),
            (
                "--vulkan-window",
                "windowed presentation (Windows; default offscreen)",
            ),
            ("--vulkan-msaa <N>", "MSAA sample count"),
            (
                "--pulse-mode <MODE>",
                "power pulsing: fixed/sweep (MODS VkStressPulse)",
            ),
            (
                "--pulse-duty <PCT>",
                "burst fraction of each pulse period [default: 30]",
            ),
            (
                "--pulse-low-hz <HZ>",
                "pulse floor, 50 Hz = the RM power-capping period",
            ),
            (
                "--pulse-high-hz <HZ>",
                "sweep ceiling (frames > period saturate duty)",
            ),
            (
                "--pulse-spin-idle",
                "spin through idle windows (kHz range, burns a core)",
            ),
            (
                "--vulkan-queue-priority-low",
                "render queue at LOW global priority",
            ),
        ],
    );
    section(
        "video",
        "VIDEO ENGINES",
        &[
            (
                "--video",
                "NVENC→NVDEC encode/decode loop alongside the CUDA load",
            ),
            ("--video-only", "pure video-engine run"),
            (
                "--video-validate",
                "CRC-check every decoded frame (default on; forces CONSTQP)",
            ),
            (
                "--video-width <PX>",
                "synthetic frame width [default: 1920]",
            ),
            (
                "--video-height <PX>",
                "synthetic frame height [default: 1080]",
            ),
            (
                "--video-preset <P>",
                "NVENC preset p1..p7 (1 = fastest, 7 = best) [default: p3]",
            ),
            (
                "--video-bitrate <BPS>",
                "H264 CBR bitrate (only with --video-validate off)",
            ),
        ],
    );
    section(
        "common",
        "COMMON",
        &[
            ("--gpu-index <N>", "CUDA GPU index in PCI-bus-sorted order"),
            ("--gpu-uuid <UUID>", "select GPU by UUID"),
            ("--json-out <PATH>", "machine-readable verdict JSON"),
            ("--config <PATH>", "runtime TOML config"),
            ("--profile <NAME>", "built-in TOML profile (e.g. standard)"),
        ],
    );
    text.push_str(
        "Run `cli-stressor-rs --help` for the complete option list; GPU-mutating loads need a real device.\n",
    );
    text
}

/// True when the process was invoked with no arguments at all.
fn invoked_without_args() -> bool {
    std::env::args().count() <= 1
}

#[cfg(feature = "cuda")]
pub fn run_from_args() {
    if invoked_without_args() {
        println!("{}", render_grouped_help());
        return;
    }
    let (mut args, cli_set) = parse_args_with_cli_sources();
    // --config reads a user-supplied runtime file, whereas --profile selects
    // one of the TOML documents compiled into the binary above.
    let file_config = match (&args.config, &args.profile) {
        (Some(path), None) => match load_file_config(path) {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                eprintln!(
                    "{}",
                    stylize(&format!("Invalid config file: {}", err), true)
                );
                std::process::exit(2);
            }
        },
        (None, Some(profile)) => match load_builtin_profile(profile) {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                eprintln!("{}", stylize(&format!("Invalid profile: {}", err), true));
                std::process::exit(2);
            }
        },
        (None, None) => None,
        (Some(_), Some(_)) => unreachable!("clap rejects --config with --profile"),
    };
    if let Err(err) = apply_file_config_to_args(&mut args, &cli_set, file_config.as_ref()) {
        eprintln!(
            "{}",
            stylize(&format!("Invalid config file: {}", err), true)
        );
        std::process::exit(2);
    }

    // Ride-on-load verification settings: TOML [verify] fills the defaults,
    // CLI flags override.
    let mut verify_cfg = VerifyConfig::default();
    if let Some(parsed) = file_config.as_ref()
        && let Some(v) = &parsed.verify
    {
        if let Some(enabled) = v.enabled {
            verify_cfg.enabled = enabled;
        }
        if let Some(self_test) = v.self_test {
            verify_cfg.self_test = self_test;
        }
        if let Some(every) = v.memcpy_every {
            verify_cfg.memcpy_every = every;
        }
        if let Some(every) = v.memset_every {
            verify_cfg.memset_every = every;
        }
        if let Some(every) = v.gemm_every {
            verify_cfg.gemm_every = every;
        }
        if let Some(samples) = v.gemm_samples {
            verify_cfg.gemm_samples = samples;
        }
        if let Some(size) = v.full_check_max_size {
            verify_cfg.full_check_max_size = size;
        }
        if let Some(c) = v.continue_on_error {
            verify_cfg.continue_on_error = c;
        }
        if let Some(i) = v.resident_interval_s {
            verify_cfg.resident_interval_s = i;
        }
        if let Some(samples) = v.intalu_samples {
            verify_cfg.intalu_samples = samples;
        }
        if let Some(size) = v.int8_validate_size {
            verify_cfg.int8_validate_size = size;
        }
    }
    if args.no_verify {
        verify_cfg.enabled = false;
    }
    if args.verify_continue_on_error {
        verify_cfg.continue_on_error = true;
    }
    verify_cfg.resident_interval_s = args.verify_resident_interval;
    if args.skip_self_test {
        verify_cfg.self_test = false;
    }

    if args.list_gpus {
        match print_cuda_gpu_list() {
            Ok(()) => std::process::exit(0),
            Err(err) => {
                eprintln!(
                    "{}",
                    stylize(&format!("Failed to list CUDA GPUs: {}", err), true)
                );
                std::process::exit(1);
            }
        }
    }

    // Vulkan-only mode: pure Vulkan path, handled before any CUDA init. GPU
    // selection comes straight from the --gpu-index/--pci-bus/--gpu-uuid
    // hints, so a Vulkan-only run never requires a working CUDA driver.
    if args.vulkan_only {
        #[cfg(feature = "vulkan")]
        {
            let selection = match build_vulkan_device_selection(&args) {
                Ok(sel) => sel,
                Err(err) => {
                    eprintln!(
                        "{}",
                        stylize(&format!("Invalid Vulkan GPU selection: {}", err), true)
                    );
                    std::process::exit(2);
                }
            };
            let render_config = build_vulkan_render_config(&args);
            std::process::exit(run_vulkan_for_duration(
                args.duration,
                selection,
                render_config,
            ));
        }

        #[cfg(not(feature = "vulkan"))]
        {
            eprintln!(
                "{}",
                stylize(
                    "--vulkan-only requires building with --features vulkan",
                    true
                )
            );
            std::process::exit(2);
        }
    }

    // Video-only mode: pure NVENC→NVDEC path, handled before any CUDA init.
    // GPU selection comes from --gpu-index interpreted in PCI-bus-sorted
    // order (resolved via driver attributes, same rule as the CUDA backend).
    if args.video_only {
        #[cfg(feature = "video")]
        {
            let config = match build_video_config(&args, args.gpu_index.unwrap_or(0), true) {
                Ok(config) => config,
                Err(err) => {
                    eprintln!(
                        "{}",
                        stylize(&format!("Invalid video configuration: {err}"), true)
                    );
                    std::process::exit(2);
                }
            };
            std::process::exit(run_video_for_duration(
                args.duration,
                config,
                args.json_out.as_ref(),
            ));
        }

        #[cfg(not(feature = "video"))]
        {
            eprintln!(
                "{}",
                stylize("--video-only requires building with --features video", true)
            );
            std::process::exit(2);
        }
    }

    let matrix_sizes = match parse_int_list(&args.matrix_sizes) {
        Ok(values) => values,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("Invalid matrix sizes argument: {}", err), true)
            );
            std::process::exit(2);
        }
    };
    let fp64_matrix_sizes = match parse_int_list(&args.fp64_matrix_sizes) {
        Ok(values) => values,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(
                    &format!("Invalid fp64 matrix sizes argument: {}", err),
                    true
                )
            );
            std::process::exit(2);
        }
    };

    let gpu_device_index = match resolve_gpu_device_index(&args) {
        Ok(idx) => idx,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("Failed to resolve GPU device: {}", err), true)
            );
            std::process::exit(2);
        }
    };

    let mem_mode: cli_stressor_rs::MemMode = args.mem_mode.into();
    let mut backend =
        match cuda_backend::CudaBackend::new_with_device_and_mem_mode(gpu_device_index, mem_mode) {
            Ok(backend) => backend,
            Err(err) => {
                eprintln!(
                    "{}",
                    stylize(
                        &format!("CUDA init failed (gpu_index={}): {}", gpu_device_index, err),
                        true
                    )
                );
                std::process::exit(1);
            }
        };
    if args.gpu_generate
        && let Err(err) = backend.enable_gpu_generate()
    {
        eprintln!(
            "{}",
            stylize(
                &format!(
                    "GPU buffer generation unavailable ({}); falling back to host generation",
                    err
                ),
                true
            )
        );
    }

    // Address-walk slab is opt-in (--verify-slab): lazy allocation keeps the
    // default run's VRAM footprint lean; pattern rotation and the write
    // density cadence run on dedicated buffers either way.
    #[cfg(feature = "cuda")]
    if args.verify_slab || mem_mode == cli_stressor_rs::MemMode::Stream {
        backend.enable_verify_slab(25);
    };

    // CUDA device identity for mixed-mode Vulkan alignment (--vulkan): the
    // render sidecar must land on the same physical GPU as the compute load.
    // Vulkan-only mode never reaches this point; it selects its GPU directly
    // from the CLI hints without touching CUDA.
    #[cfg(feature = "vulkan")]
    let cuda_device_identity = match backend.device_identity() {
        Ok(identity) => Some(identity),
        Err(err) => {
            if vulkan_stress_enabled(&args) {
                eprintln!(
                    "{}",
                    stylize(
                        &format!(
                            "Failed to read CUDA device identity for Vulkan alignment immediately after CUDA init: {}",
                            err
                        ),
                        true
                    )
                );
                std::process::exit(1);
            }
            None
        }
    };

    let info = backend.device_info();
    print_device_info(&info);

    // Detector injection self-test (HYDRA heritage): prove the detection
    // pipeline catches exactly one known injected error before trusting a
    // green run. A broken detector's all-green is worse than no detector.
    let self_test = if verify_cfg.enabled && verify_cfg.self_test {
        backend.run_verify_self_test()
    } else {
        None
    };
    if let Some(report) = &self_test {
        for check in &report.checks {
            println!(
                "{}",
                stylize(
                    &format!(
                        "[self-test] {}: {} ({})",
                        check.name,
                        if check.passed { "OK" } else { "FAIL" },
                        check.detail
                    ),
                    !check.passed
                )
            );
        }
        if !report.passed {
            eprintln!(
                "{}",
                stylize(
                    "[FATAL] verify self-test FAILED — the error-detection pipeline is not \
                     trustworthy; aborting (use --skip-self-test to bypass)",
                    true
                )
            );
            let verdict = serde_json::json!({
                "stressor": "cli-stressor-rs",
                "result": "fail",
                "classification": "self_test_failed",
                "device": info.name,
                "duration_s": args.duration,
                "self_test": self_test,
                "precisions": [],
                "coverage": 0.0,
                "confidence_pct": 0,
            });
            emit_verdict(verdict, &args.json_out);
            std::process::exit(1);
        }
    }

    let precisions = match parse_precision_list(&args.precisions) {
        Ok(values) => values,
        Err(err) => {
            eprintln!("{}", stylize(&format!("Invalid argument: {}", err), true));
            std::process::exit(2);
        }
    };

    let include_fp8 = !args.disable_fp8;
    let mut filtered = Vec::new();
    for spec in precisions {
        if spec.kind == PrecisionKind::FP8E4M3FN && !include_fp8 {
            println!(
                "{}",
                stylize("FP8 E4M3FN disabled by flag, skipping", false)
            );
            continue;
        }
        filtered.push(spec);
    }
    if filtered.is_empty() {
        eprintln!("{}", stylize("No runnable precision modes available", true));
        std::process::exit(1);
    }

    let kernel_types_all = match parse_kernel_type_list(&args.kernel_types) {
        Ok(values) => values,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("Invalid kernel types argument: {}", err), true)
            );
            std::process::exit(2);
        }
    };
    let mut kernel_types = kernel_types_all.clone();
    filter_atomic_for_sm(&mut kernel_types, &info);
    warn_int8_tensor_op(&filtered, &info);

    // Additional filtering based on backend-specific runtime availability
    kernel_types.retain(|&k| backend.supports_kernel(k));

    if kernel_types.is_empty() {
        eprintln!(
            "{}",
            stylize("No runnable kernel types after capability filtering", true)
        );
        std::process::exit(1);
    }

    // Reject (kernel, precision) combos that have no hardware path before we
    // enter the dispatch loop. INT16/INT32 expose no cuBLAS GEMM (CUDA_R_16I /
    // CUDA_R_32I are storage types only); running them against `--kernel-types
    // gemm` would otherwise soft-skip every iteration and spin the full duration
    // producing zero work (which looks like a hang). INT8 GEMM is a real,
    // hardware-accelerated path and is intentionally allowed.
    let mut incompatible: Vec<&str> = Vec::new();
    for spec in &filtered {
        let has_compatible_kernel = kernel_types
            .iter()
            .any(|&k| cli_stressor_rs::kernel_precision_compatible(k, spec.kind));
        if !has_compatible_kernel {
            incompatible.push(spec.name);
        }
    }
    if !incompatible.is_empty() {
        let need = if kernel_types.iter().all(|kind| *kind == KernelType::IntAlu) {
            "INTALU supports only INT8/INT16/INT32"
        } else if incompatible.iter().any(|n| *n == "INT16" || *n == "INT32") {
            "INT16/INT32 have no GEMM path; add `intalu` to --kernel-types"
        } else {
            "no compatible kernel type for the requested precision"
        };
        eprintln!(
            "{}",
            stylize(
                &format!(
                    "Precision(s) [{}] have no compatible kernel in --kernel-types ({}). \
                     Aborting; pick a compatible --kernel-types combination.",
                    incompatible.join(", "),
                    need
                ),
                true
            )
        );
        std::process::exit(2);
    }

    let kernel_mixture_base = if args.kernel_mixture.trim().is_empty() {
        parse_kernel_mixture(&args.kernel_mixture, &kernel_types)
    } else {
        parse_kernel_mixture(&args.kernel_mixture, &kernel_types_all)
    };
    let mut kernel_mixture = match kernel_mixture_base {
        Ok(values) => values,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("Invalid kernel mixture argument: {}", err), true)
            );
            std::process::exit(2);
        }
    };
    if kernel_types != kernel_types_all {
        kernel_mixture.retain(|entry| kernel_types.contains(&entry.kind));
        if kernel_mixture.is_empty() {
            kernel_mixture = kernel_types
                .iter()
                .map(|kind| cli_stressor_rs::KernelMixtureEntry {
                    kind: *kind,
                    weight: 1.0,
                })
                .collect();
        }
    }
    let stream_mode = match parse_stream_mode(&args.stream_mode) {
        Ok(mode) => mode,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("Invalid stream mode argument: {}", err), true)
            );
            std::process::exit(2);
        }
    };
    let config_overrides = match &file_config {
        Some(parsed) => match load_kernel_overrides_from_config(parsed) {
            Ok(values) => values,
            Err(err) => {
                eprintln!(
                    "{}",
                    stylize(&format!("Invalid config file: {}", err), true)
                );
                std::process::exit(2);
            }
        },
        None => Vec::new(),
    };
    let cli_overrides = match parse_kernel_param_overrides(&args.kernel_params) {
        Ok(values) => values,
        Err(err) => {
            eprintln!(
                "{}",
                stylize(&format!("Invalid kernel params argument: {}", err), true)
            );
            std::process::exit(2);
        }
    };
    let kernel_param_overrides = merge_kernel_overrides(config_overrides, cli_overrides);
    if let Err(err) = validate_intalu_precision_overrides(&kernel_param_overrides) {
        eprintln!(
            "{}",
            stylize(
                &format!("Invalid kernel precision configuration: {err}"),
                true
            )
        );
        std::process::exit(2);
    }

    let mut overall_passed = true;

    println!("\n{}", "-".repeat(72));
    println!("{}", stylize_title("Starting mixed-kernel stress"));
    println!(
        "{}",
        stylize_config(&format!(
            "  Precisions: {:?}",
            filtered.iter().map(|spec| spec.name).collect::<Vec<_>>()
        ))
    );
    println!(
        "{}",
        stylize_config(&format!("  Duration: {:.1} s", args.duration))
    );
    // Show per-kernel warmup/burst overrides if present
    let warmup_display = if kernel_param_overrides
        .iter()
        .any(|o| o.warmup_iters.is_some())
    {
        let mut parts = vec![format!(
            "  Warmup iterations: {} (global default)",
            args.warmup_iters
        )];
        for override_item in &kernel_param_overrides {
            if let Some(w) = override_item.warmup_iters {
                parts.push(format!("    {:?}: {}", override_item.kind, w));
            }
        }
        parts.join("\n")
    } else {
        format!("  Warmup iterations: {}", args.warmup_iters)
    };
    println!("{}", stylize_config(&warmup_display));

    let burst_display = if kernel_param_overrides
        .iter()
        .any(|o| o.burst_iters.is_some())
    {
        let mut parts = vec![format!(
            "  Burst iterations: {} (global default)",
            args.burst_iters
        )];
        for override_item in &kernel_param_overrides {
            if let Some(b) = override_item.burst_iters {
                parts.push(format!("    {:?}: {}", override_item.kind, b));
            }
        }
        parts.join("\n")
    } else {
        format!("  Burst iterations: {}", args.burst_iters)
    };
    println!("{}", stylize_config(&burst_display));
    println!(
        "{}",
        stylize_config(&if args.validate_interval > 0.0 {
            format!("  Validation interval: {:.1} s", args.validate_interval)
        } else {
            "  Validation interval: disabled (set to 0)".to_string()
        })
    );
    println!(
        "{}",
        stylize_config(&format!("  Validation size: {}", args.validate_size))
    );
    if args.gpu_generate {
        println!(
            "{}",
            stylize_config("  Buffer generation: GPU (NVRTC fill kernels)")
        );
    }

    let minor_mixture_display = if kernel_param_overrides
        .iter()
        .any(|o| o.minor_mixture_rate.is_some())
    {
        let mut parts = vec![format!(
            "  Minor mixture rate: {:.2} (global default)",
            args.minor_mixture_rate
        )];
        for override_item in &kernel_param_overrides {
            if let Some(m) = override_item.minor_mixture_rate {
                parts.push(format!("    {:?}: {:.2}", override_item.kind, m));
            }
        }
        parts.join("\n")
    } else {
        format!("  Minor mixture rate: {:.2}", args.minor_mixture_rate)
    };
    println!("{}", stylize_config(&minor_mixture_display));
    println!(
        "{}",
        stylize_config(&format!("  Kernel types: {:?}", kernel_types))
    );
    println!(
        "{}",
        stylize_config(&format!("  Kernel mixture: {:?}", kernel_mixture))
    );
    println!(
        "{}",
        stylize_config(&format!(
            "  Stream mode: {:?} ({} streams)",
            stream_mode,
            stream_mode.stream_count()
        ))
    );
    if args.vulkan {
        println!(
            "{}",
            stylize_config(&format!(
                "  Vulkan render: {}x{}, {} shells, {} iters/px, {}x MSAA, {}",
                args.vulkan_width,
                args.vulkan_height,
                args.vulkan_shells,
                args.vulkan_iters,
                args.vulkan_msaa,
                if args.vulkan_window {
                    "windowed"
                } else {
                    "offscreen"
                },
            ))
        );
    }

    // Optionally start the Vulkan graphics engine (if built with --features "vulkan").
    #[cfg(feature = "vulkan")]
    let mut vulkan_engine: Option<VulkanGraphicsEngine> = None;

    #[cfg(feature = "vulkan")]
    {
        if vulkan_stress_enabled(&args)
            && let Some(identity) = cuda_device_identity
        {
            let render_config = build_vulkan_render_config(&args);
            let selection = VulkanDeviceSelection {
                uuid: identity.uuid,
                pci_bus: identity.pci_bus,
                index: None,
            };
            let mut eng = VulkanGraphicsEngine::with_selection(selection, render_config);
            match eng.start_stress_thread() {
                Ok(_) => {
                    vulkan_engine = Some(eng);
                }
                Err(e) => {
                    eprintln!(
                        "{}",
                        stylize(
                            &format!("Failed to start VulkanGraphicsEngine: {}", e),
                            true
                        )
                    );
                }
            }
        }
    }

    #[cfg(feature = "vulkan")]
    let vulkan_abort = vulkan_engine.as_ref().map(|eng| eng.get_error_flag_arc());
    #[cfg(not(feature = "vulkan"))]
    let vulkan_abort: Option<std::sync::Arc<std::sync::atomic::AtomicBool>> = None;

    // Optionally start the NVENC→NVDEC video sidecar (if built with
    // --features video). It rides the already-resolved CUDA device; a
    // missing/old driver video library only disables the sidecar.
    #[cfg(feature = "video")]
    let mut video_engine: Option<VideoEngine> = None;

    #[cfg(feature = "video")]
    if args.video {
        match build_video_config(&args, gpu_device_index, false) {
            Ok(config) => match VideoEngine::new(config) {
                Ok(mut engine) => match engine.start() {
                    Ok(()) => {
                        video_engine = Some(engine);
                    }
                    Err(err) => {
                        eprintln!(
                            "{}",
                            stylize(&format!("Failed to start VideoEngine: {err}"), true)
                        );
                    }
                },
                Err(err) => {
                    println!(
                        "{}",
                        stylize(
                            &format!("Warning: video engine unavailable (sidecar disabled): {err}"),
                            false
                        )
                    );
                }
            },
            Err(err) => {
                eprintln!(
                    "{}",
                    stylize(&format!("Invalid video configuration: {err}"), true)
                );
                std::process::exit(2);
            }
        }
    }

    // ---- SDC event stream (--sdc-log): structured fault records with the
    // telemetry snapshot taken at detection time, plus a fixed-cadence
    // telemetry thread writing rows to the same JSONL (SDC-rate vs
    // frequency/temperature mapping data).
    let sdc_writer: Option<std::sync::Arc<std::sync::Mutex<std::io::BufWriter<std::fs::File>>>> =
        args.sdc_log.as_ref().map(|path| {
            std::sync::Arc::new(std::sync::Mutex::new(std::io::BufWriter::new(
                fs::File::create(path).unwrap_or_else(|err| {
                    eprintln!(
                        "{}",
                        stylize(&format!("Failed to create SDC log {path:?}: {err}"), true)
                    );
                    std::process::exit(1);
                }),
            )))
        });
    let sdc_telemetry =
        cli_stressor_rs::Telemetry::open(gpu_device_index, backend.device_uuid().ok());
    let sdc_event_count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let sdc_sampler_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Some(writer) = sdc_writer.clone()
        && sdc_telemetry.available()
    {
        let stop = sdc_sampler_stop.clone();
        // Own NVML handle: Nvml is neither Send nor Sync, but each thread can
        // hold its own (nvmlInit is refcounted inside the driver).
        let sampler_telemetry =
            cli_stressor_rs::Telemetry::open(gpu_device_index, backend.device_uuid().ok());
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                if let Some(sample) = sampler_telemetry.sample() {
                    let mut row = serde_json::to_value(sample).unwrap_or_default();
                    row["kind"] = serde_json::json!("telemetry");
                    if let Ok(mut w) = writer.lock() {
                        use std::io::Write;
                        let _ = writeln!(w, "{row}");
                    }
                }
            }
        });
    } else {
        sdc_sampler_stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    // Sink closure: serialize events into the JSONL (the event's telemetry
    // field was sampled by the dispatch loop via `sdc_telemetry` below).
    let sdc_sink_impl = sdc_writer.map(|writer| {
        let counter = sdc_event_count.clone();
        move |event: &cli_stressor_rs::SdcEvent| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Ok(mut w) = writer.lock() {
                use std::io::Write;
                if let Ok(line) = serde_json::to_string(event) {
                    let _ = writeln!(w, "{line}");
                }
            }
        }
    });
    let sdc_sink: Option<&dyn Fn(&cli_stressor_rs::SdcEvent)> = sdc_sink_impl
        .as_ref()
        .map(|f| f as &dyn Fn(&cli_stressor_rs::SdcEvent));

    let results = run_stress_mixed(
        &mut backend,
        &filtered,
        StressRunConfig {
            matrix_sizes: &matrix_sizes,
            fp64_matrix_sizes: &fp64_matrix_sizes,
            duration_s: args.duration,
            warmup_iters: args.warmup_iters,
            burst_iters: args.burst_iters,
            validate_interval_s: args.validate_interval,
            validate_size: args.validate_size,
            transpose_prob: args.transpose_prob,
            base_seed: args.seed,
            minor_mixture_rate: args.minor_mixture_rate,
            kernel_mixture: &kernel_mixture,
            stream_mode,
            kernel_param_overrides: &kernel_param_overrides,
            verify: verify_cfg,
            sdc_event_sink: sdc_sink,
            telemetry: Some(&sdc_telemetry),
        },
        vulkan_abort,
    );
    sdc_sampler_stop.store(true, std::sync::atomic::Ordering::SeqCst);

    for res in &results {
        if res.first_error.is_some() && res.supported {
            overall_passed = false;
        }
    }

    #[cfg(feature = "vulkan")]
    if let Some(eng) = &vulkan_engine
        && eng
            .get_error_flag_arc()
            .load(std::sync::atomic::Ordering::SeqCst)
    {
        eprintln!(
            "{}",
            stylize("[FATAL] Vulkan engine reported an error", true)
        );
        overall_passed = false;
    }

    print_summary(&results, &info);

    // Stop Vulkan engine if it was started.
    #[cfg(feature = "vulkan")]
    if let Some(mut eng) = vulkan_engine
        && let Err(e) = eng.stop()
    {
        eprintln!(
            "{}",
            stylize(&format!("[FATAL] Vulkan engine stop failed: {}", e), true)
        );
        overall_passed = false;
    }

    // Stop the video sidecar and collect its counters for the verdict.
    #[cfg(feature = "video")]
    let video_stats: Option<VideoStats> = if let Some(mut engine) = video_engine {
        if engine
            .get_error_flag_arc()
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            eprintln!(
                "{}",
                stylize("[FATAL] Video engine reported an error", true)
            );
            overall_passed = false;
        }
        match engine.stop() {
            Ok(stats) => Some(stats),
            Err(err) => {
                eprintln!(
                    "{}",
                    stylize(&format!("[FATAL] Video engine stop failed: {err}"), true)
                );
                overall_passed = false;
                Some(engine.stats())
            }
        }
    } else {
        None
    };

    #[cfg(feature = "video")]
    if let Some(stats) = &video_stats {
        println!(
            "{}",
            stylize(
                &format!(
                    "[Video] summary: encoded={} decoded={} crc_mismatches={} encode={:.1} Mbps decode={:.1} fps",
                    stats.encoded_frames,
                    stats.decoded_frames,
                    stats.crc_mismatches,
                    stats.encode_mbps,
                    stats.decode_fps
                ),
                false
            )
        );
        if stats.crc_mismatches > 0 {
            overall_passed = false;
        }
    }
    #[cfg(not(feature = "video"))]
    let video_stats: Option<serde_json::Value> = None;

    // Machine-readable verdict (P0#4): one stdout line + optional --json-out.
    let verify_totals: cli_stressor_rs::DetectorStats = {
        let mut totals = cli_stressor_rs::DetectorStats::default();
        for r in &results {
            totals.merge(&r.detectors);
        }
        totals
    };
    let validation_failures: u64 = results.iter().map(|r| r.validation_failures as u64).sum();
    let detector_errors: u64 =
        verify_totals.total_errors + verify_totals.mismatches + verify_totals.nonfinite;
    let runtime_errors: u64 = results
        .iter()
        .filter(|r| {
            r.supported
                && r.first_error
                    .as_deref()
                    .is_some_and(|e| e.starts_with("runtime error"))
        })
        .count() as u64;
    let self_ok = self_test.as_ref().map(|s| s.passed).unwrap_or(true);
    let class = classify(
        self_ok,
        validation_failures,
        detector_errors,
        runtime_errors,
    );
    let elements_produced: u64 = results.iter().map(|r| r.elements_produced).sum();
    let coverage = if elements_produced > 0 {
        (verify_totals.elements_checked as f64 / elements_produced as f64).min(1.0)
    } else {
        0.0
    };
    // Initial confidence formula: errors are hard-fails anyway, so the number
    // communicates how much of the produced workload the detectors observed.
    let confidence_pct = (100.0 * coverage * (1.0 - (detector_errors as f64).min(1.0))) as u32;
    let verdict = serde_json::json!({
        "stressor": "cli-stressor-rs",
        "result": if class == VerdictClass::None && overall_passed { "pass" } else { "fail" },
        "classification": class.as_str(),
        "device": info.name,
        "duration_s": args.duration,
        "self_test": self_test,
        "precisions": results,
        "coverage": (coverage * 10000.0).round() / 10000.0,
        "confidence_pct": confidence_pct,
        "verify_config": {
            "enabled": verify_cfg.enabled,
            "self_test": verify_cfg.self_test,
            "memcpy_every": verify_cfg.memcpy_every,
            "memset_every": verify_cfg.memset_every,
            "gemm_every": verify_cfg.gemm_every,
            "gemm_samples": verify_cfg.gemm_samples,
            "gemm_full_check_max_size": verify_cfg.full_check_max_size,
            "continue_on_error": verify_cfg.continue_on_error,
            "resident_interval_s": verify_cfg.resident_interval_s,
            "intalu_samples": verify_cfg.intalu_samples,
            "int8_validate_size": verify_cfg.int8_validate_size,
        },
        "mem_mode": format!("{mem_mode:?}").to_lowercase(),
        "l2_cache_bytes": backend.device_info().l2_cache_bytes,
        "resident_words": backend.resident_words(),
        "sdc_events": sdc_event_count.load(std::sync::atomic::Ordering::SeqCst),
        "telemetry_enabled": sdc_telemetry.available(),
        "video": video_stats,
    });
    emit_verdict(verdict, &args.json_out);

    if !overall_passed {
        std::process::exit(1);
    }
}

#[cfg(all(not(feature = "cuda"), feature = "vulkan"))]
pub fn run_from_args() {
    if invoked_without_args() {
        println!("{}", render_grouped_help());
        return;
    }
    let args = Args::parse();

    // Video-only mode works without CUDA (the video engine owns its own
    // CUDA context via the driver API).
    if args.video_only {
        #[cfg(feature = "video")]
        {
            let config = match build_video_config(&args, args.gpu_index.unwrap_or(0), true) {
                Ok(config) => config,
                Err(err) => {
                    eprintln!(
                        "{}",
                        stylize(&format!("Invalid video configuration: {err}"), true)
                    );
                    std::process::exit(2);
                }
            };
            std::process::exit(run_video_for_duration(
                args.duration,
                config,
                args.json_out.as_ref(),
            ));
        }

        #[cfg(not(feature = "video"))]
        {
            eprintln!(
                "{}",
                stylize("--video-only requires building with --features video", true)
            );
            std::process::exit(2);
        }
    }

    if vulkan_stress_enabled(&args) || args.vulkan_only {
        let selection = match build_vulkan_device_selection(&args) {
            Ok(sel) => sel,
            Err(err) => {
                eprintln!(
                    "{}",
                    stylize(&format!("Invalid Vulkan GPU selection: {}", err), true)
                );
                std::process::exit(2);
            }
        };
        let render_config = build_vulkan_render_config(&args);
        std::process::exit(run_vulkan_for_duration(
            args.duration,
            selection,
            render_config,
        ));
    }

    eprintln!(
        "CUDA support is disabled. Use --vulkan-only or --vulkan when building with --features vulkan, or rebuild with --features cuda."
    );
    std::process::exit(1);
}

#[cfg(all(not(feature = "cuda"), not(feature = "vulkan")))]
pub fn run_from_args() {
    if invoked_without_args() {
        println!("{}", render_grouped_help());
        return;
    }
    let args = Args::parse();

    // Video-only mode is the one engine that runs without the cuda/vulkan
    // features (the video feature loads its own driver libraries).
    if args.video_only {
        #[cfg(feature = "video")]
        {
            let config = match build_video_config(&args, args.gpu_index.unwrap_or(0), true) {
                Ok(config) => config,
                Err(err) => {
                    eprintln!(
                        "{}",
                        stylize(&format!("Invalid video configuration: {err}"), true)
                    );
                    std::process::exit(2);
                }
            };
            std::process::exit(run_video_for_duration(
                args.duration,
                config,
                args.json_out.as_ref(),
            ));
        }

        #[cfg(not(feature = "video"))]
        {
            eprintln!(
                "{}",
                stylize("--video-only requires building with --features video", true)
            );
            std::process::exit(2);
        }
    }

    eprintln!(
        "{}",
        stylize(
            "CUDA support is disabled. Rebuild with --features cuda.",
            true
        )
    );
    std::process::exit(1);
}

#[allow(dead_code)]
fn main() {
    run_from_args();
}
