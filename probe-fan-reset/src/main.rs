//! Round 2: verify the bit0 (override) model end-to-end on both cards and
//! leave every card at driver-auto (bit0 = 0):
//!   pin 75 -> sample -> reset(level:None, bit0=0) -> samples over ~9s
//!   -> pin 40 -> sample -> reset -> final samples
//! A pinned level reads back constant with the requested %; after the reset
//! the level readback is 0 (no request) and the tach shows auto behavior.

use std::thread;
use std::time::Duration;

use nvapi::sys::gpu::cooler::undocumented::FanCoolerId;
use nvapi::{CoolerPolicy, CoolerSettings, Percentage};
use nvapi::PhysicalGpu;

fn tach(gpu: &PhysicalGpu) -> String {
    match gpu.cooler_status() {
        Ok(map) => map
            .iter()
            .map(|(id, s)| {
                let t = s
                    .current_tach
                    .map(|r| r.0.to_string())
                    .unwrap_or_else(|| "-".into());
                format!("{:?}: {}% {}rpm", id, s.current_level.0, t)
            })
            .collect::<Vec<_>>()
            .join(" | "),
        Err(e) => format!("err {e}"),
    }
}

fn sample(gpu: &PhysicalGpu, label: &str) {
    println!("  [{label}] {}", tach(gpu));
}

fn pin(gpu: &PhysicalGpu, level: u32) {
    let rc = gpu.set_cooler([(
        FanCoolerId::Cooler1,
        CoolerSettings {
            policy: CoolerPolicy::TemperatureContinuous,
            level: Some(Percentage(level)),
        },
    )]);
    println!("== pin {level}%: {:?}", rc.map(|_| "ok").map_err(|e| e.to_string()));
}

fn reset(gpu: &PhysicalGpu) {
    // bit0 = 0 via level:None — the candidate-b reset.
    let rc = gpu.set_cooler([(
        FanCoolerId::Cooler1,
        CoolerSettings {
            policy: CoolerPolicy::TemperatureContinuous,
            level: None,
        },
    )]);
    println!("== reset (bit0=0): {:?}", rc.map(|_| "ok").map_err(|e| e.to_string()));
}

fn cycle(gpu: &PhysicalGpu) {
    pin(gpu, 75);
    thread::sleep(Duration::from_secs(2));
    sample(gpu, "pinned75 t2");
    reset(gpu);
    for s in ["reset t1", "reset t4", "reset t7", "reset t10"] {
        thread::sleep(Duration::from_secs(3));
        sample(gpu, s);
    }
    pin(gpu, 40);
    thread::sleep(Duration::from_secs(2));
    sample(gpu, "repinned40 t2");
    reset(gpu);
    for s in ["final t1", "final t4", "final t7"] {
        thread::sleep(Duration::from_secs(3));
        sample(gpu, s);
    }
}

fn main() {
    let args: Vec<usize> = std::env::args()
        .skip(1)
        .filter_map(|s| s.parse().ok())
        .collect();
    let targets: Vec<usize> = if args.is_empty() { vec![0, 1] } else { args };

    nvapi::initialize().expect("nvapi init");
    let gpus = PhysicalGpu::enumerate().expect("enumerate");
    println!("gpus: {}", gpus.len());
    for idx in targets {
        let Some(gpu) = gpus.get(idx) else { continue };
        println!(
            "── gpu idx {idx} bus {:?} ──",
            gpu.bus_id().map_err(|e| e.to_string())
        );
        cycle(gpu);
    }
}
