//! regtab(= fifoctx)活体定位探针(只读,2026-10-08)。
//!
//! 591.86 静态定案(nvlddmkm__591.86.sys,regWrite032 = sub_14011FAD0 汇编实证
//! `mov rbx,[rcx]` 后 `mov r14,[rbx+4280h]`):
//! - `*(obj+0x4280)` = fifoctx 指针;fifoctx 本体即 RM regtab ctx
//!   (修正旧结论"再解一层 +0x4280":region 表直接挂在 fifoctx 上)
//! - `fifoctx+0x1F00+8*i`(i=0..12)= 通道 region 指针表(sub_140309300,
//!   chanIdx≥13 拒;chan0 未建时惰性建为 fifoctx+0xDB8)
//! - 慢路径写 = sub_140309BB0 按 (chan, subchan, addr∈[lo,hi]) 走 region+40
//!   回调链;快速路径 = fifoctx+0x8AB flag → +0x20E0 backend → +0x3B50
//!   wrapper → +0x50 reg 对象 vtable[+64] write32(addr,val)
//!
//! chan0 自指签名(零额外读,同页判):页 P 内 qword[i] == P + i*8 − 0x1148
//! (0x1F00−0xDB8;fifoctx 页对齐时成立,非对齐漏检,靠值共现兜底)。
//!
//! 三步:A 宽表锚发现(root 一跳域共现)→ B 扩展域扫描(root/Major/宽表三锚
//! 指针一跳,同池 VA 距离优先,每页双判:值共现 + chan0 自指)→ C fifoctx
//! 候选 dump(13 region 指针 + 各 region 首 4KB TGP 值共现 + region+40 回调链)。
//! 预算:发现轮 ≤768 + 扩展域 ≤1024 + dump ≤64,总 ≤1856 < 2000(BSOD 纪律)。
//! 前置:PMXDRV 传输在位 + 管理员令牌。#cfg(ignore):需活体环境。
#![cfg(windows)]

use std::collections::HashSet;

use nvoc_core::kmd::board::{self, BoardLiveValues};
use nvoc_core::kmd::layout_probe::probe as probe_layout;
use nvoc_core::kmd::pagewalk::{PeFingerprint, discover_root, find_loaded_module, read_virtual};
use nvoc_core::kmd::pmxdrv::{PmxDrv, PmxDrvPhysMem};

const CHAN_TABLE_OFF: u64 = 0x1F00;
const CHAN_INLINE_OFF: u64 = 0xDB8;
const CHAN_COUNT: usize = 13;
const ANCHOR_PAGES_PER_OBJ: u64 = 8;
const EXTEND_CAP: usize = 1024;
const DUMP_BUDGET: usize = 280;
const TOTAL_BUDGET: usize = 1856;

fn is_kernel_va(v: u64) -> bool {
    (0xFFFF_8000_0000_0000..=0xFFFF_F7FF_FFFF_F000).contains(&v)
}

