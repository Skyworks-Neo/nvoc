//! Board 窗定位 —— 桌面形态破解臂的目标搜索逻辑(纯算法,跨平台可单测)。
//!
//! 台式形态(无 board 配置对象)PowerRoot 永不武装(init=0/key=0/UPPER=0,
//! 见 `kmd-power-policy-layout-r610-74.md` §6 与 desktop-2070 台账):root 的
//! UPPER 字段在桌面上没有任何观察者,破解目标换成 **Board 控制表的滑条窗
//! (max)** —— percent(0xAD95F5ED)/NVML 写路径的窗钳源头。本模块不做任何
//! 写:只按「同页三元组」判据(任务书 §3.2 消歧器)在 root 对象页 + 其内核
//! 指针一跳目标里搜 {current, default, max} 活体值,唯一候选才放行写臂。
//!
//! 定位与驱动版本无关(纯值签名,零布局硬编码);唯一引用的"布局"是 root
//! 对象起始 VA(来自 layout_probe 的 GPU 链)与其扫查页跨度。

use std::collections::HashSet;

use super::pagewalk::{PhysicalMemory, is_kernel_pointer, read_virtual, translate};

/// 活体窗三元组(GET 面 0x67F31384 range + 0x8B3E7343 status,全部安全命令)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardLiveValues {
    /// 当前控制值(滑条 last write;status 的 current,None 时以 default 代)。
    pub current_mw: u32,
    /// 出厂默认(= tgp_watt_range.default_mw)。
    pub default_mw: u32,
    /// 滑条窗顶(= tgp_watt_range.max_mw,Board 窗臂的写目标槽现值)。
    pub max_mw: u32,
    /// 窗底(可选;部分卡不暴露)。
    pub min_mw: Option<u32>,
}

/// 一次同页命中(页内偏移):活体控制行 = {max, current, default} 全对上
/// (cur Some);info 行(静态策略行,无 control 槽)= {max, default(+min)}
/// 降级匹配(cur None)。写臂先探控制行再探 info 行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardWindowMatch {
    pub max_off: usize,
    /// control 槽命中(活体行 Some;info 行 None)。
    pub cur_off: Option<usize>,
    pub def_off: usize,
    /// min 槽命中(可选;同值槽不与 cur/def 复用同一 dword)。
    pub min_off: Option<usize>,
    /// 命中字段的跨度(hi-lo,≤ [`MATCH_SPAN`])。
    pub span: usize,
}

/// 三元组聚拢判据:全部命中字段的跨度上限(字节)。4060L root 五元组跨度
/// 0x14、Board 值行步距 ~0x10,取 0x20 为容差上限。
pub const MATCH_SPAN: usize = 0x20;

/// root 对象扫查页数:root_va 起连续 8 页(32 KiB),覆盖已知三代 root
/// 字段族(576: +0x1638 / 610: +0x3CE0..CF4 / 616.92: +0x3D10..D2C)。
pub const ROOT_OBJECT_PAGES: u64 = 8;

/// 指针展开目标页上限(root 对象页里的内核指针一跳)。
pub const POINTER_TARGET_CAP: usize = 512;

/// 候选页邻域扫查半径(±64 页):活体行常与 echo/lease 行同池分配,
/// 2026-10-07 桌面 2070 实测 echo cell(0xFE 标记行)非窗钳源,活体行
/// 未被指针一跳覆盖 —— 池邻域是下一个最高先验位置。
pub const NEIGHBORHOOD_PAGES: u64 = 64;

/// 邻域扫查页预算上限(总预算仍 ≤ BSOD 纪律 2000:520 + 768)。
pub const SWEEP_BUDGET: usize = 768;

/// 单次扫查页预算(BSOD 纪律 ≤2000;本臂 root 8 页 + 展开 512 页远低于线)。
pub const PAGE_BUDGET: usize = ROOT_OBJECT_PAGES as usize + POINTER_TARGET_CAP;

/// root 对象扫查页序列(root_va 页对齐起,连续 [`ROOT_OBJECT_PAGES`] 页)。
pub fn root_object_pages(root_va: u64) -> Vec<u64> {
    let base = root_va & !0xFFF;
    (0..ROOT_OBJECT_PAGES).map(|i| base + i * 0x1000).collect()
}

