//! locate_root 活体链路追踪(只读诊断,2026-10-07)。
//!
//! 复现 `core/src/kmd/power.rs::locate_root` 的完整链路,但**不做任何过滤**:
//! 每个 GPU 表条目的 major/root/init/key/UPPER/LOWER/base/amount 全部打印,
//! 并逐条标注身份门四条件各自的判定 —— 用于诊断 root 臂挂在哪一节。
//!
//! 2026-10-07 增补(桌面 board 臂配套,全部只读):
//! 1. board 链固定偏移 dump(610 家族诊断值,静态逆向来源,见
//!    `kmd-power-policy-layout-r610-74.md` §2/§3;仅诊断用,不进写路径);
//! 2. 活体窗三元组(tgp_watt_range + tgp_watt_status,GET 全安全);
//! 3. Board 窗扫查(`kmd::board::locate_board_window_candidates`,与写臂同一
//!    实现):root 对象 8 页 + 内核指针一跳,打印全部候选 + max 槽邻域 hexdump,
//!    供歧义时人工判读。
//!
//! 读取量:全局槽/表指针/count + 每条目 ~10 次 4-8 字节读 + board 扫查
//! ≤520 页(root 8 页 + 指针一跳 512 上限),远低于 BSOD 教训的 2000 页预算。
//! 前置:PMXDRV 传输在位(\\.\PMXDRV 可连)+ 管理员令牌。
//! #![ignore]:需要活体环境,显式 --ignored 运行。
#![cfg(windows)]

use nvoc_core::kmd::board;
use nvoc_core::kmd::layout_probe::probe as probe_layout;
use nvoc_core::kmd::pagewalk::{
    PeFingerprint, discover_root, find_loaded_module, read_virtual, translate,
};
use nvoc_core::kmd::pmxdrv::{PmxDrv, PmxDrvPhysMem};

fn is_kernel_va(v: u64) -> bool {
    (0xFFFF_8000_0000_0000..=0xFFFF_F7FF_FFFF_F000).contains(&v)
}