#[test]
#[ignore = "requires PMXDRV transport + elevated shell; read-only"]
fn regtab_probe_live() {
    let module = find_loaded_module("nvlddmkm.sys").expect("nvlddmkm.sys 不在系统模块列表");
    println!(
        "nvlddmkm: 基址 0x{:016X}, 磁盘 {}",
        module.base,
        module.path.display()
    );
    let img = std::fs::read(&module.path).expect("读在役镜像失败");
    let layout = probe_layout(&img).expect("布局推导失败");
    let fingerprint = PeFingerprint::from_image(&img).expect("在役镜像 PE 头无效");
    let drv = PmxDrv::connect().expect("PMXDRV 连接失败(服务须运行)");
    let pm = PmxDrvPhysMem::new(&drv);
    let discovery = discover_root(&pm, module.base, &fingerprint);
    let walk_root = discovery.unique_root().expect("页表根不唯一");
    println!("走查根: 0x{walk_root:016X}");

    let rd =
        |va: u64, len: usize| -> Option<Vec<u8>> { read_virtual(&pm, walk_root, va, len).ok() };
    let rd_u64 = |va: u64| -> Option<u64> {
        rd(va, 8).map(|b| u64::from_le_bytes(b[0..8].try_into().unwrap()))
    };

    // ---- 链:slot → state → table → entry0.major → root
    let Some(state) = rd_u64(module.base + layout.global_slot_rva).filter(|v| is_kernel_va(*v))
    else {
        println!("✗ 全局槽非内核指针 — 中止");
        return;
    };
    let Some(table) =
        rd_u64(state + u64::from(layout.state_table_off)).filter(|v| is_kernel_va(*v))
    else {
        println!("✗ state 表指针非内核指针 — 中止");
        return;
    };
    let Some(major) =
        rd_u64(table + u64::from(layout.entry_major_off)).filter(|v| is_kernel_va(*v))
    else {
        println!("✗ Major 槽非内核指针 — 中止");
        return;
    };
    let Some(root_va) =
        rd_u64(major + u64::from(layout.major_root_off)).filter(|v| is_kernel_va(*v))
    else {
        println!("✗ root 指针非内核指针 — 中止");
        return;
    };
    println!(
        "链: state=0x{state:016X} table=0x{table:016X} major=0x{major:016X} root=0x{root_va:016X}"
    );

    // ---- 活体窗
    let Some(gpu) = nvapi::PhysicalGpu::enumerate()
        .ok()
        .and_then(|g| g.into_iter().next())
        .map(nvapi::hi::Gpu::new)
    else {
        println!("NVAPI GPU 不可达 — 中止");
        return;
    };
    let range = gpu.tgp_watt_range().ok().flatten();
    let status = gpu.tgp_watt_status().ok().flatten();
    println!("活体窗: range={range:?} status={status:?}");
    let Some(default_mw) = range.as_ref().and_then(|r| r.default_mw) else {
        println!("窗 default 缺失 — 中止");
        return;
    };
    let live = BoardLiveValues {
        current_mw: status.and_then(|s| s.current_mw).unwrap_or(default_mw),
        default_mw,
        max_mw: range.as_ref().and_then(|r| r.max_mw).unwrap_or(default_mw),
        min_mw: range.and_then(|r| r.min_mw),
    };
    println!(
        "目标值: min={:?} def={} max={} cur={}",
        live.min_mw, live.default_mw, live.max_mw, live.current_mw
    );

    // ---- A. 宽表锚发现:root 一跳域共现(现有实现,自带命中邻域轮)
    let (root_worklist, notes) = board::scan_domain_pages(&pm, walk_root, root_va);
    for n in &notes {
        println!("  [note] {n}");
    }
    println!("A. 宽表发现轮(root 一跳域,{} 页)…", root_worklist.len());
    let co = board::value_cooccurrence_scan(&pm, walk_root, &root_worklist, &live, 768);
    println!("A. 共现 {} 页", co.len());
    for hit in &co {
        println!("  {}", fmt_hit(hit));
    }
    let wide_pages: Vec<u64> = co.iter().map(|h| h.page_va).take(4).collect();

    // ---- B. 扩展域:三锚(root/Major/宽表)各 8 页 + 指针一跳,同池距离优先
    let mut anchor_pages: Vec<u64> = Vec::new();
    for base in [root_va, major] {
        for i in 0..ANCHOR_PAGES_PER_OBJ {
            anchor_pages.push(base + i * 0x1000);
        }
    }
    anchor_pages.extend(wide_pages.iter().copied());
    anchor_pages.sort_unstable();
    anchor_pages.dedup();

    let mut targets: Vec<u64> = Vec::new();
    for &pg in &anchor_pages {
        let Some(page) = rd(pg, 4096) else { continue };
        for ptr in board::collect_kernel_pointers(&page) {
            let target = ptr & !0xFFF;
            if !anchor_pages.contains(&target) {
                targets.push(target);
            }
        }
    }
    targets.sort_unstable();
    targets.dedup();
    // 同池先验:池分配聚集,离锚集 VA 距离近的先扫
    let anchors_for_dist = [root_va, major];
    targets.sort_by_key(|t| {
        anchors_for_dist
            .iter()
            .map(|a| t.abs_diff(a & !0xFFF))
            .min()
            .unwrap_or(u64::MAX)
    });
    if targets.len() > EXTEND_CAP {
        println!(
            "B. 一跳目标 {} 截断到 {EXTEND_CAP}(同池距离优先)",
            targets.len()
        );
        targets.truncate(EXTEND_CAP);
    }
    println!(
        "B. 扩展域:锚 {} 页 + 一跳 {} 页,双判扫描…",
        anchor_pages.len(),
        targets.len()
    );

    let mut scanned = 0usize;
    let mut unreadable = 0usize;
    let mut co_ext: Vec<board::CooccurrenceHit> = Vec::new();
    let mut fifo_candidates: Vec<u64> = Vec::new();
    let mut seen: HashSet<u64> = HashSet::new();
    'outer: for round in 0..2 {
        let pages: &[u64] = if round == 0 { &anchor_pages } else { &targets };
        for &pg in pages {
            if scanned >= EXTEND_CAP || scanned >= TOTAL_BUDGET {
                break 'outer;
            }
            if !seen.insert(pg) {
                continue;
            }
            let Ok(page) = read_virtual(&pm, walk_root, pg, 4096) else {
                unreadable += 1;
                continue;
            };
            scanned += 1;
            // 判 1:chan0 自指(零额外读)
            let qwords: Vec<u64> = page
                .chunks_exact(8)
                .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
                .collect();
            for (i, &v) in qwords.iter().enumerate() {
                // 自指:S(槽址) = v + 0x1148 ⇔ v == S - 0x1148;fifoctx = v − 0xDB8
                if is_kernel_va(v)
                    && v > CHAN_INLINE_OFF
                    && v == pg + (i as u64) * 8 - (CHAN_TABLE_OFF - CHAN_INLINE_OFF)
                {
                    let fifo = v - CHAN_INLINE_OFF;
                    println!(
                        "  fifoctx 候选(自指): 槽 0x{:016X} +{:#x} → fifoctx 0x{fifo:016X}",
                        pg + (i as u64) * 8,
                        i as u64 * 8
                    );
                    fifo_candidates.push(fifo);
                }
            }
            // 判 2:值共现
            if let Some(hit) = page_hit(pg, &page, &live) {
                println!("  共现: {}", fmt_hit(&hit));
                co_ext.push(hit);
            }
        }
    }
    println!(
        "B. 扫描 {scanned} 页(不可读 {unreadable});扩展域共现 {} 页,fifoctx 候选 {}",
        co_ext.len(),
        fifo_candidates.len()
    );

    // 候选去重 + 上限
    fifo_candidates.sort_unstable();
    fifo_candidates.dedup();
    fifo_candidates.truncate(4);

    // ---- C. fifoctx dump:13 region 指针 + 各 region 首 4KB 值 + 回调链
    let mut dump_reads = 0usize;
    for &fc in &fifo_candidates {
        println!("---- fifoctx 0x{fc:016X} ----");
        println!(
            "  fast-path flag [+8AB] = {:?}(1=backend vtable 直写生效)",
            rd(fc + 0x8AB, 1).map(|b| b[0])
        );
        println!(
            "  backend [+20E0] = {:?}  wrapper[+3B50 链尾 +50] 见静态链",
            rd_u64(fc + 0x20E0)
        );
        // backend 对象首页值共现(fifoctx-0x400 邻域;若 TGP 值在此,深采样转向 backend)
        if let Some(be) = rd_u64(fc + 0x20E0).filter(|v| is_kernel_va(*v)) {
            dump_reads += 1;
            if let Some(page) = rd(be, 4096) {
                let hits = page_tgp_hits(&page, &live);
                println!(
                    "  backend 首页 0x{be:016X} TGP 命中: {}",
                    if hits.is_empty() {
                        "无".into()
                    } else {
                        hits.join("  ")
                    }
                );
            } else {
                println!("  backend 首页不可读");
            }
        }
        for i in 0..CHAN_COUNT {
            let Some(rp) = rd_u64(fc + CHAN_TABLE_OFF + (i as u64) * 8) else {
                println!("  chan{i:2}: 读失败");
                continue;
            };
            dump_reads += 1;
            if rp == 0 {
                println!("  chan{i:2}: 0(未建)");
                continue;
            }
            let inline = rp == fc + CHAN_INLINE_OFF;
            println!(
                "  chan{i:2}: 0x{rp:016X}{}",
                if inline { " (=内联 chan0)" } else { "" }
            );
            if dump_reads >= DUMP_BUDGET {
                println!("  [dump 预算尽,停]");
                break;
            }
            let Some(page) = rd(rp, 4096) else {
                println!("       region 首页不可读");
                dump_reads += 1;
                continue;
            };
            dump_reads += 1;
            let mut found = 0usize;
            for (off, win) in page.windows(4).enumerate() {
                let v = u32::from_le_bytes(win.try_into().unwrap());
                if [
                    live.min_mw,
                    Some(live.default_mw),
                    Some(live.max_mw),
                    Some(live.current_mw),
                ]
                .into_iter()
                .flatten()
                .any(|t| t == v)
                {
                    println!("       +{off:#06x} = {v}(TGP 值命中)");
                    found += 1;
                    if found >= 8 {
                        break;
                    }
                }
            }
            // 深采样:shadow slot = region + regaddr(regaddr 量级见回调 addr 范围,
            // 0x400000+;sub_1400E3530 实证 *(region + 4*(addr>>2)) = val)。
            // 默认每 0x40000 采样一页(64 采样/16MB);REGTAB_DENSE=1 时步长
            // 0x10000(256 采样/16MB,差分轮用 —— 读数 +192 页仍在总预算内)。
            // 命中打印 regaddr(即 slot 偏移)。
            let stride: u64 = if std::env::var("REGTAB_DENSE").as_deref() == Ok("1") {
                0x10000
            } else {
                0x40000
            };
            let steps = 0x1_000_000 / stride;
            if inline {
                for k in 1..steps {
                    if dump_reads >= DUMP_BUDGET {
                        println!("       [深采样预算尽 @ {:#x}]", k * stride);
                        break;
                    }
                    let va = rp + k * stride;
                    let Some(sp) = rd(va, 4096) else {
                        continue;
                    };
                    dump_reads += 1;
                    for (off, win) in sp.chunks_exact(4).enumerate() {
                        let v = u32::from_le_bytes((*win).try_into().unwrap());
                        if [
                            live.min_mw,
                            Some(live.default_mw),
                            Some(live.max_mw),
                            Some(live.current_mw),
                        ]
                        .into_iter()
                        .flatten()
                        .any(|t| t == v)
                        {
                            println!(
                                "       shadow+{:#x}(+{:#x} 页内) = {v}(regaddr ≈ {:#x})",
                                k * stride + (off * 4) as u64,
                                off * 4,
                                k * stride + (off * 4) as u64
                            );
                            found += 1;
                            if found >= 12 {
                                break;
                            }
                        }
                    }
                    if found >= 12 {
                        break;
                    }
                }
            }
            // region+40 回调链(寄存器写拦截;节点 {next, w8, mask+8, chan+12, sub+16, lo+20, hi+24, fn+32})
            if let Some(head) = rd_u64(rp + 40) {
                let mut node = head;
                let mut n = 0;
                while is_kernel_va(node) && n < 4 {
                    let Some(b) = rd(node, 40) else { break };
                    dump_reads += 1;
                    let g = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
                    let d = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
                    println!(
                        "       cb@0x{node:016X}: w8={:x} chan={} sub={} addr=[{:#x},{:#x}] fn={:?}",
                        g(8),
                        d(12),
                        d(16),
                        d(20),
                        d(24),
                        g(32)
                            .checked_sub(module.base)
                            .map(|r| format!("RVA {r:#x}"))
                            .unwrap_or_else(|| format!("{:#x}", g(32)))
                    );
                    node = g(0);
                    n += 1;
                }
            }
            if dump_reads >= DUMP_BUDGET {
                println!("  [dump 预算尽,停]");
                break;
            }
        }
    }
    println!("总计读页: 发现轮 ≤768 + 扩展 {scanned} + dump {dump_reads}(预算 {TOTAL_BUDGET})");
    if co_ext.is_empty() && fifo_candidates.is_empty() {
        println!(
            "双判全空:region/fifoctx 不在锚一跳域。下一轮:对 B 轮命中邻域外扩 ±64 页,或把宽表页 VA 以 env 传入作第四锚。"
        );
    }
}