/// 单页扫查:控制行三元组优先,info 行降级({max, default(+min)},无
/// control 槽)。返回全部命中(多命中 = 歧义素材,由上层判)。
///
/// 值相撞语义:出厂未扰动时 current 常等于 default,位置集合相同 —— 靠
/// 「cur/def/min 必须取不同偏移」区分;三元全等时要求三处不同 dword 同聚拢,
/// 只有两处不算命中(噪声门)。
pub fn scan_page_for_window(page: &[u8], live: &BoardLiveValues) -> Vec<BoardWindowMatch> {
    let lim = page.len() / 4 * 4;
    let words: Vec<u32> = (0..lim)
        .step_by(4)
        .map(|o| u32::from_le_bytes(page[o..o + 4].try_into().expect("步长 4 对齐")))
        .collect();
    let pos = |v: u32| -> Vec<usize> {
        words
            .iter()
            .enumerate()
            .filter(|(_, w)| **w == v)
            .map(|(i, _)| i * 4)
            .collect()
    };
    let near = |list: &[usize], center: usize| -> Vec<usize> {
        list.iter()
            .copied()
            .filter(|o| o.abs_diff(center) <= MATCH_SPAN)
            .collect()
    };

    let cur_pos = pos(live.current_mw);
    let def_pos = pos(live.default_mw);
    let min_pos = live.min_mw.map(pos);
    let mut out = Vec::new();
    for m in pos(live.max_mw) {
        // 控制行三元组:{max, current, default} 互异同聚拢
        let mut best: Option<BoardWindowMatch> = None;
        for c in near(&cur_pos, m) {
            for d in near(&def_pos, m) {
                // 三个字段是不同槽位:值再怎么撞也不允许复用同一 dword
                if d == c || m == c || m == d {
                    continue;
                }
                let min_off = min_pos.as_ref().and_then(|list| {
                    near(list, m)
                        .into_iter()
                        .filter(|o| *o != c && *o != d)
                        .min_by_key(|o| o.abs_diff(m))
                });
                let mut offs = vec![m, c, d];
                offs.extend(min_off);
                let lo = *offs.iter().min().expect("非空");
                let hi = *offs.iter().max().expect("非空");
                let span = hi - lo;
                if span > MATCH_SPAN {
                    continue; // 全体字段必须聚拢,不接受两端拉伸
                }
                let cand = BoardWindowMatch {
                    max_off: m,
                    cur_off: Some(c),
                    def_off: d,
                    min_off,
                    span,
                };
                if best
                    .as_ref()
                    .is_none_or(|b: &BoardWindowMatch| cand.span < b.span)
                {
                    best = Some(cand);
                }
            }
        }
        if let Some(b) = best {
            out.push(b);
            continue;
        }
        // info 行降级:本页无 control 槽邻位,{max, default(+min)} 两/三元。
        // 三元全等退化态不降级 —— 那时任何两处同值 dword 都会假命中。
        let degenerate = live.current_mw == live.default_mw && live.default_mw == live.max_mw;
        let mut best2: Option<BoardWindowMatch> = None;
        if degenerate {
            continue;
        }
        for d in near(&def_pos, m) {
            if d == m {
                continue;
            }
            let min_off = min_pos.as_ref().and_then(|list| {
                near(list, m)
                    .into_iter()
                    .filter(|o| *o != m && *o != d)
                    .min_by_key(|o| o.abs_diff(m))
            });
            let mut offs = vec![m, d];
            offs.extend(min_off);
            let lo = *offs.iter().min().expect("非空");
            let hi = *offs.iter().max().expect("非空");
            let span = hi - lo;
            if span > MATCH_SPAN {
                continue;
            }
            let cand = BoardWindowMatch {
                max_off: m,
                cur_off: None,
                def_off: d,
                min_off,
                span,
            };
            if best2
                .as_ref()
                .is_none_or(|b: &BoardWindowMatch| cand.span < b.span)
            {
                best2 = Some(cand);
            }
        }
        if let Some(b) = best2 {
            out.push(b);
        }
    }
    // 同簇去重:全等三元组/值相撞时,同一字段集会以 (m,c,d) 的不同指派产生
    // 多条等价命中 —— 按排序字段集收敛为一条,真实的第二簇(不同字段集)保留。
    let mut seen: HashSet<Vec<Option<usize>>> = HashSet::new();
    out.retain(|hit| {
        let mut key: Vec<Option<usize>> = vec![Some(hit.max_off), hit.cur_off, Some(hit.def_off)];
        key.extend(hit.min_off.map(Some));
        key.sort_unstable();
        seen.insert(key)
    });
    out
}

