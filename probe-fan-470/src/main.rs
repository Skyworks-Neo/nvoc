//! Round 2: pin down WHY NvAPI_GPU_ClientFanCoolersSetControl returns -1 on
//! 472.12 while the GET half is alive.
//!
//! Hypotheses tested, in order:
//!   H1 input-shape: `Gpu::set_cooler` sends a fresh struct (top-level flags=0,
//!      never valid-bit-set, entries rebuilt from scratch). The driver's SET
//!      handler may require the GET'd snapshot (RMW) and/or flags bit0=valid.
//!      set_fan_rpm (0xEB44E8AA) does GET→patch→SET and works — corroborates.
//!   H2 public family dead: raw public GetCoolerSettings returned -104 with
//!      the V4 stamp + TARGET_ALL(7). Retry with V1 stamp / single index to
//!      see if ANY public cooler surface answers (decides whether widening
//!      the -3/-4 fallback to public SetCoolerLevels can even work here).
//!
//! Stages (write stages self-restore from the GET snapshot):
//!   read      — GETs only (default)
//!   fresh-set — reproduce set_cooler's fresh-struct SET (expected -1, no-op)
//!   rmw-set   — snapshot → patch level/manual/valid → SET → verify → restore

use std::fmt::Write as _;
use std::thread;
use std::time::Duration;

use nvapi::sys::gpu::cooler::undocumented::{
    NvAPI_GPU_ClientFanCoolersGetControl, NvAPI_GPU_ClientFanCoolersSetControl,
    NvAPI_GPU_GetCoolerSettings, NV_GPU_CLIENT_FAN_COOLERS_CONTROL,
    NV_GPU_GETCOOLER_SETTINGS, NV_GPU_GETCOOLER_SETTINGS_V1,
};
use nvapi::PhysicalGpu;

fn status_name(rc: i32) -> &'static str {
    match rc {
        0 => "OK",
        -1 => "ERROR(-1)",
        -3 => "NO_IMPLEMENTATION(-3)",
        -4 => "API_NOT_INITIALIZED(-4)",
        -5 => "INVALID_ARGUMENT(-5)",
        -9 => "INCOMPATIBLE_STRUCT_VERSION(-9)",
        -104 => "NOT_SUPPORTED(-104)",
        _ => "other",
    }
}

fn snapshot(gpu: &PhysicalGpu) -> Option<NV_GPU_CLIENT_FAN_COOLERS_CONTROL> {
    let mut control = NV_GPU_CLIENT_FAN_COOLERS_CONTROL::default();
    let rc = unsafe {
        NvAPI_GPU_ClientFanCoolersGetControl(*gpu.handle(), &mut control as *mut _)
    };
    if rc != 0 {
        println!("  GET failed: {rc} {}", status_name(rc));
        return None;
    }
    println!(
        "  GET: top flags {:#x} valid={} count {}",
        control.flags,
        control.valid(),
        control.count
    );
    for c in control.coolers() {
        println!(
            "    id {} level {} flags {:#x} manual={}",
            c.cooler_id.repr(),
            c.level,
            c.flags,
            c.manual()
        );
    }
    Some(control)
}

/// SET with a fresh struct like `Gpu::set_cooler` builds it; `count` mirrors
/// the CLI `--fan all` (Cooler1+Cooler2) vs a single-cooler target.
unsafe fn fresh_set(gpu: &PhysicalGpu, level: u32, count: u32) -> i32 {
    let mut data = NV_GPU_CLIENT_FAN_COOLERS_CONTROL::default();
    data.count = count;
    for (i, entry) in data.coolers.iter_mut().enumerate().take(count as usize) {
        let id = match i {
            0 => nvapi::sys::gpu::cooler::undocumented::FanCoolerId::Cooler1,
            _ => nvapi::sys::gpu::cooler::undocumented::FanCoolerId::Cooler2,
        };
        entry.cooler_id = id.into();
        entry.level = level;
        entry.set_manual(true);
    }
    NvAPI_GPU_ClientFanCoolersSetControl(*gpu.handle(), &data as *const _)
}

/// RMW SET: patch level+manual (+valid bit), send the snapshot back.
unsafe fn rmw_set(
    gpu: &PhysicalGpu,
    snap: &NV_GPU_CLIENT_FAN_COOLERS_CONTROL,
    level: u32,
    force_valid: bool,
) -> i32 {
    let mut data = NV_GPU_CLIENT_FAN_COOLERS_CONTROL::default();
    data.version = snap.version;
    data.flags = snap.flags;
    data.count = snap.count;
    data.coolers = snap.coolers;
    if force_valid {
        data.set_valid(true);
    }
    for entry in data.coolers.iter_mut().take(data.count as usize) {
        entry.level = level;
        entry.set_manual(true);
    }
    NvAPI_GPU_ClientFanCoolersSetControl(*gpu.handle(), &data as *const _)
}

