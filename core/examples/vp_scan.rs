//! 只读诊断:扫 ROM 里的 VP 表 / footers / 功率 / 热表 / DCB 并逐条打印
//! (校准用)。
//! 用法: cargo run -p nvoc-core --example vp_scan -- <rom> [<rom>...]
fn main() {
    for path in std::env::args().skip(1) {
        let Ok(data) = std::fs::read(&path) else {
            eprintln!("{path}: unreadable");
            continue;
        };
        let tables = nvoc_core::legacy_vbios_parser::find_vp_tables(&data);
        println!("{path}: {} VP table(s), {} bytes", tables.len(), data.len());
        for t in &tables {
            println!(
                "  gen={:?} header=0x{:X} ladder=0x{:X} profiles={} entries={} mem_raw={:#06x} mem_ddr={}MHz",
                t.generation,
                t.header_offset,
                t.ladder_offset,
                t.profiles.len(),
                t.entries.len(),
                t.mem_clock_raw,
                t.mem_clock_mhz()
            );
            for p in &t.profiles {
                println!(
                    "    profile id={:#04x} @0x{:X} limits={:?} mem={}MHz ddr={}MHz",
                    p.id,
                    p.offset,
                    p.limit_raw,
                    p.mem_clock_mhz(),
                    p.mem_clock_ddr_mhz()
                );
            }
            for (i, e) in t.entries.iter().enumerate() {
                println!(
                    "    [{i}] @0x{:X} raw={:#010x} freq={:.2} MHz denom={:#04x}",
                    e.offset,
                    e.raw,
                    e.freq_mhz(),
                    e.denominator
                );
            }
            println!(
                "    footers: {:?}",
                t.footers
                    .iter()
                    .map(|f| (f.id, f.mem_clock_mhz(), f.mem_clock_ddr_mhz()))
                    .collect::<Vec<_>>()
            );
        }
        for p in nvoc_core::legacy_vbios_parser::find_power_tables(&data) {
            println!(
                "  power: {} target={}W limit={}W slider={:?} anchor=0x{:X}",
                p.platform.name(),
                p.target_mw / 1000,
                p.limit_mw / 1000,
                p.sliders.iter().map(|s| s.enabled).collect::<Vec<_>>(),
                p.anchor_offset
            );
        }
        match nvoc_core::legacy_vbios_parser::find_perf_ptr_map(&data) {
            Ok(m) => {
                println!(
                    "  perf ptrs: P@0x{:X} len={} ({} non-zero)",
                    m.ptab_offset,
                    m.p_len,
                    m.slots.len()
                );
                for s in &m.slots {
                    println!(
                        "    +0x{:02X} {:<18} -> 0x{:X} hdr={:?} plausible={}",
                        s.slot_offset, s.name, s.abs_offset, s.header, s.plausible
                    );
                }
            }
            Err(e) => println!("  perf ptrs: err {e}"),
        }
        if let Ok(Some(mc)) = nvoc_core::legacy_vbios_parser::find_memory_clock_table(&data) {
            println!(
                "  memclock: @0x{:X} ver={:#x} ranges={}",
                mc.table_offset,
                mc.ver,
                mc.entries.len()
            );
            for e in &mc.entries {
                println!(
                    "    freq {}..{} straps={}",
                    e.freq_min_raw,
                    e.freq_max_raw,
                    e.straps.len()
                );
            }
        }
        if let Ok(Some(mt)) = nvoc_core::legacy_vbios_parser::find_memory_tweak_table(&data) {
            println!(
                "  memtweak: @0x{:X} ver={:#x} sets={} x {}B (ext {}x{}B)",
                mt.table_offset,
                mt.ver,
                mt.entries.len(),
                mt.base_entry_size,
                mt.extended_entry_count,
                mt.extended_entry_size
            );
        }
        if let Ok(Some(mi)) = nvoc_core::legacy_vbios_parser::find_memory_info(&data) {
            println!(
                "  meminfo: @0x{:X} straps={} variants={}",
                mi.table_offset,
                mi.strap_count,
                mi.entries.len()
            );
            for e in &mi.entries {
                println!(
                    "    type={:?} vendor={:?} dens={:?} strap={} rev={}",
                    nvoc_core::legacy_vbios_parser::memory_type_name(e.variant.mem_type),
                    nvoc_core::legacy_vbios_parser::memory_vendor_name(e.variant.vendor_id),
                    nvoc_core::legacy_vbios_parser::memory_density_name(e.variant.density),
                    e.variant.strap,
                    e.variant.rev_id
                );
            }
        }
        match nvoc_core::legacy_vbios_parser::find_bios_identity(&data) {
            Ok(id) => println!(
                "  identity: version={:?} build={:?} msg={:?} board={:?}",
                id.version, id.build_date, id.message, id.board_id
            ),
            Err(e) => println!("  identity: err {e}"),
        }
        if let Ok(Some(tp)) = nvoc_core::legacy_vbios_parser::find_thermal_policy(&data) {
            println!("  thermal policy: @0x{:X}", tp.table_offset);
            for e in &tp.entries {
                println!(
                    "    enabled={} temps={:?}/{:?}/{:?}",
                    e.enabled, e.temp_a_c, e.temp_b_c, e.temp_c_c
                );
            }
        }
        if let Ok(Some(fc)) = nvoc_core::legacy_vbios_parser::find_fan_cooler(&data) {
            println!(
                "  fan coolers ({}): @0x{:X}",
                fc.coolers.len(),
                fc.table_offset
            );
            for c in &fc.coolers {
                println!(
                    "    max_duty={:?}% max_rpm={:?}",
                    c.max_duty_percent, c.max_rpm
                );
            }
        }
        if let Ok(Some(fp)) = nvoc_core::legacy_vbios_parser::find_fan_policy(&data) {
            println!(
                "  fan policy: @0x{:X} entry_len={}",
                fp.table_offset, fp.entry_len
            );
            for (i, curve) in fp.curves.iter().enumerate() {
                let pts: Vec<String> = curve
                    .iter()
                    .map(|p| format!("{:.1}C->{}rpm", p.temp_c, p.rpm))
                    .collect();
                println!("    curve[{i}]: {}", pts.join(", "));
            }
            if fp.curves.iter().all(|c| c.is_empty()) {
                for e in &fp.raw_entries {
                    println!(
                        "    {}",
                        e.iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                }
            }
        }
        match nvoc_core::legacy_vbios_parser::find_falcon_table(&data) {
            Ok(Some(f)) => {
                println!("  falcon: @0x{:X} raw={:#x}", f.table_offset, f.raw_ptr);
                for e in &f.entries {
                    println!(
                        "    app={:?}({:#x}) target={:?} desc={:?}",
                        e.application,
                        e.application_id,
                        e.target,
                        e.desc
                            .as_ref()
                            .map(|d| (d.has_version_crypt, d.stored_size))
                    );
                }
            }
            Ok(None) => println!("  falcon: None"),
            Err(e) => println!("  falcon: err {e}"),
        }
        if let Some(n) = nvoc_core::legacy_vbios_parser::parse_nvgi(&data) {
            println!(
                "  nvgi: version={:#x} total={:#x} sub={:#x}:{:#x}",
                n.version, n.total_data_size, n.xve_sub_vendor, n.xve_subsystem_id
            );
        }
        match nvoc_core::legacy_vbios_parser::find_thermal(&data) {
            Ok(Some(th)) => println!(
                "  thermal: ver={:#x} mode={:?} duty={:?}/{:?} linear={:?}..{:?} pwm={:?} unknown={:?}",
                th.ver,
                th.fan_mode.map(|m| m.name()),
                th.min_duty,
                th.max_duty,
                th.linear_min_temp,
                th.linear_max_temp,
                th.pwm_freq,
                th.unknown_entries.iter().map(|u| u.tag).collect::<Vec<_>>()
            ),
            Ok(None) => println!("  thermal: pointer zero (absent)"),
            Err(e) => println!("  thermal: err {e}"),
        }
        if let Some(b) = nvoc_core::vbios_dcb::DcbBlock::primary(&data) {
            println!("  dcb: @0x{:X} conn @0x{:X}", b.offset, b.connector_offset);
            for p in b.display_map().iter().filter(|p| !p.roles.is_empty()) {
                println!(
                    "    {} = {:?} (conns: {:?})",
                    p.name,
                    p.roles,
                    p.connectors
                        .iter()
                        .map(|c| (c.index, c.type_code))
                        .collect::<Vec<_>>()
                );
            }
        }
    }
}