/// 页内 8 字节对齐内核指针收集(指针展开用)。
pub fn collect_kernel_pointers(page: &[u8]) -> Vec<u64> {
    page.chunks_exact(8)
        .map(|c| u64::from_le_bytes(c.try_into().expect("8 对齐")))
        .filter(|v| is_kernel_pointer(*v))
        .collect()
}

/// 一页候选:页 VA + 物理帧 + 命中明细。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardWindowCandidate {
    pub page_va: u64,
    /// 物理帧(0 = 翻译失败,写臂会拒绝)。
    pub frame: u64,
    pub hit: BoardWindowMatch,
}

/// 扫查汇总(候选 + 预算记账 + 诊断 note,note 封顶 16 条防刷屏)。
#[derive(Debug, Default, Clone)]
pub struct BoardWindowScan {
    pub candidates: Vec<BoardWindowCandidate>,
    pub pages_scanned: usize,
    pub pages_unreadable: usize,
    pub notes: Vec<String>,
}

/// 逐页读 + 三元组扫查(跨调用共享 `seen` 去重、预算内;不可读页计数跳过)。
pub fn scan_pages<P: PhysicalMemory>(
    phys: &P,
    walk_root: u64,
    pages: &[u64],
    live: &BoardLiveValues,
    budget: usize,
    seen: &mut HashSet<u64>,
) -> BoardWindowScan {
    let mut scan = BoardWindowScan::default();
    for &page_va in pages {
        if scan.pages_scanned >= budget {
            scan.notes
                .push(format!("页预算 {budget} 耗尽,余 {} 页未扫", pages.len()));
            break;
        }
        if !seen.insert(page_va) {
            continue;
        }
        match read_virtual(phys, walk_root, page_va, 4096) {
            Ok(page) => {
                scan.pages_scanned += 1;
                for hit in scan_page_for_window(&page, live) {
                    let frame = translate(phys, walk_root, page_va)
                        .map(|pa| pa & !0xFFF)
                        .unwrap_or(0);
                    scan.candidates.push(BoardWindowCandidate {
                        page_va,
                        frame,
                        hit,
                    });
                }
            }
            Err(e) => {
                scan.pages_unreadable += 1;
                if scan.notes.len() < 16 {
                    scan.notes.push(format!("页 {page_va:#x} 不可读({e}),跳过"));
                }
            }
        }
    }
    scan
}

/// 域内工作表:root 对象页 + 其内核指针一跳目标(去重;扫查与值共现报告
/// 共用)。返回 (页列表, 截断等 note)。
pub fn scan_domain_pages<P: PhysicalMemory>(
    phys: &P,
    walk_root: u64,
    root_va: u64,
) -> (Vec<u64>, Vec<String>) {
    let root_pages = root_object_pages(root_va);
    let mut notes = Vec::new();
    // hop1:root 对象页里的内核指针 → 目标页(排除自指/对象页自身)
    let mut targets: Vec<u64> = Vec::new();
    for &pg in &root_pages {
        if let Ok(page) = read_virtual(phys, walk_root, pg, 4096) {
            for ptr in collect_kernel_pointers(&page) {
                let target = ptr & !0xFFF;
                if target == pg || root_pages.contains(&target) {
                    continue;
                }
                targets.push(target);
            }
        }
    }
    targets.sort_unstable();
    targets.dedup();
    if targets.len() > POINTER_TARGET_CAP {
        notes.push(format!(
            "指针目标 {} 超 CAP {POINTER_TARGET_CAP},截断(只扫前 {})",
            targets.len(),
            POINTER_TARGET_CAP
        ));
        targets.truncate(POINTER_TARGET_CAP);
    }
    let mut worklist = root_pages;
    worklist.extend(targets);
    (worklist, notes)
}

/// 值共现报告:含有 ≥2 种活体值的页(各值全部页内偏移)。宽记录值行
/// (步距 > [`MATCH_SPAN`],如 0x148 记录族)三元组/邻域判据抓不到 ——
/// 共现地图是它们的定位手段(2070/3060 判读用,read-only)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CooccurrenceHit {
    pub page_va: u64,
    /// (值, 该值全部页内偏移);按 distinct 值种数降序后按值升序。
    pub values: Vec<(u32, Vec<usize>)>,
}

