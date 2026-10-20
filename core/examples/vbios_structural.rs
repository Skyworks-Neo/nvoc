//! 结构化解码冒烟工具（对照 vix-editor 的只读能力面）：
//! `cargo run -p nvoc-core --example vbios_structural -- <rom>...`
//!
//! 对每张 ROM 输出：镜像链、PCI 身份 → 架构族门、功率策略/通道、rail/VFE
//! （REL 上限 µV、评估 VMIN）、时钟域范围、风扇通道；GB20x 另有对象目录
//! SHA384 完整性与 override profiles。失败逐表报告（fail-closed 的表会
//! 标 `unsupported`）。

use std::fmt::Write as _;

use nvoc_core::vbios::families::{self, Family};
use nvoc_core::vbios::gb20x;
use nvoc_core::vbios::image_chain::{ImageKind, scan_images};
use nvoc_core::vbios::tables_v3;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: vbios_structural <rom>...");
        std::process::exit(2);
    }
    let mut pass = 0usize;
    let mut fail = 0usize;
    for path in &args {
        match std::fs::read(path) {
            Ok(data) => {
                let (ok, report) = analyze(&data);
                println!("== {} ({} bytes)", path, data.len());
                println!("{}", report.trim_end());
                if ok {
                    pass += 1;
                } else {
                    fail += 1;
                }
            }
            Err(e) => {
                println!("== {} — read error: {e}", path);
                fail += 1;
            }
        }
    }
    println!("\n{} pass, {} fail", pass, fail);
    if fail > 0 {
        std::process::exit(1);
    }
}

/// 返回 (整体是否解出链+至少一层表, 报告)。
fn analyze(data: &[u8]) -> (bool, String) {
    let mut out = String::new();
    let (pci, extended, issues) = scan_images(data);
    let _ = writeln!(
        out,
        "  images: {} pci + {} npds, issues: {}",
        pci.len(),
        extended.len(),
        issues.len()
    );
    for p in pci.iter().chain(extended.iter()) {
        let kind = match p.kind {
            ImageKind::Pcir => "PCIR",
            ImageKind::Npds => "NPDS",
        };
        let _ = writeln!(
            out,
            "    {kind} @{:#08x} size={:#x} type={:#04x} dev={:04x}:{:04x}{}",
            p.start,
            p.size,
            p.image_type,
            p.vendor,
            p.device,
            if p.last { " last" } else { "" }
        );
    }
    let versions = families::scan_bios_versions(data);
    let _ = writeln!(out, "  bios versions: {:?}", versions);
    let Some(legacy) = pci.iter().find(|p| p.is_legacy()) else {
        return (
            false,
            out + "  no legacy PCIR image (container-only dump?)\n",
        );
    };
    let arch = families::architecture(
        legacy.device,
        &versions.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
    );
    let _ = writeln!(
        out,
        "  arch: device={:04x} family={:?} product={} in_scope={} coherent={}",
        legacy.device, arch.family, arch.product, arch.in_scope, arch.bios_family_consistent
    );
    let family = match arch.family {
        Some(f) => Some(f),
        // 设备不在 NVIDIA 产品表（如 OEM Tesla T10 0x1E37）：诊断工具按
        // BIOS 版本前缀推断族（90.=turing、94.=ampere、95.=ada、98.=gb20x），
        // 明确标注为推断而非身份识别。
        None => {
            let guess = versions.first().and_then(|v| match v.as_bytes().first() {
                Some(b'9') => match v.as_bytes().get(1) {
                    Some(b'0') => Some(Family::Turing),
                    Some(b'2') => Some(Family::Ampere), // GA100 92.（容量未审计，仅表解码）
                    Some(b'4') => Some(Family::Ampere),
                    Some(b'5') => Some(Family::Ada),
                    Some(b'8') => Some(Family::BlackwellGb20x),
                    _ => None,
                },
                _ => None,
            });
            if guess.is_some() {
                let _ = writeln!(
                    out,
                    "  note: device not in product inventory; family inferred from BIOS version prefix"
                );
            }
            guess
        }
    };
    let Some(Family::BlackwellGb20x) = family else {
        return analyze_3x(data, family, &mut out);
    };
    analyze_gb20x(data, &mut out)
}