/// READ-ONLY: raw view of the 0xEB44E8AA simulation control block (enable
/// bit + absolute 0..65536 duty level) — the surface the percent fallback
/// pins through.
fn sim_read(gpu: &PhysicalGpu) {
    use nvapi::sys::gpu::cooler::undocumented::{
        NvAPI_GPU_FanCoolerGetControl, NvAPI_GPU_FanCoolerGetInfo,
        NV_GPU_FAN_COOLER_CONTROL_MAGIC, NV_GPU_FAN_COOLER_CONTROL_SIZE,
        NV_GPU_FAN_COOLER_ENTRY_STRIDE, NV_GPU_FAN_COOLER_ENTRY0_BASE,
        NV_GPU_FAN_COOLER_INFO_MAGIC, NV_GPU_FAN_COOLER_INFO_SIZE,
        NV_GPU_FAN_COOLER_OFF_ENABLE, NV_GPU_FAN_COOLER_OFF_LEVEL,
        NV_GPU_FAN_COOLER_OFF_MAX_RPM, NV_GPU_FAN_COOLER_OFF_MIN_RPM,
        NV_GPU_FAN_COOLER_OFF_TYPE,
    };

    fn read_u32(buf: &[u8], off: usize) -> u32 {
        u32::from_ne_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
    }

    let mut info = vec![0u8; NV_GPU_FAN_COOLER_INFO_SIZE];
    info[..4].copy_from_slice(&NV_GPU_FAN_COOLER_INFO_MAGIC.to_ne_bytes());
    let rc = unsafe { NvAPI_GPU_FanCoolerGetInfo(*gpu.handle(), info.as_mut_ptr() as *mut _) };
    println!("  FanCoolerGetInfo: {rc}");
    let mask = read_u32(&info, 0x04);
    let mut buf = vec![0u8; NV_GPU_FAN_COOLER_CONTROL_SIZE];
    buf[..4].copy_from_slice(&NV_GPU_FAN_COOLER_CONTROL_MAGIC.to_ne_bytes());
    buf[4..8].copy_from_slice(&mask.to_ne_bytes());
    let rc = unsafe { NvAPI_GPU_FanCoolerGetControl(*gpu.handle(), buf.as_mut_ptr() as *mut _) };
    println!("  FanCoolerGetControl: {rc} (mask {mask:#x})");
    for k in 0..32u32 {
        if mask & (1 << k) == 0 {
            continue;
        }
        let base = NV_GPU_FAN_COOLER_ENTRY0_BASE + k as usize * NV_GPU_FAN_COOLER_ENTRY_STRIDE;
        let en = read_u32(&buf, base + NV_GPU_FAN_COOLER_OFF_ENABLE);
        let level = read_u32(&buf, base + NV_GPU_FAN_COOLER_OFF_LEVEL);
        println!(
            "    cooler {k}: type {} min {} max {} enable {:#x} level {} (≈{}%)",
            read_u32(&buf, base + NV_GPU_FAN_COOLER_OFF_TYPE),
            read_u32(&buf, base + NV_GPU_FAN_COOLER_OFF_MIN_RPM),
            read_u32(&buf, base + NV_GPU_FAN_COOLER_OFF_MAX_RPM),
            en,
            level,
            level * 100 / 65536
        );
    }
}