#[test]
#[ignore = "requires PMXDRV transport + elevated shell; read-only"]
fn locate_root_trace_live() {
    // ---- 与 connect_lane 相同的前置:模块 → 在役镜像 → probe → 走查根
    let module =
        find_loaded_module("nvlddmkm.sys").expect("nvlddmkm.sys not in system module list");
    println!(
        "nvlddmkm: base 0x{:016X}, image {}",
        module.base,
        module.path.display()
    );
    let img = std::fs::read(&module.path).expect("failed to read in-service image");
    let layout = probe_layout(&img).expect("layout derivation failed");
    println!(
        "layout: slot={:#x} state+{:#x} count=+{:#x} Major=+{:#x} M→root={:#x} init={:#x} key={:#x} LOWER={:#x} UPPER={:#x}",
        layout.global_slot_rva,
        layout.state_table_off,
        layout.table_count_off,
        layout.entry_major_off,
        layout.major_root_off,
        layout.root_init_off,
        layout.root_key_off,
        layout.root_lower_off,
        layout.root_upper_off
    );
    let fingerprint =
        PeFingerprint::from_image(&img).expect("in-service image has invalid PE header");
    let drv = PmxDrv::connect().expect("PMXDRV connect failed (service must be running)");
    let pm = PmxDrvPhysMem::new(&drv);
    let discovery = discover_root(&pm, module.base, &fingerprint);
    for event in &discovery.events {
        println!("  {event}");
    }
    let walk_root = discovery.unique_root().expect("page-table root not unique");
    println!("walk root: 0x{walk_root:016X}");

    // ---- 链路逐节读出(不过滤,全打印)
    let rd =
        |va: u64, len: usize| -> Option<Vec<u8>> { read_virtual(&pm, walk_root, va, len).ok() };
    let rd_u64 = |va: u64| -> Option<u64> {
        rd(va, 8).map(|b| u64::from_le_bytes(b[0..8].try_into().unwrap()))
    };
    let rd_u32 = |va: u64| -> Option<u32> {
        rd(va, 4).map(|b| u32::from_le_bytes(b[0..4].try_into().unwrap()))
    };

    let slot_va = module.base + layout.global_slot_rva;
    let raw_slot = rd_u64(slot_va);
    println!("① global slot @{slot_va:016X} raw = {raw_slot:?}");
    let state = raw_slot.filter(|v| is_kernel_va(*v));
    println!(
        "   → state = {}",
        match state {
            Some(v) => format!("0x{v:016X}"),
            None => "✗ not a kernel pointer".into(),
        }
    );
    let Some(state) = state else { return };

    let table_raw = rd_u64(state + u64::from(layout.state_table_off));
    println!("② state+{:x} raw = {table_raw:?}", layout.state_table_off);
    let table = table_raw.filter(|v| is_kernel_va(*v));
    println!(
        "   → GPU table = {}",
        match table {
            Some(v) => format!("0x{v:016X}"),
            None => "✗ not a kernel pointer".into(),
        }
    );
    let Some(table) = table else { return };

    let count = rd_u32(table + u64::from(layout.table_count_off));
    println!("③ table+{:x} count = {count:?}", layout.table_count_off);
    let Some(count) = count else { return };
    if count == 0 || count > 32 {
        println!(
            "   ✗ count abnormal (locate_root would fail here with \"GPU table count abnormal\")"
        );
        return;
    }

    // 表头邻域留证:count 前后各一条 + entry0 槽位原值
    println!(
        "   table ID[0] @+{:x} = {:?} (dword)",
        layout.entry_id_off,
        rd_u32(table + u64::from(layout.entry_id_off))
    );

    let mut first_root_va: Option<u64> = None;
    for i in 0..count {
        println!("---- entry{i} ----");
        let major_va = table
            + u64::from(layout.entry_major_off)
            + u64::from(i) * u64::from(layout.entry_stride);
        let major = rd_u64(major_va);
        println!(
            "  Major slot @{major_va:016X} = {}",
            match major {
                Some(v) => format!(
                    "0x{v:016X}{}",
                    if is_kernel_va(v) {
                        ""
                    } else {
                        "  ✗ not a kernel pointer"
                    }
                ),
                None => "✗ read failed".into(),
            }
        );
        let Some(major) = major.filter(|v| is_kernel_va(*v)) else {
            continue;
        };

        let root_slot_va = major + u64::from(layout.major_root_off);
        let root_va = rd_u64(root_slot_va);
        println!(
            "  [Major+{:x}] = {}",
            layout.major_root_off,
            match root_va {
                Some(v) => format!(
                    "0x{v:016X}{}",
                    if is_kernel_va(v) {
                        ""
                    } else {
                        "  ✗ not a kernel pointer"
                    }
                ),
                None => "✗ read failed".into(),
            }
        );
        let Some(root_va) = root_va.filter(|v| is_kernel_va(*v)) else {
            continue;
        };
        if first_root_va.is_none() {
            first_root_va = Some(root_va);
        }

        let init = rd_u32(root_va + u64::from(layout.root_init_off));
        let elig = rd_u32(root_va + u64::from(layout.root_init_off + 1));
        let amount_active = rd_u32(root_va + u64::from(layout.root_init_off + 2));
        let base = rd_u32(root_va + u64::from(layout.root_base_off));
        let amount = rd_u32(root_va + u64::from(layout.root_amount_off));
        let key = rd_u32(root_va + u64::from(layout.root_key_off));
        let lower = rd_u32(root_va + u64::from(layout.root_lower_off));
        let upper = rd_u32(root_va + u64::from(layout.root_upper_off));
        let aux1 = rd_u32(root_va + u64::from(layout.root_upper_off + 4));
        let aux2 = rd_u32(root_va + u64::from(layout.root_upper_off + 8));
        println!(
            "  root @0x{root_va:016X}: init={init:?} elig={elig:?} aa={amount_active:?} base={base:?} amount={amount:?} key={key:?} LOWER={lower:?} UPPER={upper:?} aux={aux1:?}/{aux2:?}"
        );
        // 身份门逐条判定(与 power.rs locate_root 同判据)
        let g1 = init.map(|v| v & 0xFF) == Some(1);
        let g2 = key.map(|v| v & 0xFF) < Some(0x40);
        let g3 = upper.is_some_and(|u| (10_000..=500_000).contains(&u));
        let g4 = lower.is_none_or(|l| l <= upper.unwrap_or(u32::MAX));
        println!(
            "  identity gate: init==1 {} | key<0x40 {} | UPPER∈[10W,500W] {} | LOWER≤UPPER {} → {}",
            mark(g1),
            mark(g2),
            mark(g3),
            mark(g4),
            if g1 && g2 && g3 && g4 {
                "pass ✓"
            } else {
                "fail ✗"
            }
        );
        let fr = translate(&pm, walk_root, root_va)
            .map(|pa| pa & !0xFFF)
            .ok();
        let fru = translate(&pm, walk_root, root_va + u64::from(layout.root_upper_off))
            .map(|pa| pa & !0xFFF)
            .ok();
        println!("  frames: root=0x{:?} upper=0x{:?}", fr, fru);

        // ---- board 链固定偏移 dump(610 家族诊断;root 臂 0x1C90 应为表指针,
        //      台式实测 0x2D000001 = 板注册表从未被构造期填充的活体证据)
        let o = |off: u64| root_va + off;
        println!(
            "  board chain: [root+1C90]={:?} (u64) [root+1CC8]={:?} (lookup fn{}) [root+2508]={:?}",
            rd_u64(o(0x1C90)),
            rd_u64(o(0x1CC8)),
            match rd_u64(o(0x1CC8)) {
                Some(p)
                    if (module.base..module.base + u64::from(fingerprint.size_of_image))
                        .contains(&p) =>
                    format!("(image RVA {:#x})", p - module.base),
                _ => String::new(),
            },
            rd_u64(o(0x2508)),
        );
        if let Some(bytes) = rd(o(0x1CA0), 16) {
            println!("  bitmap [1CA0..1CB0]: {}", hex(&bytes));
        }
        if let Some(bytes) = rd(o(0x268C), 48) {
            println!(
                "  selector map [268C..26BC]: {} (construction-time selector bytes [268F]={:?} [2694]={:?})",
                hex(&bytes),
                rd(o(0x268F), 1).map(|b| b[0]),
                rd(o(0x2694), 1).map(|b| b[0]),
            );
        }
    }

    // ---- 活体窗三元组(GET 面,全安全)+ Board 窗扫查(与写臂同实现)
    let Some(root_va) = first_root_va else {
        println!("no valid root — skipping board scan");
        return;
    };
    let Some(gpu) = nvapi::PhysicalGpu::enumerate()
        .ok()
        .and_then(|g| g.into_iter().next())
        .map(nvapi::hi::Gpu::new)
    else {
        println!("NVAPI GPU unreachable — skipping board scan");
        return;
    };
    let range = gpu.tgp_watt_range().ok().flatten();
    let status = gpu.tgp_watt_status().ok().flatten();
    println!("live window: range={range:?} status={status:?}");
    let (Some(default_mw), Some(max_mw)) = (
        range.as_ref().and_then(|r| r.default_mw),
        range.as_ref().and_then(|r| r.max_mw),
    ) else {
        println!("tgp window incomplete (default/max missing) — skipping board scan");
        return;
    };
    let live = board::BoardLiveValues {
        current_mw: status.and_then(|s| s.current_mw).unwrap_or(default_mw),
        default_mw,
        max_mw,
        min_mw: range.and_then(|r| r.min_mw),
    };
    println!(
        "Board window scan (root object 8 pages + pointer one-hop, budget {} pages)…",
        board::PAGE_BUDGET
    );
    let scan = board::locate_board_window_candidates(&pm, walk_root, root_va, &live);
    println!(
        "scan: {} pages readable / {} skipped / {} candidates",
        scan.pages_scanned,
        scan.pages_unreadable,
        scan.candidates.len()
    );
    for note in &scan.notes {
        println!("  [note] {note}");
    }
    for cand in &scan.candidates {
        println!(
            "candidate: page 0x{:016X} (frame 0x{:X}) max@+{:#x} cur@{:?} def@+{:#x} min@{:?} span {}B",
            cand.page_va,
            cand.frame,
            cand.hit.max_off,
            cand.hit.cur_off,
            cand.hit.def_off,
            cand.hit.min_off,
            cand.hit.span
        );
        // max 槽邻域 hexdump(±0x20,人工判读结构用)
        let lo = cand.page_va + cand.hit.max_off.saturating_sub(0x20) as u64;
        if let Some(bytes) = rd(lo, 0x50) {
            println!("  max neighborhood [{lo:#x}..+{:#x}]:", bytes.len());
            for (row, chunk) in bytes.chunks(16).enumerate() {
                println!(
                    "    +{:04x}: {}",
                    lo as usize % 0x1000 + row * 16,
                    hex(chunk)
                );
            }
        }
    }
    // ---- 值共现地图(宽记录行 >MATCH_SPAN 的定位手段;基本域 = root + 指针一跳)
    let (worklist, _domain_notes) = board::scan_domain_pages(&pm, walk_root, root_va);
    let co = board::value_cooccurrence_scan(&pm, walk_root, &worklist, &live, board::PAGE_BUDGET);
    println!(
        "value co-occurrence: {} pages with ≥2 live values (first 12)",
        co.len()
    );
    for hit in co.iter().take(12) {
        let vals = hit
            .values
            .iter()
            .map(|(v, offs)| {
                let os = offs
                    .iter()
                    .map(|o| format!("{o:#x}"))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{v}@[{os}]")
            })
            .collect::<Vec<_>>()
            .join("  ");
        println!("  page {:#016X}: {}", hit.page_va, vals);
    }
    if co.len() > 12 {
        println!(
            "  …(+{} pages omitted, needs a narrower criterion)",
            co.len() - 12
        );
    }
    // ---- 单位变体(µW = ×1000):FIFO 寄存器族的常见存储单位,值签名全家桶
    let mut live_uw = live.clone();
    live_uw.current_mw = live.current_mw.saturating_mul(1000);
    live_uw.default_mw = live.default_mw.saturating_mul(1000);
    live_uw.max_mw = live.max_mw.saturating_mul(1000);
    live_uw.min_mw = live.min_mw.map(|v| v.saturating_mul(1000));
    let co_uw =
        board::value_cooccurrence_scan(&pm, walk_root, &worklist, &live_uw, board::PAGE_BUDGET);
    println!("µW variant co-occurrence: {} pages (first 8)", co_uw.len());
    for hit in co_uw.iter().take(8) {
        let vals = hit
            .values
            .iter()
            .map(|(v, offs)| {
                let os = offs
                    .iter()
                    .map(|o| format!("{o:#x}"))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{v}@[{os}]")
            })
            .collect::<Vec<_>>()
            .join("  ");
        println!("  page {:#016X}: {}", hit.page_va, vals);
    }

    // ---- 仅含 max 值的页(钳表可能只有窗顶无邻位;排除共现已报页)
    let co_pages: Vec<u64> = co.iter().map(|h| h.page_va).collect();
    let max_only = board::max_only_scan(
        &pm,
        walk_root,
        &worklist,
        &live,
        board::PAGE_BUDGET,
        &co_pages,
    );
    println!(
        "max-only pages: {} (first 8, co-occurrence pages excluded)",
        max_only.len()
    );
    for (pg, offs) in max_only.iter().take(8) {
        let os = offs
            .iter()
            .map(|o| format!("{o:#x}"))
            .collect::<Vec<_>>()
            .join(",");
        println!("  page {pg:#016X}: max@[{os}]");
    }
    match scan.candidates.len() {
        1 => println!("unique candidate ✓"),
        0 => println!(
            "no candidates (run a diff: nvidia-smi -pl <in-window non-default> to perturb current and rescan — the live row's current will follow)"
        ),
        n => println!(
            "{n} candidates (echo/mirror indistinguishable from the live row statically — the write arm probes GET-follow one by one, follower wins; neighborhood dumps above for manual triage)"
        ),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn mark(ok: bool) -> &'static str {
    if ok { "✓" } else { "✗" }
}