pub fn value_cooccurrence_scan<P: PhysicalMemory>(
    phys: &P,
    walk_root: u64,
    pages: &[u64],
    live: &BoardLiveValues,
    budget: usize,
) -> Vec<CooccurrenceHit> {
    let mut targets: Vec<u32> = vec![live.current_mw, live.default_mw, live.max_mw];
    targets.extend(live.min_mw);
    targets.sort_unstable();
    targets.dedup();
    let mut seen: HashSet<u64> = HashSet::new();
    let mut hits: Vec<CooccurrenceHit> = Vec::new();
    let mut scanned = 0usize;
    loop {
        // 一轮 = 基本域(首轮)/ 命中页池邻域(后续轮):591.86 实测钳表不与
        // 宽表同页,同池分配是下一个先验位置(与行 sweep 同一教训)
        let mut round_pages: Vec<u64> = if hits.is_empty() {
            pages.to_vec()
        } else {
            let mut nbr: Vec<u64> = Vec::new();
            for h in &hits {
                for i in 1..=NEIGHBORHOOD_PAGES {
                    if let Some(lo) = h.page_va.checked_sub(i * 0x1000) {
                        nbr.push(lo);
                    }
                    if let Some(hi) = h.page_va.checked_add(i * 0x1000) {
                        nbr.push(hi);
                    }
                }
            }
            nbr.sort_unstable();
            nbr.dedup();
            nbr
        };
        let mut new_hit = false;
        for pg in round_pages.drain(..) {
            if scanned >= budget {
                break;
            }
            if !seen.insert(pg) {
                continue;
            }
            let Ok(page) = read_virtual(phys, walk_root, pg, 4096) else {
                continue;
            };
            scanned += 1;
            let mut values: Vec<(u32, Vec<usize>)> = Vec::new();
            for &v in &targets {
                let offs: Vec<usize> = page
                    .chunks_exact(4)
                    .enumerate()
                    .filter(|(_, c)| u32::from_le_bytes((*c).try_into().expect("4 对齐")) == v)
                    .map(|(i, _)| i * 4)
                    .collect();
                if !offs.is_empty() {
                    values.push((v, offs));
                }
            }
            if values.len() >= 2 {
                hits.push(CooccurrenceHit {
                    page_va: pg,
                    values,
                });
                new_hit = true;
            }
        }
        if !new_hit || scanned >= budget || hits.len() >= 16 {
            break;
        }
    }
    hits
}

/// 仅含 max 值的页(钳表可能只有窗顶、无 min/default 邻位 —— 行判据与
/// 共现都抓不到;591.86 判读用)。返回 (页 VA, max 全部偏移)。
pub fn max_only_scan<P: PhysicalMemory>(
    phys: &P,
    walk_root: u64,
    pages: &[u64],
    live: &BoardLiveValues,
    budget: usize,
    exclude: &[u64],
) -> Vec<(u64, Vec<usize>)> {
    let mut seen: HashSet<u64> = HashSet::new();
    let mut out = Vec::new();
    let mut scanned = 0usize;
    for &pg in pages {
        if scanned >= budget {
            break;
        }
        if exclude.contains(&pg) || !seen.insert(pg) {
            continue;
        }
        let Ok(page) = read_virtual(phys, walk_root, pg, 4096) else {
            continue;
        };
        scanned += 1;
        let offs: Vec<usize> = page
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, c)| u32::from_le_bytes((*c).try_into().expect("4 对齐")) == live.max_mw)
            .map(|(i, _)| i * 4)
            .collect();
        if !offs.is_empty() {
            out.push((pg, offs));
        }
    }
    out
}