fn main() {
    let stage = std::env::args().nth(1).unwrap_or_else(|| "read".into());
    nvapi::initialize().expect("nvapi init");
    let gpus = PhysicalGpu::enumerate().expect("enumerate");
    let gpu = &gpus[0];

    println!("== stage: {stage} ==");
    let Some(snap) = snapshot(gpu) else { return };

    if stage == "read" {
        // Public family probes: V4+ALL already gave -104. Try V1 stamp and a
        // single-cooler index to see if any public variant answers.
        {
            let mut s = NV_GPU_GETCOOLER_SETTINGS_V1::zeroed();
            s.version = nvapi::sys::nvapi::NvVersion::with_version((1 << 16) | 152);
            let rc = unsafe {
                NvAPI_GPU_GetCoolerSettings(*gpu.handle(), 7, &mut s as *mut _ as *mut _)
            };
            println!("  public GET v1 stamp idx7: {rc} {}", status_name(rc));
        }
        {
            let mut s = NV_GPU_GETCOOLER_SETTINGS_V1::zeroed();
            s.version = nvapi::sys::nvapi::NvVersion::with_version((1 << 16) | 152);
            let rc = unsafe {
                NvAPI_GPU_GetCoolerSettings(*gpu.handle(), 0, &mut s as *mut _ as *mut _)
            };
            println!("  public GET v1 stamp idx0: {rc} {}", status_name(rc));
        }
        {
            let mut s = NV_GPU_GETCOOLER_SETTINGS::default();
            let rc =
                unsafe { NvAPI_GPU_GetCoolerSettings(*gpu.handle(), 0, &mut s as *mut _) };
            println!("  public GET v4 stamp idx0: {rc} {}", status_name(rc));
        }
        return;
    }

    if stage == "sim-read" {
        sim_read(gpu);
        return;
    }

    if stage == "elev-diag" {
        let mut out = String::new();
        let _ = writeln!(out, "GET snapshot:");
        {
            let mut control = NV_GPU_CLIENT_FAN_COOLERS_CONTROL::default();
            let rc = unsafe {
                NvAPI_GPU_ClientFanCoolersGetControl(*gpu.handle(), &mut control as *mut _)
            };
            let _ = writeln!(out, "  rc {rc} flags {:#x} count {}", control.flags, control.count);
            for c in control.coolers() {
                let _ = writeln!(
                    out, "  id {} level {} flags {:#x}", c.cooler_id.repr(), c.level, c.flags
                );
            }
            let snap = control;
            let _ = writeln!(out, "variant A verbatim snapshot SET:");
            let rc = unsafe {
                NvAPI_GPU_ClientFanCoolersSetControl(*gpu.handle(), &snap as *const _)
            };
            let _ = writeln!(out, "  rc {rc}");
            let _ = writeln!(out, "variant B fresh count=1 level=100 manual:");
            let rc = unsafe { fresh_set(gpu, 100, 1) };
            let _ = writeln!(out, "  rc {rc}");
            let _ = writeln!(out, "variant C fresh count=2 (CLI --fan all) level=100 manual:");
            let rc = unsafe { fresh_set(gpu, 100, 2) };
            let _ = writeln!(out, "  rc {rc}");
            for force_valid in [false, true] {
                let _ = writeln!(
                    out, "variant D{} RMW snapshot level=40 manual valid={force_valid}:",
                    if force_valid { "+" } else { "" }
                );
                let rc = unsafe { rmw_set(gpu, &snap, 40, force_valid) };
                let _ = writeln!(out, "  rc {rc}");
                if rc == 0 {
                    thread::sleep(Duration::from_secs(2));
                    let mut verify = NV_GPU_CLIENT_FAN_COOLERS_CONTROL::default();
                    unsafe {
                        NvAPI_GPU_ClientFanCoolersGetControl(
                            *gpu.handle(),
                            &mut verify as *mut _,
                        )
                    };
                    for c in verify.coolers() {
                        let _ = writeln!(
                            out, "  verify id {} level {} flags {:#x}",
                            c.cooler_id.repr(), c.level, c.flags
                        );
                    }
                    break;
                }
            }
            let _ = writeln!(out, "restore snapshot:");
            let rc = unsafe {
                NvAPI_GPU_ClientFanCoolersSetControl(*gpu.handle(), &snap as *const _)
            };
            let _ = writeln!(out, "  rc {rc}");
        }
        let path = std::env::args().nth(2).unwrap_or_else(|| "elev-diag.txt".into());
        std::fs::write(&path, out).expect("write out");
        return;
    }

    if stage == "verbatim-set" {
        // Control experiment: SET the GET'd snapshot back verbatim (level 0,
        // manual off, valid off). If even this is -1, the handler rejects
        // every input, not just the fresh struct's shape.
        let rc =
            unsafe { NvAPI_GPU_ClientFanCoolersSetControl(*gpu.handle(), &snap as *const _) };
        println!("  verbatim snapshot SET: {rc} {}", status_name(rc));
        return;
    }

    if stage == "fresh-set" {
        let rc = unsafe { fresh_set(gpu, 100, 1) };
        println!("  fresh-struct SET level=100 manual: {rc} {}", status_name(rc));
        snapshot(gpu);
        return;
    }

    if stage == "rmw-set" {
        for force_valid in [false, true] {
            let rc = unsafe { rmw_set(gpu, &snap, 40, force_valid) };
            println!(
                "  RMW SET level=40 manual valid_bit={force_valid}: {rc} {}",
                status_name(rc)
            );
            if rc == 0 {
                break;
            }
        }
        thread::sleep(Duration::from_secs(2));
        println!("  -- verify applied --");
        snapshot(gpu);
        match gpu.cooler_status() {
            Ok(map) => {
                for (id, s) in &map {
                    println!(
                        "  tach {:?}: {}% {}rpm",
                        id,
                        s.current_level.0,
                        s.current_tach.map(|r| r.0).unwrap_or(0)
                    );
                }
            }
            Err(e) => println!("  tach read err {e}"),
        }
        // Restore the exact GET snapshot.
        let rc =
            unsafe { NvAPI_GPU_ClientFanCoolersSetControl(*gpu.handle(), &snap as *const _) };
        println!("  restore snapshot SET: {rc} {}", status_name(rc));
        thread::sleep(Duration::from_secs(2));
        println!("  -- verify restored --");
        snapshot(gpu);
    }
}