fn page_tgp_hits(page: &[u8], live: &BoardLiveValues) -> Vec<String> {
    let targets: Vec<u32> = [
        live.min_mw,
        Some(live.default_mw),
        Some(live.max_mw),
        Some(live.current_mw),
    ]
    .into_iter()
    .flatten()
    .collect();
    let mut out = Vec::new();
    for (off, win) in page.chunks_exact(4).enumerate() {
        let v = u32::from_le_bytes(win.try_into().unwrap());
        if targets.contains(&v) {
            out.push(format!("+{off:#x}={v}"));
            if out.len() >= 16 {
                break;
            }
        }
    }
    out
}

fn page_hit(pg: u64, page: &[u8], live: &BoardLiveValues) -> Option<board::CooccurrenceHit> {
    let mut targets: Vec<u32> = vec![live.current_mw, live.default_mw, live.max_mw];
    targets.extend(live.min_mw);
    targets.sort_unstable();
    targets.dedup();
    let mut values: Vec<(u32, Vec<usize>)> = Vec::new();
    for &v in &targets {
        let offs: Vec<usize> = page
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, c)| u32::from_le_bytes((*c).try_into().unwrap()) == v)
            .map(|(i, _)| i * 4)
            .collect();
        if !offs.is_empty() {
            values.push((v, offs));
        }
    }
    (values.len() >= 2).then_some(board::CooccurrenceHit {
        page_va: pg,
        values,
    })
}

fn fmt_hit(hit: &board::CooccurrenceHit) -> String {
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
    format!("页 {:#016X}: {}", hit.page_va, vals)
}