/// 完整定位扫查,两阶段:
///
/// 1. root 对象页(直接扫)+ 其内核指针一跳目标(展开后扫),预算
///    [`PAGE_BUDGET`];
/// 2. 命中候选页的池邻域(±[`NEIGHBORHOOD_PAGES`] 页)再扫,预算
///    [`SWEEP_BUDGET`] —— 活体行常与 echo/lease 行同池而未被指针覆盖。
///
/// 返回全部候选,**不**做放行裁决(写臂做逐候选探测,人工判读走 trace
/// 工具打印全部)。
pub fn locate_board_window_candidates<P: PhysicalMemory>(
    phys: &P,
    walk_root: u64,
    root_va: u64,
    live: &BoardLiveValues,
) -> BoardWindowScan {
    let (worklist, notes) = scan_domain_pages(phys, walk_root, root_va);
    let mut seen: HashSet<u64> = HashSet::new();
    let mut scan = scan_pages(phys, walk_root, &worklist, live, PAGE_BUDGET, &mut seen);

    // phase2:候选页池邻域(±64 页)再扫,活体行与 echo 行同池是高先验位置
    let mut hit_pages: Vec<u64> = scan.candidates.iter().map(|c| c.page_va).collect();
    hit_pages.sort_unstable();
    hit_pages.dedup();
    if !hit_pages.is_empty() {
        let mut sweep: Vec<u64> = Vec::new();
        for pg in &hit_pages {
            for i in 1..=NEIGHBORHOOD_PAGES {
                if let Some(lo) = pg.checked_sub(i * 0x1000) {
                    sweep.push(lo);
                }
                if let Some(hi) = pg.checked_add(i * 0x1000) {
                    sweep.push(hi);
                }
            }
        }
        sweep.sort_unstable();
        sweep.dedup();
        scan.notes.push(format!(
            "邻域扫查: {} 个候选页 ±{} 页(sweep 预算 {SWEEP_BUDGET})",
            hit_pages.len(),
            NEIGHBORHOOD_PAGES
        ));
        let scan2 = scan_pages(phys, walk_root, &sweep, live, SWEEP_BUDGET, &mut seen);
        scan.pages_scanned += scan2.pages_scanned;
        scan.pages_unreadable += scan2.pages_unreadable;
        scan.candidates.extend(scan2.candidates);
        for note in scan2.notes {
            if scan.notes.len() < 16 {
                scan.notes.push(note);
            }
        }
    }
    scan.notes.splice(0..0, notes);
    scan
}

/// 写臂探测清单门:0 候选拒(窗表在扫查域之外);超过 `cap` 拒(歧义面
/// 失控,人工判读)。候选的逐个放行由写臂"写-GET 验-回滚"探测完成
/// (echo/lease 镜像行与活体行静态不可分,探测是唯一可靠的消歧器)。
pub fn probeable_candidates(
    scan: &BoardWindowScan,
    cap: usize,
) -> Result<&[BoardWindowCandidate], String> {
    match scan.candidates.len() {
        0 => Err(
            "无三元组候选:活体窗值与扫查页(root 对象 8 页 + 指针一跳 + 候选邻域)全不匹配 \
                  —— 先用 locate-trace 工具跑差分(current 扰动后再扫)扩大判据"
                .into(),
        ),
        n if n > cap => Err(format!(
            "{n} 个三元组候选超过探测上限 {cap} — 拒(用 locate-trace 打印各候选邻域后人工判读)"
        )),
        n => Ok(&scan.candidates[..n]),
    }
}