fn analyze_3x(data: &[u8], family: Option<Family>, out: &mut String) -> (bool, String) {
    let Some(family) = family.filter(|f| f.in_scope()) else {
        return (
            false,
            out.clone() + "  family out of scope for structural tables\n",
        );
    };
    let ctx = match tables_v3::TableContext::new(data, family) {
        Ok(c) => c,
        Err(e) => return (false, format!("{out}  chain/BIT: {e}\n")),
    };
    let _ = writeln!(
        out,
        "  chain: {} images, {} duplicate bank(s)",
        ctx.chain.len(),
        ctx.chain_count
    );
    for (name, ok) in ctx.integrity(data) {
        if !ok {
            let _ = writeln!(out, "  integrity: {name} checksum != 0");
        }
    }
    let mut ok = true;
    report(
        "power policy",
        tables_v3::power_policy(data, &ctx).map(|p| {
            let tgp = &p.rows[p.tgp_index];
            format!(
                "v{:#x} rows={} tgp=row{} min/def/max = {}/{}/{} {}{}",
                p.version,
                p.rows.len(),
                p.tgp_index,
                tgp.minimum_raw,
                tgp.default_raw,
                tgp.maximum_raw,
                if tgp.unit_ma { "mA" } else { "mW" },
                if p.notebook { " (notebook)" } else { "" }
            )
        }),
        out,
        &mut ok,
    );
    report(
        "power channels",
        tables_v3::power_channels(data, &ctx).map(|c| {
            let total = c.entries.iter().filter(|e| e.is_total_board()).count();
            format!(
                "{} entries, entry_size {}, total-board channels: {}",
                c.count, c.entry_size, total
            )
        }),
        out,
        &mut ok,
    );
    report(
        "rails",
        tables_v3::rails(data, &ctx).map(|r| {
            r.rows
                .iter()
                .map(|row| {
                    let sels: Vec<String> = row
                        .selectors
                        .iter()
                        .map(|(role, _, raw)| {
                            format!(
                                "{}={}",
                                role.name(),
                                if *raw == 0xFF {
                                    "absent".into()
                                } else {
                                    format!("{raw}")
                                }
                            )
                        })
                        .collect();
                    format!(
                        "{} boot={}mV {}",
                        row.name,
                        row.boot_voltage_uv / 1000,
                        sels.join(" ")
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ")
        }),
        out,
        &mut ok,
    );
    report(
        "REL limits",
        tables_v3::rail_limits(data, &ctx).map(|l| {
            l.iter()
                .map(|r| {
                    format!(
                        "{} REL max = {:.1} mV (kind {:?})",
                        r.rail_name,
                        r.output_maximum_uv / 1000.0,
                        r.equation_kind
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ")
        }),
        out,
        &mut ok,
    );
    report(
        "VMIN",
        tables_v3::rail_vmin(data, &ctx).map(|v| {
            v.iter()
                .map(|r| match (r.value_uv, r.diagnostic) {
                    (Some(uv), _) => format!(
                        "{} = {:.1} mV (pinned)",
                        r.rail_name,
                        f64::from(uv) / 1000.0
                    ),
                    (None, Some(d)) => format!("{}: {d}", r.rail_name),
                    (None, None) => format!("{}: unpinned", r.rail_name),
                })
                .collect::<Vec<_>>()
                .join(" | ")
        }),
        out,
        &mut ok,
    );
    report(
        "clocks",
        tables_v3::clock_ranges(data, &ctx).map(|c| {
            c.iter()
                .filter(|r| r.range_stored)
                .map(|r| format!("{} {}..{}MHz", r.domain, r.minimum_mhz, r.maximum_mhz))
                .collect::<Vec<_>>()
                .join(", ")
        }),
        out,
        &mut ok,
    );
    match tables_v3::fan_tables(data, &ctx) {
        Ok(None) => {
            let _ = writeln!(out, "  fans: no GPU-driven fan");
        }
        Ok(Some(f)) => {
            let chans: Vec<String> = f
                .rows
                .iter()
                .filter(|r| r.active)
                .map(|r| {
                    format!(
                        "ch{} {}..{}% {}..{}rpm pwm={}Hz curve={:?}",
                        r.index,
                        r.level_minimum,
                        r.level_maximum,
                        r.rpm_minimum,
                        r.rpm_maximum,
                        r.pwm_frequency_hz,
                        r.curve.points
                    )
                })
                .collect();
            let _ = writeln!(
                out,
                "  fans: channels {:?} {}",
                f.channels,
                chans.join(" | ")
            );
            for w in &f.warnings {
                let _ = writeln!(out, "    warning: {w}");
            }
        }
        Err(e) => {
            let _ = writeln!(out, "  fans: unsupported ({e})");
        }
    }
    (ok, out.clone())
}

fn analyze_gb20x(data: &[u8], out: &mut String) -> (bool, String) {
    let layout = match gb20x::parse_layout(data) {
        Ok(l) => l,
        Err(e) => return (false, format!("{out}  layout: {e}\n")),
    };
    let integrity = gb20x::integrity(data, &layout);
    let _ = writeln!(
        out,
        "  directory: {} objects, sha384 valid = {}, image sums = {:?}, legacy dir = {}",
        integrity.object_count,
        integrity.directory_sha384_valid,
        integrity.image_checksums,
        layout.legacy_dir.is_some()
    );
    if !integrity.bad_object_ids.is_empty() {
        let _ = writeln!(out, "  BAD objects: {:?}", integrity.bad_object_ids);
    }
    let mut ok = true;
    report(
        "power policy",
        gb20x::power_policy(data, &layout).map(|p| {
            let tgp = if p.notebook { &p.rows[2] } else { &p.rows[8] };
            format!(
                "v{:#x} rows={} {} TGP=row{} min/def/max = {}/{}/{} mW{}",
                p.version,
                p.rows.len(),
                if p.notebook { "notebook" } else { "desktop" },
                if p.notebook { 2 } else { 8 },
                tgp.minimum_raw,
                tgp.default_raw,
                tgp.maximum_raw,
                if p.notebook { "" } else { " (row8)" }
            )
        }),
        out,
        &mut ok,
    );
    report(
        "REL limits",
        gb20x::rail_limits(data, &layout).map(|l| {
            l.iter()
                .map(|r| {
                    format!(
                        "{} REL max = {:.1} mV",
                        r.rail_name,
                        r.output_maximum_uv / 1000.0
                    )
                })
                .collect::<Vec<_>>()
                .join(" | ")
        }),
        out,
        &mut ok,
    );
    report(
        "VMIN",
        gb20x::rail_vmin(data, &layout).map(|v| {
            v.iter()
                .map(|r| match (r.value_uv, r.diagnostic) {
                    (Some(uv), _) => format!(
                        "{} = {:.1} mV (pinned)",
                        r.rail_name,
                        f64::from(uv) / 1000.0
                    ),
                    (None, Some(d)) => format!("{}: {d}", r.rail_name),
                    (None, None) => format!("{}: unpinned", r.rail_name),
                })
                .collect::<Vec<_>>()
                .join(" | ")
        }),
        out,
        &mut ok,
    );
    report(
        "clocks",
        gb20x::clock_ranges(data, &layout).map(|c| {
            c.iter()
                .map(|r| format!("{} {}..{}MHz", r.domain, r.minimum_mhz, r.maximum_mhz))
                .collect::<Vec<_>>()
                .join(", ")
        }),
        out,
        &mut ok,
    );
    report(
        "profiles",
        gb20x::power_profiles(data, &layout).map(|p| {
            if p.is_empty() {
                "none enabled".to_string()
            } else {
                p.iter()
                    .map(|prof| {
                        format!(
                            "#{}: {}",
                            prof.index,
                            prof.entries
                                .iter()
                                .map(|e| format!(
                                    "p{}/{}={}",
                                    e.policy,
                                    if e.default_field { "def" } else { "max" },
                                    e.value_raw
                                ))
                                .collect::<Vec<_>>()
                                .join(" ")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" | ")
            }
        }),
        out,
        &mut ok,
    );
    (ok, out.clone())
}

fn report(label: &str, r: Result<String, nvoc_core::Error>, out: &mut String, ok: &mut bool) {
    match r {
        Ok(s) => {
            let _ = writeln!(out, "  {label}: {s}");
        }
        Err(e) => {
            let _ = writeln!(out, "  {label}: unsupported ({e})");
            *ok = false;
        }
    }
}