/// 写臂单轮探测的候选数上限。
pub const PROBE_CAP: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// 内存模拟物理内存(与 pagewalk tests 同构):4 KiB 页存取,缺席页不可读。
    struct MockPhysical {
        pages: HashMap<u64, [u8; 4096]>,
    }

    impl PhysicalMemory for MockPhysical {
        fn read(&self, pa: u64, out: &mut [u8]) -> Result<(), super::super::pagewalk::PhysError> {
            let page = self
                .pages
                .get(&(pa & !0xFFF))
                .ok_or(super::super::pagewalk::PhysError::Unreadable(pa))?;
            let offset = (pa & 0xFFF) as usize;
            match out.len() {
                4096 => {
                    if offset != 0 {
                        return Err(super::super::pagewalk::PhysError::BadLength(out.len()));
                    }
                    out.copy_from_slice(page);
                    Ok(())
                }
                8 => {
                    out.copy_from_slice(&page[offset..offset + 8]);
                    Ok(())
                }
                other => Err(super::super::pagewalk::PhysError::BadLength(other)),
            }
        }
    }

    fn set_pte(pages: &mut HashMap<u64, [u8; 4096]>, table: u64, index: u64, entry: u64) {
        let page = pages.entry(table).or_insert([0u8; 4096]);
        let at = (index * 8) as usize;
        page[at..at + 8].copy_from_slice(&entry.to_le_bytes());
    }

    /// 四级页表逐级建表映射 VA→帧,返回内容页句柄(帧基址)。
    fn map_page(
        pages: &mut HashMap<u64, [u8; 4096]>,
        root: u64,
        va: u64,
        frame: u64,
        next_table: &mut u64,
    ) {
        let mut table = root;
        for &shift in &[39u64, 30, 21] {
            let index = (va >> shift) & 0x1FF;
            let existing = pages
                .get(&table)
                .map(|p| {
                    u64::from_le_bytes(
                        p[(index * 8) as usize..(index * 8 + 8) as usize]
                            .try_into()
                            .expect("8 对齐"),
                    )
                })
                .unwrap_or(0);
            let next = if existing & 1 == 1 {
                existing & !0xFFF
            } else {
                let t = *next_table;
                *next_table += 0x1000;
                set_pte(pages, table, index, t | 0x3);
                t
            };
            table = next;
        }
        set_pte(pages, table, (va >> 12) & 0x1FF, frame | 0x3);
        pages.entry(frame & !0xFFF).or_insert([0u8; 4096]);
    }

    const WALK_ROOT: u64 = 0x9000;
    const ROOT_VA: u64 = 0xFFFF_9000_0000_0000;
    const BOARD_VA: u64 = 0xFFFF_9000_0200_0000;

    fn live() -> BoardLiveValues {
        BoardLiveValues {
            current_mw: 90_000,
            default_mw: 100_000,
            max_mw: 140_000,
            min_mw: Some(50_000),
        }
    }

    fn put_dwords(frame: &mut [u8; 4096], at: usize, values: &[u32]) {
        for (i, v) in values.iter().enumerate() {
            frame[at + i * 4..at + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
    }

    #[test]
    fn scan_page_matches_adjacent_row() {
        let mut page = [0u8; 4096];
        put_dwords(&mut page, 0xFC, &[50_000, 100_000, 90_000, 140_000]);
        let hits = scan_page_for_window(&page, &live());
        assert_eq!(hits.len(), 1);
        let hit = &hits[0];
        assert_eq!(hit.max_off, 0x108);
        assert_eq!(hit.cur_off, Some(0x104));
        assert_eq!(hit.def_off, 0x100);
        assert_eq!(hit.min_off, Some(0xFC));
        assert_eq!(hit.span, 12);
    }

    #[test]
    fn scan_page_current_equals_default_needs_two_dwords() {
        // 出厂未扰动:current == default,两处不同 dword + max 聚拢
        let mut page = [0u8; 4096];
        put_dwords(&mut page, 0x2C, &[50_000, 100_000, 100_000, 140_000]);
        let vals = BoardLiveValues {
            current_mw: 100_000,
            ..live()
        };
        let hits = scan_page_for_window(&page, &vals);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].max_off, 0x38);
        assert_ne!(hits[0].cur_off, Some(hits[0].def_off));
        assert_eq!(hits[0].min_off, Some(0x2C));
    }

    #[test]
    fn scan_page_all_equal_triple_requires_three_dwords() {
        // 三元全等(2060 类卡 current=default=max):三处 dword 才算命中
        let mut page = [0u8; 4096];
        put_dwords(&mut page, 0x40, &[100_000, 100_000, 100_000]);
        let same = BoardLiveValues {
            current_mw: 100_000,
            default_mw: 100_000,
            max_mw: 100_000,
            min_mw: None,
        };
        assert_eq!(scan_page_for_window(&page, &same).len(), 1);
        // 只有两处 → 噪声门拒
        let mut page2 = [0u8; 4096];
        put_dwords(&mut page2, 0x40, &[100_000, 100_000]);
        assert!(scan_page_for_window(&page2, &same).is_empty());
    }

    #[test]
    fn scan_page_rejects_stretched_fields() {
        // default 远离 max(> MATCH_SPAN):不聚拢,拒绝
        let mut page = [0u8; 4096];
        put_dwords(&mut page, 0x400, &[100_000]);
        put_dwords(&mut page, 0x104, &[90_000]);
        put_dwords(&mut page, 0x108, &[140_000]);
        assert!(scan_page_for_window(&page, &live()).is_empty());
    }

    #[test]
    fn scan_page_matches_info_row_without_control_slot() {
        // info 行(静态策略行):只有 {min, default, max},页上没有 current 值
        // —— 降级匹配(cur None),这是除控制行之外的第二类候选
        let mut page = [0u8; 4096];
        put_dwords(&mut page, 0xFC, &[50_000, 100_000, 140_000]);
        let hits = scan_page_for_window(&page, &live());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].cur_off, None);
        assert_eq!(hits[0].max_off, 0x104);
        assert_eq!(hits[0].def_off, 0x100);
        assert_eq!(hits[0].min_off, Some(0xFC));
    }

    #[test]
    fn locate_finds_via_pointer_hop() {
        let mut pages: HashMap<u64, [u8; 4096]> = HashMap::new();
        let mut next_table = 0x10_000u64;
        let root_frames = 0x20_000u64;
        for i in 0..ROOT_OBJECT_PAGES {
            map_page(
                &mut pages,
                WALK_ROOT,
                ROOT_VA + i * 0x1000,
                root_frames + i * 0x1000,
                &mut next_table,
            );
        }
        map_page(&mut pages, WALK_ROOT, BOARD_VA, 0x40_000, &mut next_table);
        // root 对象偏移 0x1CA0 在对象第 2 页(页内偏移 0xCA0):一根指针指向 board 页
        let ptr_page = pages.get_mut(&(root_frames + 0x1000)).unwrap();
        ptr_page[0xCA0..0xCA8].copy_from_slice(&BOARD_VA.to_le_bytes());
        // board 页值行
        put_dwords(
            pages.get_mut(&0x40_000).unwrap(),
            0xFC,
            &[50_000, 100_000, 90_000, 140_000],
        );

        let phys = MockPhysical { pages };
        let scan = locate_board_window_candidates(&phys, WALK_ROOT, ROOT_VA, &live());
        assert_eq!(scan.candidates.len(), 1, "scan = {scan:?}");
        let cands = probeable_candidates(&scan, PROBE_CAP).expect("恰一候选应放行");
        assert_eq!(cands[0].page_va, BOARD_VA);
        assert_eq!(cands[0].frame, 0x40_000);
        assert_eq!(cands[0].hit.max_off, 0x108);
    }

    #[test]
    fn locate_sweeps_pool_neighbors() {
        // echo 行(指针可达)+ 活体行(同池邻页,无指针指向):phase2 邻域
        // 扫查必须把活体行也捞出来 —— 两副本静态不可分,交给写臂探测消歧
        let mut pages: HashMap<u64, [u8; 4096]> = HashMap::new();
        let mut next_table = 0x10_000u64;
        let root_frames = 0x20_000u64;
        for i in 0..ROOT_OBJECT_PAGES {
            map_page(
                &mut pages,
                WALK_ROOT,
                ROOT_VA + i * 0x1000,
                root_frames + i * 0x1000,
                &mut next_table,
            );
        }
        let echo_va = 0xFFFF_9000_0200_0000;
        let live_va = echo_va + 0x1000; // 邻页,无指针指向
        map_page(&mut pages, WALK_ROOT, echo_va, 0x40_000, &mut next_table);
        map_page(&mut pages, WALK_ROOT, live_va, 0x41_000, &mut next_table);
        let ptr_page = pages.get_mut(&(root_frames + 0x1000)).unwrap();
        ptr_page[0xCA0..0xCA8].copy_from_slice(&echo_va.to_le_bytes());
        for f in [0x40_000, 0x41_000] {
            put_dwords(
                pages.get_mut(&f).unwrap(),
                0xFC,
                &[50_000, 100_000, 90_000, 140_000],
            );
        }
        let phys = MockPhysical { pages };
        let scan = locate_board_window_candidates(&phys, WALK_ROOT, ROOT_VA, &live());
        assert!(
            scan.candidates.iter().any(|c| c.page_va == live_va),
            "邻页活体行必须被 sweep 捞出: {scan:?}"
        );
        assert_eq!(scan.candidates.len(), 2);
        assert!(probeable_candidates(&scan, PROBE_CAP).is_ok());
    }

    #[test]
    fn locate_refuses_unprobeable_ambiguity() {
        let mut pages: HashMap<u64, [u8; 4096]> = HashMap::new();
        let mut next_table = 0x10_000u64;
        for i in 0..ROOT_OBJECT_PAGES {
            map_page(
                &mut pages,
                WALK_ROOT,
                ROOT_VA + i * 0x1000,
                0x20_000 + i * 0x1000,
                &mut next_table,
            );
        }
        let board_a = 0xFFFF_9000_0200_0000;
        let board_b = 0xFFFF_9000_0200_1000;
        map_page(&mut pages, WALK_ROOT, board_a, 0x40_000, &mut next_table);
        map_page(&mut pages, WALK_ROOT, board_b, 0x41_000, &mut next_table);
        let rf = pages.get_mut(&0x21_000).unwrap(); // root 对象页 1
        rf[0xCA0..0xCA8].copy_from_slice(&board_a.to_le_bytes());
        rf[0xCA8..0xCB0].copy_from_slice(&board_b.to_le_bytes());
        for f in [0x40_000, 0x41_000] {
            put_dwords(
                pages.get_mut(&f).unwrap(),
                0xFC,
                &[50_000, 100_000, 90_000, 140_000],
            );
        }
        let phys = MockPhysical { pages };
        let scan = locate_board_window_candidates(&phys, WALK_ROOT, ROOT_VA, &live());
        assert_eq!(scan.candidates.len(), 2);
        assert!(probeable_candidates(&scan, 1).is_err(), "超探测上限必须拒");
    }

    #[test]
    fn cooccurrence_sweeps_hit_neighborhoods() {
        // 钳表不在基本域、在命中页邻页:二轮 sweep 必须捞出
        let mut pages: HashMap<u64, [u8; 4096]> = HashMap::new();
        let mut next_table = 0x10_000u64;
        let va = 0xFFFF_9000_0200_0000;
        let clamp_va = va + 0x1000; // 邻页,不在调用方页表里
        map_page(&mut pages, WALK_ROOT, va, 0x40_000, &mut next_table);
        map_page(&mut pages, WALK_ROOT, clamp_va, 0x41_000, &mut next_table);
        {
            let f = pages.get_mut(&0x40_000).unwrap();
            f[0x40..0x44].copy_from_slice(&90_000u32.to_le_bytes());
            f[0x400..0x404].copy_from_slice(&140_000u32.to_le_bytes());
            let g = pages.get_mut(&0x41_000).unwrap();
            g[0x80..0x84].copy_from_slice(&90_000u32.to_le_bytes());
            g[0x300..0x304].copy_from_slice(&140_000u32.to_le_bytes());
        }
        let phys = MockPhysical { pages };
        // 只喂 va(基本域);clamp_va 应由 sweep 捞出
        let hits = value_cooccurrence_scan(&phys, WALK_ROOT, &[va], &live(), 16);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert!(hits.iter().any(|h| h.page_va == clamp_va));
    }

    #[test]
    fn cooccurrence_reports_wide_record_rows() {
        // 宽记录行:current 与 max 跨度 0x3C0(> MATCH_SPAN),三元组抓不到,
        // 共现报告按"≥2 种活体值同页"给出全部偏移
        let mut pages: HashMap<u64, [u8; 4096]> = HashMap::new();
        let mut next_table = 0x10_000u64;
        let va = 0xFFFF_9000_0200_0000;
        map_page(&mut pages, WALK_ROOT, va, 0x40_000, &mut next_table);
        {
            let f = pages.get_mut(&0x40_000).unwrap();
            f[0x40..0x44].copy_from_slice(&90_000u32.to_le_bytes());
            f[0x400..0x404].copy_from_slice(&140_000u32.to_le_bytes());
        }
        let phys = MockPhysical { pages };
        let hits = value_cooccurrence_scan(&phys, WALK_ROOT, &[va], &live(), 8);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page_va, va);
        assert_eq!(
            hits[0].values,
            vec![(90_000, vec![0x40]), (140_000, vec![0x400])]
        );
    }

    #[test]
    fn scan_pages_respects_budget_and_counts_unreadable() {
        let mut pages: HashMap<u64, [u8; 4096]> = HashMap::new();
        let mut next_table = 0x10_000u64;
        map_page(&mut pages, WALK_ROOT, ROOT_VA, 0x20_000, &mut next_table);
        let unmapped = 0xFFFF_9000_0300_0000; // 无 PTE → 不可读
        let phys = MockPhysical { pages };
        let mut seen: HashSet<u64> = HashSet::new();
        let scan = scan_pages(
            &phys,
            WALK_ROOT,
            &[ROOT_VA, unmapped],
            &live(),
            1,
            &mut seen,
        );
        assert_eq!(scan.pages_scanned, 1, "预算 1 只扫一页");
        assert_eq!(scan.pages_unreadable, 0, "预算耗尽在不可读页之前");
        let mut seen2: HashSet<u64> = HashSet::new();
        let scan2 = scan_pages(
            &phys,
            WALK_ROOT,
            &[ROOT_VA, unmapped],
            &live(),
            2,
            &mut seen2,
        );
        assert_eq!(scan2.pages_scanned, 1);
        assert_eq!(scan2.pages_unreadable, 1);
    }
}
