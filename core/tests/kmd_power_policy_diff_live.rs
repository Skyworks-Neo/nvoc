//! L2.2 活体差分 harness(任务书 §3.2;kmd 通道,本 harness 全程只读)。
//!
//! GPU 写扰动由 nvoc-cli 在外部完成:`set-pwr-cur-limit tgp 90` → snapshot a →
//! `tgp 95` → diff → `tgp 100` 恢复。相位经环境变量选择(elevated PowerShell):
//!
//! ```text
//! $env:NVOC_POWER_DIFF_PHASE='snapshot'; $env:NVOC_POWER_DIFF_TAG='a'
//! <编译产物>\kmd_power_policy_diff_live-*.exe --ignored --nocapture
//! ```
//!
//! snapshot:读 nvlddmkm 镜像数据节收集内核指针 → 逐目标页翻译+读取 →
//!   值指纹(mW 值集 + F7 记录 0x3F7 + PowerRoot 同页指纹)→ 存 JSON。
//! diff:重扫快照 A 的候选页,dword 级 diff,定位 90000→95000 变化页。
//! read:按 NVOC_POWER_READ_VA 直读 root 对象并解析八字段(L3 身份基线)。
#![cfg(windows)]

use nvoc_core::kmd::pagewalk::{
    discover_root, find_loaded_module, read_virtual, translate, PeFingerprint, PhysicalMemory,
};
use nvoc_core::kmd::pmxdrv::{PmxDrv, PmxDrvPhysMem};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::env;
use std::path::PathBuf;

/// 差分观测值(mW):90/95/100/140 W 与窗下限 5 W;150000=策略 max 候选
/// (锚页值表实测),135000=NVVDD OCP max,50000=半功率槽。
const VALUES: [u32; 8] = [90000, 95000, 100000, 140000, 5000, 150000, 135000, 50000];
/// Board selector 记录里的 F7 生成器输出标记(616.92: source=0xF7,mode=3)。
const F7_RECORD: u16 = 0x3F7;
/// 610.74 PowerRoot 字段偏移(capstone 静态轨产出)。
const OFF_INIT: u64 = 0x3CE0;
const OFF_BASE: u64 = 0x3CE4;
const OFF_AMOUNT: u64 = 0x3CE8;
const OFF_KEY: u64 = 0x3CEC;
const OFF_LOWER: u64 = 0x3CF0;
const OFF_UPPER: u64 = 0x3CF4;

/// 参与走查的镜像节(RVA,字节数;capstone 阶段据节表选定的数据/表节)。
const SECTIONS: [(&str, u32, u32); 10] = [
    (".data", 0x10ED000, 0x2F2A28),
    ("_DDTEXT", 0xD30000, 0x5915C),
    ("_DDRDATA", 0x14CA000, 0x4358),
    ("_DDDATA", 0x14CF000, 0x41B8),
    (".rdata", 0xE07000, 0x2E5814),
    ("PAGEDATA", 0x19D0000, 0xB9E04),
    ("PAGE_DDC", 0x1A8A000, 0x2C590),
    ("PAGE_DDD", 0x1AB7000, 0x96B0),
    ("PAGEcRM", 0x1D60000, 0x61871),
    ("PAGEcRMA", 0x731C000, 0xFBDC),
];

/// PTE 页缓存:translate 的 8 字节读高度重复合访同一页表页,缓存把
/// 每次 miss 的 map/unmap 往返压掉;4096 字节载荷读直通不缓存。
/// RAM 白名单:BSOD 教训(2026-10-06,BFS 批量映射后 KMODE_EXCEPTION_NOT_HANDLED,
/// 头号嫌疑 = 映射到 MMIO/保留帧)——任何帧映射前先校验落在注册表
/// Physical Memory 资源表报告的 RAM 范围内,否则拒绝读。
struct CachedPhys<'a> {
    inner: &'a PmxDrvPhysMem<'a>,
    cache: RefCell<HashMap<u64, [u8; 4096]>>,
    failed: RefCell<HashSet<u64>>,
    ram_ranges: Vec<(u64, u64)>,
    rejected: RefCell<usize>,
}

/// 读 HKLM\HARDWARE\RESOURCEMAP\System Resources\Physical Memory(REG_RESOURCE_LIST)
/// 提取 RAM 范围。解析失败返回空表(调用方按"不过滤"降级并打印告警)。
fn ram_ranges() -> Vec<(u64, u64)> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_LOCAL_MACHINE, KEY_READ,
    };
    const PATH: &str = r"HARDWARE\RESOURCEMAP\System Resources\Physical Memory";
    const VALUE: &str = ".Translated";
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let mut buf = vec![0u8; 65536];
    let mut size = buf.len() as u32;
    let mut value_type: u32 = 0;
    let mut hkey: *mut std::ffi::c_void = std::ptr::null_mut();
    let path_w = wide(PATH);
    let value_w = wide(VALUE);
    // RegGetValueW 对 REG_RESOURCE_LIST 实测返回 ERROR_FILE_NOT_FOUND,
    // 用经典 Open/Query 对。
    let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, path_w.as_ptr(), 0, KEY_READ, &mut hkey) };
    if status != 0 {
        println!("  警告: 打开 Physical Memory 键失败 status=0x{status:X},RAM 白名单降级为不过滤");
        return Vec::new();
    }
    let status = unsafe {
        RegQueryValueExW(hkey, value_w.as_ptr(), std::ptr::null_mut(), &mut value_type, buf.as_mut_ptr(), &mut size)
    };
    unsafe { RegCloseKey(hkey) };
    if status != 0 {
        println!("  警告: 读 .Translated 失败 status=0x{status:X},RAM 白名单降级为不过滤");
        return Vec::new();
    }
    buf.truncate(size as usize);
    // CM_RESOURCE_LIST: FullDescriptors[] { InterfaceType, BusNumber, Version, Revision,
    //   Count, PartialResourceDescriptors[] };PARTIAL_DESCRIPTOR { Type u8, ShareDisposition u8,
    //   Flags u16, u64 union }。type=3(CmResourceTypeMemory) 且 Flags bit12(CM_RESOURCE_MEMORY_READABLE)? 
    //   取 union: 内存描述符 union = {u64 Start}。
    let le = |b: &[u8]| -> u64 {
        b.iter().take(8).enumerate().fold(0u64, |a, (i, &x)| a | ((x as u64) << (8 * i)))
    };
    let mut ranges = Vec::new();
    // CM_PARTIAL_RESOURCE_DESCRIPTOR 的 stride/union 偏移存在两种候选布局
    // (stride20/union4 vs stride24/union8),用自检自动裁决:正确布局必须
    // 覆盖低内存页(0x20000 必为 RAM)且总量 > 1 GiB。
    if buf.len() < 4 {
        return ranges;
    }
    let count = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    println!(
        "  RAM 表原始头 64B: {}",
        buf[..buf.len().min(64)].iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let mut best: Option<(Vec<(u64, u64)>, &str)> = None;
    for (stride, union_off, label) in [(20usize, 4usize, "stride20/union4"), (24, 8, "stride24/union8")] {
        let mut rs = Vec::new();
        let mut off = 4usize;
        let mut bad = false;
        for _ in 0..count {
            if off + 16 > buf.len() {
                bad = true;
                break;
            }
            let partial_count = u32::from_le_bytes(buf[off + 12..off + 16].try_into().unwrap()) as usize;
            off += 16;
            for _ in 0..partial_count {
                if off + stride > buf.len() {
                    bad = true;
                    break;
                }
                if buf[off] == 3 {
                    let start = le(&buf[off + union_off..off + union_off + 8]);
                    let len = u32::from_le_bytes(
                        buf[off + union_off + 8..off + union_off + 12].try_into().unwrap(),
                    ) as u64;
                    if len > 0 {
                        rs.push((start, start + len));
                    }
                }
                off += stride;
            }
        }
        let total: u64 = rs.iter().map(|(s, e)| e - s).sum();
        let covers_low = rs.iter().any(|(s, e)| 0x20000 >= *s && 0x20000 < *e);
        println!(
            "  RAM 表布局 {label}: {} 段, 总 {:.2} GiB, 覆盖低内存={covers_low}, 截断={bad}",
            rs.len(),
            total as f64 / (1 << 30) as f64
        );
        if covers_low && total > (1 << 30) {
            let better = match &best {
                None => true,
                Some((old, _)) => total > old.iter().map(|(s, e)| e - s).sum::<u64>(),
            };
            if better {
                best = Some((rs, label));
            }
        }
    }
    match best {
        Some((rs, label)) => {
            println!("  RAM 表采用布局 {label}");
            ranges = rs;
        }
        None => println!("  警告: 两种布局都不能覆盖低内存,RAM 白名单降级为不过滤"),
    }
    ranges
}

impl<'a> CachedPhys<'a> {
    fn new(inner: &'a PmxDrvPhysMem<'a>) -> Self {
        let ram = ram_ranges();
        let total: u64 = ram.iter().map(|(s, e)| e - s).sum();
        println!(
            "RAM 白名单: {} 段, 共 {:.1} GiB{}",
            ram.len(),
            total as f64 / (1 << 30) as f64,
            if ram.is_empty() { "(降级:不过滤)" } else { "" }
        );
        Self {
            inner,
            cache: RefCell::new(HashMap::new()),
            failed: RefCell::new(HashSet::new()),
            ram_ranges: ram,
            rejected: RefCell::new(0),
        }
    }

    fn in_ram(&self, frame: u64) -> bool {
        if self.ram_ranges.is_empty() {
            return true;
        }
        let ok = self.ram_ranges.iter().any(|(s, e)| frame >= *s && frame < *e);
        if !ok {
            *self.rejected.borrow_mut() += 1;
        }
        ok
    }
}

impl PhysicalMemory for CachedPhys<'_> {
    fn read(&self, pa: u64, out: &mut [u8]) -> Result<(), nvoc_core::kmd::pagewalk::PhysError> {
        use nvoc_core::kmd::pagewalk::PhysError;
        let frame = pa & !0xFFF;
        if !self.in_ram(frame) {
            return Err(PhysError::Unreadable(pa));
        }
        if out.len() != 8 {
            return self.inner.read(pa, out);
        }
        let off = (pa - frame) as usize;
        if let Some(page) = self.cache.borrow().get(&frame) {
            out.copy_from_slice(&page[off..off + 8]);
            return Ok(());
        }
        if self.failed.borrow().contains(&frame) {
            return Err(PhysError::Unreadable(pa));
        }
        let mut page = [0u8; 4096];
        match self.inner.read(frame, &mut page) {
            Ok(()) => {
                let mut map = self.cache.borrow_mut();
                if map.len() > 24_576 {
                    map.clear();
                }
                map.insert(frame, page);
                drop(map);
                out.copy_from_slice(&page[off..off + 8]);
                Ok(())
            }
            Err(err) => {
                self.failed.borrow_mut().insert(frame);
                Err(err)
            }
        }
    }
}

fn out_dir() -> PathBuf {
    env::var_os("NVOC_POWER_OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../reverse/kmd-power-4060l"))
}

fn connect_walk() -> (
    nvoc_core::kmd::pagewalk::LoadedModule,
    CachedPhys<'static>,
    u64,
) {
    // 返回值生命周期走借用会牵扯 PmxDrv 存活期;这里泄漏连接对象,
    // 测试进程退出即回收(单次批处理工具,不做运行时清理)。
    let module = find_loaded_module("nvlddmkm.sys").expect("nvlddmkm.sys 不在系统模块列表");
    println!(
        "nvlddmkm: 基址 0x{:016X}, 磁盘 {}",
        module.base,
        module.path.display()
    );
    let disk_head =
        std::fs::read(&module.path).unwrap_or_else(|e| panic!("读磁盘镜像失败: {e}"));
    let fingerprint = PeFingerprint::from_image(&disk_head).expect("磁盘镜像不是有效 PE32+");
    let drv: &'static PmxDrv =
        Box::leak(Box::new(PmxDrv::connect().expect("PMXDRV 连接失败(服务须运行)")));
    let pm: &'static PmxDrvPhysMem<'static> = Box::leak(Box::new(PmxDrvPhysMem::new(drv)));
    let phys = CachedPhys::new(pm);
    let discovery = discover_root(&phys, module.base, &fingerprint);
    for event in &discovery.events {
        println!("  {event}");
    }
    let root = discovery
        .unique_root()
        .expect("页表根不唯一,中止(见事件)");
    println!("页表根: 0x{root:016X}");
    (module, phys, root)
}

/// 逐页读一段内核 VA(缺页置 0 并记录洞)。
fn read_range(
    phys: &dyn PhysicalMemory,
    root: u64,
    va: u64,
    len: usize,
    holes: &mut Vec<u64>,
) -> Vec<u8> {
    let mut out = vec![0u8; len];
    let mut done = 0usize;
    while done < len {
        let page_va = va + done as u64;
        match read_virtual(phys, root, page_va & !0xFFF, 4096) {
            Ok(page) => {
                let off = (page_va & 0xFFF) as usize;
                let n = (len - done).min(4096 - off);
                out[done..done + n].copy_from_slice(&page[off..off + n]);
            }
            Err(_) => holes.push(page_va & !0xFFF),
        }
        done += 4096 - ((page_va & 0xFFF) as usize);
    }
    out
}

/// PowerRoot 同页指纹(放宽版,适配 610 base 可能为 -1 哨兵):
/// UPPER 槽 dword[o]==100000 && init 字节 [o-0x14]==1 && key 字节 [o-8]<0x40。
/// base(0x3CE4-eq)不作硬条件,另行报告。
fn root_fingerprint(buf: &[u8]) -> Vec<usize> {
    let mut hits = Vec::new();
    if buf.len() < 0x20 {
        return hits;
    }
    for o in (0x14..buf.len() - 3).step_by(4) {
        let upper = u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
        if upper != 100000 {
            continue;
        }
        let init = buf[o - 0x14];
        let key = buf[o - 8];
        if init == 1 && key < 0x40 {
            hits.push(o);
        }
    }
    hits
}

/// 一页的观测:命中的值偏移、F7 记录偏移、root 指纹偏移。
struct PageScan {
    value_hits: HashMap<u32, Vec<usize>>,
    f7_hits: Vec<usize>,
    fingerprint: Vec<usize>,
}

fn scan_page(buf: &[u8]) -> PageScan {
    let mut value_hits: HashMap<u32, Vec<usize>> = HashMap::new();
    for &v in &VALUES {
        let mut offs = Vec::new();
        for o in (0..buf.len() - 3).step_by(4) {
            if u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()) == v {
                offs.push(o);
            }
        }
        if !offs.is_empty() {
            value_hits.insert(v, offs);
        }
    }
    let mut f7_hits = Vec::new();
    for o in (0..buf.len() - 1).step_by(2) {
        if u16::from_le_bytes(buf[o..o + 2].try_into().unwrap()) == F7_RECORD {
            f7_hits.push(o);
        }
    }
    let fingerprint = root_fingerprint(buf);
    PageScan {
        value_hits,
        f7_hits,
        fingerprint,
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[test]
#[ignore = "requires PMXDRV_NEW service + elevated shell; phases via NVOC_POWER_DIFF_PHASE"]
fn l2_power_policy_diff() {
    let phase = env::var("NVOC_POWER_DIFF_PHASE").unwrap_or_else(|_| "snapshot".into());
    let tag = env::var("NVOC_POWER_DIFF_TAG").unwrap_or_else(|_| "a".into());
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    println!("相位 {phase},标签 {tag},输出目录 {}", dir.display());

    match phase.as_str() {
        "snapshot" => snapshot(&tag, &dir),
        "diff" => diff(&tag, &dir),
        "read" => read_object(&dir),
        "near" => near(&dir),
        "hub" => hub(&dir),
        "region" => region(&tag, &dir),
        "trace" => trace(&dir),
        "trace" => trace(&dir),
        "graphwalk" => graphwalk(&tag, &dir),
        "graphwalk" => graphwalk(&tag, &dir),
        "marker" => marker(&dir),
        "chain" => chain(&dir),
        "statewalk" => statewalk(&dir),
        other => panic!("未知相位 {other}(snapshot|diff|read|near|hub|region|trace|graphwalk|marker|chain|statewalk)"),
    }
}

/// statewalk:全局槽 0x13AAD58 → state 页全部指针 P,对每个 P 探两种形态:
/// [P+0xEE10](per-GPU 大上下文持 root)与 [P+0x2510](Major 持 root)。
/// root 门:init==1 && key<0x40 && UPPER ∈ 候选集。
fn statewalk(dir: &PathBuf) {
    let (module, phys, root) = connect_walk();
    let slot_rva = 0x13AAD58;
    let state = {
        let mut h = Vec::new();
        let b = read_range(&phys, root, module.base + slot_rva, 8, &mut h);
        let v = u64::from_le_bytes(b[0..8].try_into().unwrap());
        assert!(h.is_empty() && v >= 0xFFFF_8000_0000_0000, "state 指针无效");
        v
    };
    println!("state = 0x{state:016X}");
    let state_span = env::var("NVOC_POWER_STATE_SPAN")
        .ok()
        .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x1000);
    let mut state_ptrs: Vec<(u64, u64)> = Vec::new(); // (state 内偏移, 指针)
    for off in (0..state_span).step_by(0x1000) {
        let mut h = Vec::new();
        let page = read_range(&phys, root, state + off as u64, 4096, &mut h);
        if !h.is_empty() {
            continue;
        }
        for o in (0..4088).step_by(8) {
            let q = u64::from_le_bytes(page[o..o + 8].try_into().unwrap());
            if (0xFFFF_8000_0000_0000..=0xFFFF_F7FF_FFFF_F000).contains(&q) {
                state_ptrs.push(((off + o) as u64, q & !0xFFF));
            }
        }
    }
    println!("state 指针 {} 个(span 0x{state_span:X})", state_ptrs.len());
    let page = [0u8; 4096]; // 兼容旧循环变量(实际循环已下沉)
    let expect_uppers: Vec<u32> = env::var("NVOC_POWER_EXPECT_UPPER")
        .unwrap_or_else(|_| "100000,140000,150000,135000".into())
        .split(',')
        .filter_map(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
        .collect();

    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    let mut spent = 0usize;
    for (o, p) in state_ptrs.clone() {
        if !seen.insert(p) {
            continue;
        }
        for (probe_off, label) in [(0xEE10u64, "ctx+EE10"), (0x2510, "Major+2510")] {
            let Some(va) = p.checked_add(probe_off) else { continue };
            let mut hh = Vec::new();
            let b = read_range(&phys, root, va, 8, &mut hh);
            spent += 1;
            if !hh.is_empty() || b.len() != 8 {
                continue;
            }
            let root_va = u64::from_le_bytes(b[0..8].try_into().unwrap());
            if !(0xFFFF_8000_0000_0000..=0xFFFF_F7FF_FFFF_F000).contains(&root_va) {
                continue;
            }
            let Some(init_va) = root_va.checked_add(OFF_INIT) else { continue };
            let mut h2 = Vec::new();
            let head = read_range(&phys, root, init_va, 16, &mut h2);
            spent += 1;
            if h2.is_empty() && head.len() == 16 {
                let init = head[0];
                let key = head[12];
                if init != 1 || key >= 0x40 {
                    continue;
                }
                let Some(uva) = root_va.checked_add(OFF_UPPER) else { continue };
                let mut h3 = Vec::new();
                let ub = read_range(&phys, root, uva, 4, &mut h3);
                spent += 1;
                if h3.is_empty() && ub.len() == 4 {
                    let upper = u32::from_le_bytes(ub[0..4].try_into().unwrap());
                    if !expect_uppers.contains(&upper) {
                        continue;
                    }
                    let f = |roff: u64| -> Option<u32> {
                        let Some(v2) = root_va.checked_add(roff) else { return None };
                        let mut h4 = Vec::new();
                        let b4 = read_range(&phys, root, v2, 4, &mut h4);
                        if h4.is_empty() && b4.len() == 4 {
                            Some(u32::from_le_bytes(b4[0..4].try_into().unwrap()))
                        } else {
                            None
                        }
                    };
                    let fr = translate(&phys, root, root_va).map(|pa| pa & !0xFFF).ok();
                    let fru = translate(&phys, root, root_va + OFF_UPPER).map(|pa| pa & !0xFFF).ok();
                    let rec = json!({
                        "via": label, "state_ptr_off": o, "ptr": p, "root_va": root_va,
                        "frame@root": fr, "frame@upper": fru,
                        "init@3CE0": init, "elig@3CE1": head[1], "amountActive@3CE2": head[2],
                        "base@3CE4": u32::from_le_bytes(head[4..8].try_into().unwrap()),
                        "amount@3CE8": u32::from_le_bytes(head[8..12].try_into().unwrap()),
                        "key@3CEC": key, "lower@3CF0": f(OFF_LOWER), "upper@3CF4": upper,
                        "aux1@3CF8": f(OFF_UPPER + 4), "aux2@3CFC": f(OFF_UPPER + 8),
                    });
                    println!(
                        "★ root@0x{root_va:016X} via {label}(state+0x{o:X}) init={init} elig={} aa={} base={} amount={} key={} lower={} UPPER={upper} aux1={} aux2={}",
                        rec["elig@3CE1"], rec["amountActive@3CE2"], rec["base@3CE4"],
                        rec["amount@3CE8"], rec["key@3CEC"], rec["lower@3CF0"],
                        rec["aux1@3CF8"], rec["aux2@3CFC"]
                    );
                    println!("   帧: root=0x{:016X} upper=0x{:016X}", fr.unwrap_or(0), fru.unwrap_or(0));
                    roots.push(rec);
                }
            }
        }
    }
    // 大分配探测:GPU 表(616.92 式 ~300KB)应使 P+0x48000 可读;
    // 对通过者扫 0x40000-0x50000 的指针域做 root 门。
    for (o, p) in state_ptrs {
        if !seen.insert(p) {
            continue;
        }
        let Some(probe_va) = p.checked_add(0x48000) else { continue };
        let mut hp = Vec::new();
        let _probe = read_range(&phys, root, probe_va, 8, &mut hp);
        if !hp.is_empty() {
            continue;
        }
        println!("  大分配候选 state+0x{o:X} = 0x{p:016X}(+0x48000 可读)");
        for off in (0x40000u64..0x50000).step_by(0x1000) {
            let Some(va) = p.checked_add(off) else { continue };
            let mut h5 = Vec::new();
            let buf = read_range(&phys, root, va, 4096, &mut h5);
            if !h5.is_empty() {
                continue;
            }
            spent += 1;
            for po in (0..4088).step_by(8) {
                let q2 = u64::from_le_bytes(buf[po..po + 8].try_into().unwrap());
                if !(0xFFFF_8000_0000_0000..=0xFFFF_F7FF_FFFF_F000).contains(&q2) {
                    continue;
                }
                let Some(mva) = q2.checked_add(0x2510) else { continue };
                let mut h6 = Vec::new();
                let b = read_range(&phys, root, mva, 8, &mut h6);
                spent += 1;
                if h6.is_empty() && b.len() == 8 {
                    let root_va = u64::from_le_bytes(b[0..8].try_into().unwrap());
                    if !(0xFFFF_8000_0000_0000..=0xFFFF_F7FF_FFFF_F000).contains(&root_va) {
                        continue;
                    }
                    let Some(init_va) = root_va.checked_add(OFF_INIT) else { continue };
                    let mut h7 = Vec::new();
                    let head = read_range(&phys, root, init_va, 16, &mut h7);
                    spent += 1;
                    if h7.is_empty() && head.len() == 16 {
                        let init = head[0];
                        let key = head[12];
                        if init != 1 || key >= 0x40 {
                            continue;
                        }
                        let Some(uva) = root_va.checked_add(OFF_UPPER) else { continue };
                        let mut h8 = Vec::new();
                        let ub = read_range(&phys, root, uva, 4, &mut h8);
                        spent += 1;
                        if h8.is_empty() && ub.len() == 4 {
                            let upper = u32::from_le_bytes(ub[0..4].try_into().unwrap());
                            if !expect_uppers.contains(&upper) {
                                continue;
                            }
                            let fr = translate(&phys, root, root_va).map(|pa| pa & !0xFFF).ok();
                            let fru = translate(&phys, root, root_va + OFF_UPPER).map(|pa| pa & !0xFFF).ok();
                            println!(
                                "★★ root@0x{root_va:016X} via GPU 表(state+0x{o:X},entry@+0x{off:X}+0x{po:X}) init={init} key={key} UPPER={upper}"
                            );
                            println!("   帧: root=0x{:016X} upper=0x{:016X}", fr.unwrap_or(0), fru.unwrap_or(0));
                            roots.push(json!({"via": "gpu_table", "state_ptr_off": o,
                                "table": p, "entry_va": q2, "root_va": root_va,
                                "frame@root": fr, "frame@upper": fru,
                                "init@3CE0": init, "key@3CEC": key, "upper@3CF4": upper}));
                        }
                    }
                }
            }
        }
    }
    let report = json!({"state": state, "spent": spent, "roots": roots});
    let path = dir.join("statewalk_report.json");
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("statewalk 完成: 读 {spent} 页, root 命中 {} → {}", roots.len(), path.display());
}

/// chain:静态链(全局槽 → 状态 → GPU 表 → Major → root)的有界活体走查。
/// env: NVOC_POWER_CHAIN_SLOT(全局槽 RVA)、NVOC_POWER_CHAIN_TABLE_OFF
/// (state→table 偏移)、NVOC_POWER_CHAIN_TABLE_SPAN(默认 0x60000)。
/// Major 候选 = 表内的内核指针;root 判据 = [Major+0x2510] → 页含
/// init==1 && key<0x40 && UPPER∈{100000,其它} 五元组。
fn chain(dir: &PathBuf) {
    let slot_rva = u64::from_str_radix(
        env::var("NVOC_POWER_CHAIN_SLOT").expect("chain 需 NVOC_POWER_CHAIN_SLOT").trim_start_matches("0x"),
        16,
    ).unwrap();
    let table_off = u64::from_str_radix(
        env::var("NVOC_POWER_CHAIN_TABLE_OFF").expect("chain 需 NVOC_POWER_CHAIN_TABLE_OFF").trim_start_matches("0x"),
        16,
    ).unwrap();
    let span = env::var("NVOC_POWER_CHAIN_TABLE_SPAN")
        .ok()
        .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x6_0000);
    let budget = env::var("NVOC_POWER_WALK_BUDGET")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1500);
    let (module, phys, root) = connect_walk();

    let rd_q = |va: u64| -> Option<u64> {
        let mut h = Vec::new();
        let b = read_range(&phys, root, va, 8, &mut h);
        if h.is_empty() && b.len() == 8 {
            Some(u64::from_le_bytes(b[0..8].try_into().unwrap()))
        } else {
            None
        }
    };
    let global_state = rd_q(module.base + slot_rva).filter(|v| *v >= 0xFFFF_8000_0000_0000);
    println!("全局槽 @镜像+0x{slot_rva:X} → state = 0x{:016X?}", global_state);
    let state = global_state.expect("全局槽不是内核指针");
    let table = rd_q(state + table_off).filter(|v| *v >= 0xFFFF_8000_0000_0000);
    println!("state+0x{table_off:X} → GPU 表 = 0x{:016X?}", table);
    // 侦察:转储 state 页 0x0-0x400 的全部内核指针(表指针若不在 0x208,离线挑)
    let mut h = Vec::new();
    let state_page = read_range(&phys, root, state, 4096, &mut h);
    let mut state_ptrs = Vec::new();
    if h.is_empty() {
        for o in (0..4088).step_by(8) {
            let q = u64::from_le_bytes(state_page[o..o + 8].try_into().unwrap());
            if q >= 0xFFFF_8000_0000_0000 {
                state_ptrs.push(json!({"off": o, "ptr": q}));
            }
        }
    }
    let table = match table {
        Some(t) => t,
        None => {
            let path = dir.join("chain_state_page.json");
            std::fs::write(&path, serde_json::to_string_pretty(&json!({
                "state": state, "table_off_tried": table_off, "ptrs": state_ptrs})).unwrap()).unwrap();
            panic!("GPU 表指针无效;state 页指针已转储 {}", path.display());
        }
    };

    // 表域扫描:收集 Major 候选(表内内核指针,去重目标页)
    let mut spent = 0usize;
    let mut targets: Vec<u64> = Vec::new();
    let mut seen_t = HashSet::new();
    for off in (0..span).step_by(0x1000) {
        if spent >= budget {
            break;
        }
        let mut h = Vec::new();
        let buf = read_range(&phys, root, table + off as u64, 4096, &mut h);
        if !h.is_empty() {
            continue;
        }
        spent += 1;
        for o in (0..4088).step_by(8) {
            let q = u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
            // canonical 内核地址窗(排除 0xFFFF…FF 类野指针,防加法溢出)
            if (0xFFFF_8000_0000_0000..=0xFFFF_F7FF_FFFF_F000).contains(&q)
                && seen_t.insert(q & !0xFFF)
            {
                targets.push(q & !0xFFF);
            }
        }
    }
    println!("表域 {} 页,Major 候选目标 {} 个", spent, targets.len());
    // 表域 hex 留档(离线定表项布局)
    let mut table_hex = Vec::new();
    for off in (0..span).step_by(0x1000) {
        let mut h = Vec::new();
        let buf = read_range(&phys, root, table + off as u64, 4096, &mut h);
        if h.is_empty() {
            table_hex.push(json!({"va": table + off as u64, "hex": hex(&buf)}));
        }
    }

    let mut roots = Vec::new();
    let mut checked = 0usize;
    for &major in &targets {
        if spent >= budget || checked >= 1200 {
            println!("  达到预算,截断(已检 {checked} 候选)");
            break;
        }
        // [Major+0x2510] → root 指针
        let Some(major_off) = major.checked_add(0x2510) else { continue };
        let mut h = Vec::new();
        let b = read_range(&phys, root, major_off, 8, &mut h);
        spent += 1;
        checked += 1;
        if !h.is_empty() || b.len() != 8 {
            continue;
        }
        let root_va = u64::from_le_bytes(b[0..8].try_into().unwrap());
        if root_va < 0xFFFF_8000_0000_0000 {
            continue;
        }
        // root 头 16 字节(init/elig/amountActive/pad + base/amount)快速门
        let mut h2 = Vec::new();
        let Some(init_va) = root_va.checked_add(OFF_INIT) else { continue };
        let head = read_range(&phys, root, init_va, 16, &mut h2);
        spent += 1;
        if h2.is_empty() && head.len() == 16 {
            let init = head[0];
            let base = u32::from_le_bytes(head[4..8].try_into().unwrap());
            let amount = u32::from_le_bytes(head[8..12].try_into().unwrap());
            let key = head[12];
            if init != 1 || key >= 0x40 {
                continue;
            }
            // UPPER 硬门:接受出厂/滑条顶候选集(差分时经 NVOC_POWER_EXPECT_UPPER 覆盖)
            let expect_uppers: Vec<u32> = env::var("NVOC_POWER_EXPECT_UPPER")
                .unwrap_or_else(|_| "100000,140000,150000,135000".into())
                .split(',')
                .filter_map(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
                .collect();
            let upper_now = {
                let mut h3 = Vec::new();
                let Some(uva) = root_va.checked_add(OFF_UPPER) else { continue };
                let b = read_range(&phys, root, uva, 4, &mut h3);
                if h3.is_empty() && b.len() == 4 {
                    Some(u32::from_le_bytes(b[0..4].try_into().unwrap()))
                } else {
                    None
                }
            };
            if !upper_now.map(|u| expect_uppers.contains(&u)).unwrap_or(false) {
                continue;
            }
            // 命中:读全字段 + 翻译帧
            let f = |roff: u64| -> Option<u32> {
                let mut h3 = Vec::new();
                let b = read_range(&phys, root, root_va + roff, 4, &mut h3);
                if h3.is_empty() && b.len() == 4 {
                    Some(u32::from_le_bytes(b[0..4].try_into().unwrap()))
                } else {
                    None
                }
            };
            let fr = translate(&phys, root, root_va).map(|pa| pa & !0xFFF).ok();
            let fru = translate(&phys, root, root_va + OFF_UPPER).map(|pa| pa & !0xFFF).ok();
            let frb = translate(&phys, root, root_va + OFF_BASE).map(|pa| pa & !0xFFF).ok();
            let rec = json!({
                "major_va": major, "root_va": root_va,
                "frame@root": fr, "frame@base": frb, "frame@upper": fru,
                "init@3CE0": init, "elig@3CE1": head[1], "amountActive@3CE2": head[2],
                "base@3CE4": base, "amount@3CE8": amount, "key@3CEC": key,
                "lower@3CF0": f(OFF_LOWER), "upper@3CF4": f(OFF_UPPER),
                "aux1@3CF8": f(OFF_UPPER + 4), "aux2@3CFC": f(OFF_UPPER + 8),
            });
            println!(
                "  ★ root@0x{root_va:016X}(Major 0x{major:016X}): init={init} elig={} amountActive={} base={base} amount={amount} key={key} lower={} upper={} aux={}/{}",
                rec["elig@3CE1"], rec["amountActive@3CE2"],
                rec["lower@3CF0"], rec["upper@3CF4"], rec["aux1@3CF8"], rec["aux2@3CFC"]
            );
            println!(
                "    帧: root=0x{:016X} base=0x{:016X} upper=0x{:016X}",
                fr.unwrap_or(0), frb.unwrap_or(0), fru.unwrap_or(0)
            );
            roots.push(rec);
        }
    }
    let report = json!({"slot": slot_rva, "table_off": table_off, "state": state,
        "table": table, "table_pages": spent, "candidates": checked, "roots": roots,
        "table_hex": table_hex});
    let path = dir.join("chain_report.json");
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("chain 完成: 读 {spent} 页, root 命中 {} → {}", roots.len(), path.display());
}

/// marker:查找 lookup 函数指针(静态 RVA 0xCF1EA0,写在 root+0x1CC8)的
/// 活体精确值,定位 PowerRoot 第一页 → 推出 root_va → 验指纹 → 解析全字段。
fn marker(dir: &PathBuf) {
    let budget = env::var("NVOC_POWER_WALK_BUDGET")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(2500);
    let (module, phys, root) = connect_walk();
    let image_end = module.base + 0x7370_0000u64;
    let lookup_marker = module.base + 0xCF1EA0;
    println!("lookup 指针活体值 = 0x{lookup_marker:016X}(module.base + 0xCF1EA0)");

    let mut queue: std::collections::VecDeque<u64> = std::collections::VecDeque::new();
    // 种子:.data 等节的指针一跳目标
    let mut seen = HashSet::new();
    for (_name, rva, size) in SECTIONS {
        let va = module.base + rva as u64;
        let mut holes = Vec::new();
        let buf = read_range(&phys, root, va, size as usize, &mut holes);
        for o in (0..buf.len().saturating_sub(7)).step_by(8) {
            let q = u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
            if q >= 0xFFFF_8000_0000_0000 && (q < module.base || q >= image_end)
                && seen.insert(q & !0xFFF)
            {
                queue.push_back(q & !0xFFF);
            }
        }
    }
    // 种子:枢纽页
    for h in ["0xFFFFC889A4A04000", "0xFFFFC889776E2000", "0xFFFFC88985AC0000", "0xFFFFC889710B9000"] {
        if let Ok(v) = u64::from_str_radix(h.trim_start_matches("0x"), 16) {
            queue.push_back(v & !0xFFF);
        }
    }

    let mut visited: HashSet<u64> = HashSet::new();
    let mut spent = 0usize;
    let mut roots = Vec::new();
    while let Some(page) = queue.pop_front() {
        if spent >= budget {
            println!("  达到预算 {budget},停止(队列 {})", queue.len());
            break;
        }
        if !visited.insert(page) || (page >= module.base && page < image_end) {
            continue;
        }
        let mut hole = Vec::new();
        let buf = read_range(&phys, root, page, 4096, &mut hole);
        if !hole.is_empty() {
            continue;
        }
        spent += 1;
        // 精确匹配 lookup 指针
        for o in (0..4088).step_by(8) {
            if u64::from_le_bytes(buf[o..o + 8].try_into().unwrap()) != lookup_marker {
                continue;
            }
            let root_va = page + o as u64 - 0x1CC8;
            println!("  ★ lookup 指针命中 page 0x{page:016X} +0x{o:X} → root_va = 0x{root_va:016X}");
            // 解析 root 全字段(root 可能跨页,逐字段读)
            let f = |roff: u64| -> Option<u32> {
                let mut h2 = Vec::new();
                let b = read_range(&phys, root, root_va + roff, 4, &mut h2);
                if h2.is_empty() && b.len() == 4 {
                    Some(u32::from_le_bytes(b[0..4].try_into().unwrap()))
                } else {
                    None
                }
            };
            let fr = translate(&phys, root, root_va + OFF_UPPER).map(|pa| pa & !0xFFF).ok();
            let fr0 = translate(&phys, root, root_va).map(|pa| pa & !0xFFF).ok();
            let rec = json!({
                "root_va": root_va,
                "marker_page": page, "marker_off": o,
                "frame@root": fr0, "frame@upper": fr,
                "init@3CE0": f(OFF_INIT).map(|b| b & 0xFF),
                "elig@3CE1": f(OFF_INIT + 1).map(|b| b & 0xFF),
                "amountActive@3CE2": f(OFF_INIT + 2).map(|b| b & 0xFF),
                "base@3CE4": f(OFF_BASE), "amount@3CE8": f(OFF_AMOUNT),
                "key@3CEC": f(OFF_KEY).map(|b| b & 0xFF),
                "lower@3CF0": f(OFF_LOWER), "upper@3CF4": f(OFF_UPPER),
                "aux1@3CF8": f(OFF_UPPER + 4), "aux2@3CFC": f(OFF_UPPER + 8),
                "registry@1C90": (|| { let mut h2 = Vec::new();
                    let b = read_range(&phys, root, root_va + 0x1C90, 8, &mut h2);
                    if h2.is_empty() && b.len() == 8 { Some(u64::from_le_bytes(b[0..8].try_into().unwrap())) } else { None } })(),
            });
            println!(
                "    init={} elig={} amountActive={} base={} amount={} key={} lower={} upper={} aux={}/{} registry=0x{:?}",
                rec["init@3CE0"], rec["elig@3CE1"], rec["amountActive@3CE2"],
                rec["base@3CE4"].as_u64().map(|v| v).unwrap_or(0xFFFF_FFFF),
                rec["amount@3CE8"], rec["key@3CEC"],
                rec["lower@3CF0"], rec["upper@3CF4"], rec["aux1@3CF8"], rec["aux2@3CFC"],
                rec["registry@1C90"].as_u64()
            );
            roots.push(rec);
        }
        // 入队指针(FIFO)
        for o in (0..4088).step_by(8) {
            let q = u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
            if q >= 0xFFFF_8000_0000_0000 && (q < module.base || q >= image_end) {
                let t = q & !0xFFF;
                if !visited.contains(&t) {
                    queue.push_back(t);
                }
            }
        }
    }
    let report = json!({"budget": budget, "spent": spent, "lookup_marker": lookup_marker, "roots": roots});
    let path = dir.join("marker_report.json");
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("marker 完成: 读 {spent} 页, lookup 命中 {} → {}", roots.len(), path.display());
}

/// graphwalk:从枢纽页(资源描述符注册表/锚表)做 FIFO 广度走查,预算内
/// 逐页扫 root 指纹、差分值、F7 记录(Board selector3 源数组标记,0x3F7)。
/// 命中页(含任意信号)存 hex 供离线判读。
fn graphwalk(tag: &str, dir: &PathBuf) {
    let budget = env::var("NVOC_POWER_WALK_BUDGET")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(1500);
    let hubs: Vec<u64> = env::var("NVOC_POWER_HUBS")
        .unwrap_or_else(|_| {
            "0xFFFFC889A4A04000,0xFFFFC889776E2000,0xFFFFC88985AC0000,0xFFFFC889710B9000"
                .to_string()
        })
        .split(',')
        .filter_map(|s| u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
        .map(|v| v & !0xFFF)
        .collect();
    let (module, phys, root) = connect_walk();
    let image_end = module.base + 0x7370_0000u64;

    let mut visited: HashSet<u64> = HashSet::new();
    let mut queue: std::collections::VecDeque<u64> = hubs.iter().copied().collect();
    let mut spent = 0usize;
    let mut hits = Vec::new();
    while let Some(page) = queue.pop_front() {
        if spent >= budget {
            println!("  达到预算 {budget},停止(队列 {})", queue.len());
            break;
        }
        if !visited.insert(page) || (page >= module.base && page < image_end) {
            continue;
        }
        let mut hole = Vec::new();
        let buf = read_range(&phys, root, page, 4096, &mut hole);
        if !hole.is_empty() {
            continue;
        }
        spent += 1;
        let scan = scan_page(&buf);
        let n_ptrs = (0..4088)
            .step_by(8)
            .filter(|&o| {
                u64::from_le_bytes(buf[o..o + 8].try_into().unwrap()) >= 0xFFFF_8000_0000_0000
            })
            .count();
        let signal = !scan.fingerprint.is_empty()
            || scan.value_hits.contains_key(&90000)
            || scan.value_hits.contains_key(&95000)
            || scan.f7_hits.len() >= 2;
        if signal {
            println!(
                "  ★ 0x{page:016X} fp={:?} vals={:?} f7={} ptrs={n_ptrs}",
                scan.fingerprint,
                scan.value_hits.iter().map(|(k, v)| (k, v.len())).collect::<Vec<_>>(),
                scan.f7_hits.len()
            );
            hits.push(json!({"va": page, "fingerprint": scan.fingerprint,
                "value_hits": scan.value_hits.iter()
                    .map(|(k, v)| (format!("{k}"), v.iter().take(64).collect::<Vec<_>>()))
                    .collect::<HashMap<_, _>>(),
                "f7_hits": scan.f7_hits, "bytes_hex": hex(&buf)}));
        }
        // 入队本页指针目标(FIFO 广度)
        for o in (0..4088).step_by(8) {
            let q = u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
            if q >= 0xFFFF_8000_0000_0000 && (q < module.base || q >= image_end) {
                let t = q & !0xFFF;
                if !visited.contains(&t) {
                    queue.push_back(t);
                }
            }
        }
    }
    let report = json!({"tag": tag, "budget": budget, "spent": spent, "hits": hits});
    let path = dir.join(format!("graphwalk_{tag}.json"));
    std::fs::write(&path, serde_json::to_string(&report).unwrap()).unwrap();
    println!("graphwalk 完成: 读 {spent} 页, 信号页 {} → {}", hits.len(), path.display());
}

/// trace:GPU-ID 指纹定位注册表页 → 相邻 {Major 指针, GPU ID} 对提取 Major →
/// 读 Major 页全部指针目标做 root 指纹检查。
/// 铁律:读取预算 NVOC_POWER_TRACE_BUDGET(默认 900 页),超即停。
fn trace(dir: &PathBuf) {
    let budget = env::var("NVOC_POWER_TRACE_BUDGET")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(900);
    let (module, phys, root) = connect_walk();
    let image_end = module.base + 0x7370_0000u64;

    // 1) 镜像数据节收集指针 → 读一跳目标页,找含 GPU-ID 的页
    let mut frontier: Vec<u64> = {
        let mut ptrs = Vec::new();
        let mut seen = HashSet::new();
        for (name, rva, size) in SECTIONS {
            let va = module.base + rva as u64;
            let mut holes = Vec::new();
            let buf = read_range(&phys, root, va, size as usize, &mut holes);
            for o in (0..buf.len().saturating_sub(7)).step_by(8) {
                let q = u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
                if q >= 0xFFFF_8000_0000_0000 && (q < module.base || q >= image_end)
                    && seen.insert(q & !0xFFF)
                {
                    ptrs.push(q & !0xFFF);
                }
            }
            let _ = name;
        }
        ptrs
    };
    frontier.sort_unstable();
    frontier.dedup();
    println!("一跳目标 {} 页(预算 {budget})", frontier.len());

    // GPU-ID 标记:vender:device 打包 dword 与 subsystem 设备 id
    const GPU_ID: u32 = 0x28E0_10DE;
    const SUBSYS_DEV: u32 = 0x0000_20BD;

    let mut spent = 0usize;
    let mut registry_pages: Vec<u64> = Vec::new();
    let mut majors: Vec<u64> = Vec::new();
    for page in &frontier {
        if spent >= budget {
            break;
        }
        let mut hole = Vec::new();
        let buf = read_range(&phys, root, *page, 4096, &mut hole);
        if !hole.is_empty() {
            continue;
        }
        spent += 1;
        let n_id = (0..4092)
            .step_by(4)
            .filter(|&o| {
                u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()) == GPU_ID
                    || u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()) == SUBSYS_DEV
            })
            .count();
        if n_id == 0 {
            continue;
        }
        println!("  GPU-ID 页 0x{page:016X}(标记 ×{n_id})");
        registry_pages.push(*page);
        // 提取 {内核指针, GPU-ID} 相邻对(指针在前,8 字节对齐)
        for o in (0..4080).step_by(8) {
            let q = u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
            let next = u32::from_le_bytes(buf[o + 8..o + 12].try_into().unwrap());
            let prev = if o >= 8 {
                u32::from_le_bytes(buf[o - 8..o - 4].try_into().unwrap())
            } else {
                0
            };
            if q >= 0xFFFF_8000_0000_0000 && (next == GPU_ID || next == SUBSYS_DEV || prev == GPU_ID || prev == SUBSYS_DEV)
            {
                if !majors.contains(&(q & !0xFFF)) {
                    majors.push(q & !0xFFF);
                    println!("    Major 候选 0x{:016X}(pair @+0x{o:X})", q & !0xFFF);
                }
            }
        }
    }
    println!("注册表页 {}, Major 候选 {}", registry_pages.len(), majors.len());

    // 2) 读 Major 页,对其指针目标做 root 指纹检查
    let mut roots = Vec::new();
    let mut candidates: Vec<u64> = Vec::new();
    let mut registry_hex: Vec<(u64, String)> = Vec::new();
    // 注册表页原始字节留档(离线定表项布局用)
    for &page in &registry_pages {
        let mut hole = Vec::new();
        let buf = read_range(&phys, root, page, 4096, &mut hole);
        if hole.is_empty() {
            registry_hex.push((page, hex(&buf)));
        }
    }
    for &major in majors.iter().take(16) {
        if spent >= budget {
            println!("  达到预算 {budget},Major 走查截断");
            break;
        }
        let mut hole = Vec::new();
        let buf = read_range(&phys, root, major, 4096, &mut hole);
        if !hole.is_empty() {
            println!("  Major 0x{major:016X} 不可读");
            continue;
        }
        spent += 1;
        let mut targets = (0..4088)
            .step_by(8)
            .map(|o| u64::from_le_bytes(buf[o..o + 8].try_into().unwrap()))
            .filter(|&q| q >= 0xFFFF_8000_0000_0000)
            .map(|q| q & !0xFFF)
            .collect::<Vec<_>>();
        targets.sort_unstable();
        targets.dedup();
        println!(
            "  Major 0x{major:016X}: +0x2510 处 = 0x{:016X}, 指针目标 {}",
            u64::from_le_bytes(buf[0x2510..0x2518].try_into().unwrap()),
            targets.len()
        );
        candidates.extend(targets.iter().copied());
    }
    candidates.sort_unstable();
    candidates.dedup();
    for page in &candidates {
        if spent >= budget {
            println!("  达到预算 {budget},root 检查截断");
            break;
        }
        let mut hole = Vec::new();
        let buf = read_range(&phys, root, *page, 4096, &mut hole);
        if !hole.is_empty() {
            continue;
        }
        spent += 1;
        let scan = scan_page(&buf);
        if !scan.fingerprint.is_empty() {
            for &o in &scan.fingerprint {
                let root_va = page + o as u64 - OFF_UPPER;
                let f = |roff: u64| -> Option<u32> {
                    let addr = root_va + roff;
                    let mut h2 = Vec::new();
                    let b = read_range(&phys, root, addr, 4, &mut h2);
                    if h2.is_empty() && b.len() == 4 {
                        Some(u32::from_le_bytes(b[0..4].try_into().unwrap()))
                    } else {
                        None
                    }
                };
                let fr = translate(&phys, root, *page).map(|pa| pa & !0xFFF).ok();
                roots.push(json!({
                    "root_va": root_va, "found_on_page": page, "upper_off_in_page": o,
                    "anchor_frame": fr,
                    "init@3CE0": f(OFF_INIT).map(|b| b & 0xFF),
                    "elig@3CE1": f(OFF_INIT + 1).map(|b| b & 0xFF),
                    "amountActive@3CE2": f(OFF_INIT + 2).map(|b| b & 0xFF),
                    "base@3CE4": f(OFF_BASE), "amount@3CE8": f(OFF_AMOUNT),
                    "key@3CEC": f(OFF_KEY).map(|b| b & 0xFF),
                    "lower@3CF0": f(OFF_LOWER), "upper@3CF4": f(OFF_UPPER),
                    "aux1@3CF8": f(OFF_UPPER + 4), "aux2@3CFC": f(OFF_UPPER + 8),
                }));
                println!("  ★ root 指纹命中 page 0x{page:016X} UPPER@+0x{o:X}");
            }
        } else if !scan.value_hits.is_empty() {
            println!(
                "  值页 page 0x{page:016X} vals={:?}",
                scan.value_hits.iter().map(|(k, v)| (k, v.len())).collect::<Vec<_>>()
            );
        }
    }
    let report = json!({"budget": budget, "spent": spent, "registry_pages": registry_pages,
        "registry_hex": registry_hex.iter().map(|(va, h)| json!({"va": va, "hex": h})).collect::<Vec<_>>(),
        "majors": majors, "roots": roots});
    let path = dir.join("trace_report.json");
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("trace 完成: 读 {spent} 页, root 命中 {} → {}", roots.len(), path.display());
    for r in &roots {
        println!(
            "  root@0x{:X}: init={} elig={} amountActive={} base={} amount={} key={} lower={} upper={} aux={}/{}",
            r["root_va"].as_u64().unwrap(),
            r["init@3CE0"], r["elig@3CE1"], r["amountActive@3CE2"],
            r["base@3CE4"], r["amount@3CE8"], r["key@3CEC"],
            r["lower@3CF0"], r["upper@3CF4"], r["aux1@3CF8"], r["aux2@3CFC"]
        );
    }
}

/// region:以 2MB 对齐基址起读 NVOC_POWER_REGION_SPAN(默认 0x200000=512 页)
/// 的整个池簇,存全量 hex(512×8KB≈4MB JSON)。差分在离线完成。
fn region(tag: &str, dir: &PathBuf) {
    let base = u64::from_str_radix(
        env::var("NVOC_POWER_REGION_BASE").expect("region 相位需 NVOC_POWER_REGION_BASE").trim_start_matches("0x"),
        16,
    ).unwrap() & !0x1F_FFFF;
    let span = env::var("NVOC_POWER_REGION_SPAN")
        .ok()
        .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x20_0000);
    let (_module, phys, root) = connect_walk();
    println!("region 0x{base:016X}..0x{:016X}({} 页)", base + span as u64, span / 0x1000);
    let mut pages = Vec::new();
    let mut ok = 0usize;
    let mut unreadable = 0usize;
    for off in (0..span).step_by(0x1000) {
        let va = base + off as u64;
        let mut hole = Vec::new();
        let buf = read_range(&phys, root, va, 4096, &mut hole);
        if hole.is_empty() {
            ok += 1;
            let scan = scan_page(&buf);
            if !scan.fingerprint.is_empty() || !scan.value_hits.is_empty() {
                println!(
                    "  值页 0x{va:016X} fp={:?} vals={:?} f7={}",
                    scan.fingerprint,
                    scan.value_hits.iter().map(|(k, v)| (k, v.len())).collect::<Vec<_>>(),
                    scan.f7_hits.len()
                );
            }
            pages.push(json!({"va": va, "bytes_hex": hex(&buf)}));
        } else {
            unreadable += 1;
        }
    }
    let report = json!({"tag": tag, "base": base, "span": span, "readable": ok,
        "unreadable": unreadable, "pages": pages});
    let path = dir.join(format!("region_{tag}.json"));
    std::fs::write(&path, serde_json::to_string(&report).unwrap()).unwrap();
    println!(
        "region 完成: 可读 {ok}/{} 页 → {}",
        span / 0x1000,
        path.display()
    );
}

/// hub:读锚页全部内核指针目标页(≤ NVOC_POWER_HUB_CAP/默认 256 页)做
/// 指纹+值扫描 —— 锚页(功率通道控制表)是枢纽对象,root/Board 应在其
/// 指针邻域内。二跳仅在需要时用 NVOC_POWER_HUB_HOP2=1 开启。
fn hub(dir: &PathBuf) {
    let va_src = env::var("NVOC_POWER_HUB_VA")
        .or_else(|_| env::var("NVOC_POWER_READ_VA"))
        .expect("hub 相位需 NVOC_POWER_HUB_VA");
    let anchor = u64::from_str_radix(va_src.trim_start_matches("0x"), 16).unwrap() & !0xFFF;
    let cap = env::var("NVOC_POWER_HUB_CAP")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(256);
    let hop2 = env::var("NVOC_POWER_HUB_HOP2").map(|v| v == "1").unwrap_or(false);
    let (_module, phys, root) = connect_walk();
    let mut hole = Vec::new();
    let anchor_buf = read_range(&phys, root, anchor, 4096, &mut hole);
    assert!(hole.is_empty(), "锚页不可读");
    let mut targets: Vec<u64> = (0..4088)
        .step_by(8)
        .map(|o| u64::from_le_bytes(anchor_buf[o..o + 8].try_into().unwrap()))
        .filter(|&q| q >= 0xFFFF_8000_0000_0000)
        .map(|q| q & !0xFFF)
        .filter(|&p| p != anchor)
        .collect();
    targets.sort_unstable();
    targets.dedup();
    targets.truncate(cap);
    println!("锚 0x{anchor:016X} 指针目标页 {} 个", targets.len());

    let mut queue = targets.clone();
    if hop2 {
        // 二跳:仅从"含 ≥16 内核指针"的一跳页继续(结构页),目标限制在锚的 256MB 邻域
        let mut extra = Vec::new();
        for &t in &targets {
            let mut h2 = Vec::new();
            let buf = read_range(&phys, root, t, 4096, &mut h2);
            if !h2.is_empty() {
                continue;
            }
            let n = (0..4088)
                .step_by(8)
                .filter(|&o| {
                    u64::from_le_bytes(buf[o..o + 8].try_into().unwrap()) >= 0xFFFF_8000_0000_0000
                })
                .count();
            if n >= 16 {
                for o in (0..4088).step_by(8) {
                    let q = u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
                    if q >= 0xFFFF_8000_0000_0000
                        && q & !0xFFF != t
                        && (q & !0xFFF).wrapping_sub(anchor) < 0x1000_0000
                    {
                        extra.push(q & !0xFFF);
                    }
                }
            }
        }
        extra.sort_unstable();
        extra.dedup();
        println!("二跳补充目标 {} 个(预算内截断)", extra.len().min(cap));
        extra.truncate(cap);
        queue.extend(extra);
        queue.sort_unstable();
        queue.dedup();
    }

    let mut roots = Vec::new();
    let mut scanned = 0usize;
    for page in &queue {
        let mut h = Vec::new();
        let buf = read_range(&phys, root, *page, 4096, &mut h);
        if !h.is_empty() {
            continue;
        }
        scanned += 1;
        let scan = scan_page(&buf);
        if !scan.fingerprint.is_empty() {
            for &o in &scan.fingerprint {
                let root_va = page + o as u64 - OFF_UPPER;
                let f = |roff: u64| -> Option<u32> {
                    let addr = root_va + roff;
                    let mut hole2 = Vec::new();
                    let b = read_range(&phys, root, addr, 4, &mut hole2);
                    if hole2.is_empty() && b.len() == 4 {
                        Some(u32::from_le_bytes(b[0..4].try_into().unwrap()))
                    } else {
                        None
                    }
                };
                let fr = translate(&phys, root, *page).map(|pa| pa & !0xFFF).ok();
                roots.push(json!({
                    "root_va": root_va,
                    "found_on_page": page,
                    "upper_off_in_page": o,
                    "anchor_frame": fr,
                    "init@3CE0": f(OFF_INIT).map(|b| b & 0xFF),
                    "elig@3CE1": f(OFF_INIT + 1).map(|b| b & 0xFF),
                    "amountActive@3CE2": f(OFF_INIT + 2).map(|b| b & 0xFF),
                    "base@3CE4": f(OFF_BASE),
                    "amount@3CE8": f(OFF_AMOUNT),
                    "key@3CEC": f(OFF_KEY).map(|b| b & 0xFF),
                    "lower@3CF0": f(OFF_LOWER),
                    "upper@3CF4": f(OFF_UPPER),
                    "aux1@3CF8": f(OFF_UPPER + 4),
                    "aux2@3CFC": f(OFF_UPPER + 8),
                }));
            }
            println!(
                "  指纹命中 page 0x{page:016X} fp={:?} values={:?}",
                scan.fingerprint,
                scan.value_hits.iter().map(|(k, v)| (k, v.len())).collect::<Vec<_>>()
            );
        } else if scan.value_hits.contains_key(&90000) || scan.value_hits.contains_key(&95000) {
            println!(
                "  差分值页 page 0x{page:016X} values={:?}",
                scan.value_hits.iter().map(|(k, v)| (k, v.len())).collect::<Vec<_>>()
            );
        }
    }
    let report = json!({"anchor": anchor, "scanned": scanned, "roots": roots});
    let path = dir.join("hub_report.json");
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("hub 完成: 扫 {scanned} 页, root 命中 {} → {}", roots.len(), path.display());
    for r in &roots {
        println!(
            "  root@0x{:X}: init={} elig={} amountActive={} base={} amount={} key={} lower={} upper={} aux={}/{}",
            r["root_va"].as_u64().unwrap(),
            r["init@3CE0"], r["elig@3CE1"], r["amountActive@3CE2"],
            r["base@3CE4"], r["amount@3CE8"], r["key@3CEC"],
            r["lower@3CF0"], r["upper@3CF4"], r["aux1@3CF8"], r["aux2@3CFC"]
        );
    }
}

/// near:以已知策略值池页为锚,读其邻近窗口逐页做 root 指纹(池簇定位,
/// 每次 ≤ ~50 页映射,与两次安全跑通的单跳扫描同量级)。
/// env: NVOC_POWER_NEAR_VA(锚页)、NVOC_POWER_NEAR_BEFORE/默认 0x4000、
///       NVOC_POWER_NEAR_AFTER/默认 0x7000。
fn near(dir: &PathBuf) {
    let va_src = env::var("NVOC_POWER_NEAR_VA")
        .or_else(|_| env::var("NVOC_POWER_READ_VA"))
        .expect("near 相位需 NVOC_POWER_NEAR_VA(或 NVOC_POWER_READ_VA)");
    let anchor = u64::from_str_radix(va_src.trim_start_matches("0x"), 16).unwrap() & !0xFFF;
    let before = env::var("NVOC_POWER_NEAR_BEFORE")
        .ok()
        .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x8000) as u64;
    let after = env::var("NVOC_POWER_NEAR_AFTER")
        .ok()
        .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x10000) as u64;
    let (_module, phys, root) = connect_walk();
    let start = anchor.saturating_sub(before);
    let end = anchor + after;
    println!("邻域扫描 0x{start:016X}..0x{end:016X}(锚 0x{anchor:016X}),共 {} 页", (end - start) / 0x1000);
    let mut hits = Vec::new();
    let mut pages = Vec::new();
    let mut page = start;
    while page < end {
        let mut hole = Vec::new();
        let buf = read_range(&phys, root, page, 4096, &mut hole);
        if hole.is_empty() {
            let scan = scan_page(&buf);
            let n_ptrs = (0..4088)
                .step_by(8)
                .filter(|&o| {
                    u64::from_le_bytes(buf[o..o + 8].try_into().unwrap()) >= 0xFFFF_8000_0000_0000
                })
                .count();
            if !scan.fingerprint.is_empty()
                || scan.value_hits.contains_key(&90000)
                || scan.value_hits.contains_key(&95000)
                || !scan.value_hits.is_empty()
            {
                println!(
                    "  页 0x{page:016X} fp={:?} values={:?} f7n={} ptrs={n_ptrs}",
                    scan.fingerprint,
                    scan.value_hits.iter().map(|(k, v)| (k, v.len())).collect::<Vec<_>>(),
                    scan.f7_hits.len()
                );
            }
            hits.push(json!({
                "page_va": page,
                "fingerprint": scan.fingerprint,
                "value_hits": scan.value_hits.iter()
                    .map(|(k, v)| (format!("{k}"), v.iter().take(64).collect::<Vec<_>>()))
                    .collect::<HashMap<_, _>>(),
                "f7_hits_n": scan.f7_hits.len(),
                "kernel_ptrs": n_ptrs,
                "bytes_hex": hex(&buf),
            }));
            pages.push((page, buf));
        }
        page += 0x1000;
    }
    // 对指纹命中页:解析 root 八字段 + 页帧,直接产出 L2.3 档案数据
    let mut roots = Vec::new();
    for (page_va, buf) in &pages {
        for &o in &scan_page(buf).fingerprint {
            let root_va = page_va + o as u64 - OFF_UPPER;
            let put = |off: usize| -> u32 {
                u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
            };
            let in_page = |off: usize| off + 4 <= 4096;
            let f = |roff: u64| -> Option<u32> {
                // root 字段可能落在锚页之外,回读
                let addr = root_va + roff;
                if addr & !0xFFF == *page_va && in_page((addr & 0xFFF) as usize) {
                    Some(put((addr & 0xFFF) as usize))
                } else {
                    let mut hole = Vec::new();
                    let b = read_range(&phys, root, addr, 4, &mut hole);
                    if hole.is_empty() && b.len() == 4 {
                        Some(u32::from_le_bytes(b[0..4].try_into().unwrap()))
                    } else {
                        None
                    }
                }
            };
            let fr = translate(&phys, root, *page_va).map(|pa| pa & !0xFFF).ok();
            roots.push(json!({
                "root_va": root_va,
                "anchor_page_va": page_va,
                "anchor_frame": fr,
                "init@3CE0": f(OFF_INIT).map(|b| b & 0xFF),
                "elig@3CE1": f(OFF_INIT + 1).map(|b| b & 0xFF),
                "amountActive@3CE2": f(OFF_INIT + 2).map(|b| b & 0xFF),
                "base@3CE4": f(OFF_BASE),
                "amount@3CE8": f(OFF_AMOUNT),
                "key@3CEC": f(OFF_KEY).map(|b| b & 0xFF),
                "lower@3CF0": f(OFF_LOWER),
                "upper@3CF4": f(OFF_UPPER),
                "aux1@3CF8": f(OFF_UPPER + 4),
                "aux2@3CFC": f(OFF_UPPER + 8),
            }));
        }
    }
    let report = json!({
        "anchor": anchor, "start": start, "end": end,
        "hits": hits, "roots": roots,
    });
    let path = dir.join("near_report.json");
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!("near 完成: 命中 {} 页,root 解析 {} → {}", hits.len(), roots.len(), path.display());
    for r in &roots {
        println!("  root@0x{:X}: init={} elig={} amountActive={} base={} amount={} key={} lower={} upper={} aux={}/{}",
            r["root_va"].as_u64().unwrap(),
            r["init@3CE0"], r["elig@3CE1"], r["amountActive@3CE2"],
            r["base@3CE4"], r["amount@3CE8"], r["key@3CEC"],
            r["lower@3CF0"], r["upper@3CF4"], r["aux1@3CF8"], r["aux2@3CFC"]);
        println!("    anchor 页帧: 0x{:X}", r["anchor_frame"].as_u64().unwrap_or(0));
    }
}

fn snapshot(tag: &str, dir: &PathBuf) {
    let (module, phys, root) = connect_walk();
    let image_end = module.base + 0x7370_0000u64;

    // 1) 镜像数据节:读内容,收集指针 + 值命中
    let mut pointers: Vec<(u64, u64)> = Vec::new(); // (来源 VA, 目标 VA)
    let mut section_stats = Vec::new();
    let mut seen_target: HashSet<u64> = HashSet::new();
    let mut value_only_pages: HashMap<u64, PageScan> = HashMap::new();
    let mut candidate_bytes: HashMap<u64, ([u8; 4096], PageScan)> = HashMap::new();
    let mut holes_total = 0usize;

    for (name, rva, size) in SECTIONS {
        let va = module.base + rva as u64;
        let mut holes = Vec::new();
        let buf = read_range(&phys, root, va, size as usize, &mut holes);
        holes_total += holes.len();
        let mut ptrs = 0usize;
        for o in (0..buf.len() - 7).step_by(8) {
            let q = u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
            if q >= 0xFFFF_8000_0000_0000 && (q < module.base || q >= image_end) {
                if seen_target.insert(q & !0xFFF) {
                    pointers.push((va + o as u64, q));
                }
                ptrs += 1;
            }
        }
        // 节内自身的值/指纹命中(镜像 .data 也可能承载镜像拷贝)
        for o in (0..buf.len() - 4095).step_by(4096) {
            let page_va = va + o as u64;
            let scan = scan_page(&buf[o..o + 4096]);
            if !scan.fingerprint.is_empty() {
                candidate_bytes.insert(page_va, {
                    let mut b = [0u8; 4096];
                    b.copy_from_slice(&buf[o..o + 4096]);
                    (b, scan)
                });
            } else if !scan.value_hits.is_empty() && value_only_pages.len() < 2048 {
                value_only_pages.insert(page_va, scan);
            }
        }
        println!("  节 {name}: 指针 {ptrs},洞 {}", holes.len());
        section_stats.push(json!({"name": name, "rva": rva, "size": size, "holes": holes.len()}));
    }
    println!("镜像指针(去重目标页): {}", pointers.len());

    // 2) 传递式 BFS 走查:镜像指针的池目标页继续取指针,直到队列枯竭或上限。
    //    PowerRoot 离镜像指针 2-3 跳(Major+0x2510 在池内),一跳走查够不着。
    let mut visited: HashSet<u64> = HashSet::new();
    let mut frontier: Vec<u64> = pointers.iter().map(|(_, t)| t & !0xFFF).collect();
    frontier.sort_unstable();
    frontier.dedup();
    let mut scanned = 0usize;
    // 硬安全预算:BSOD 教训(2026-10-06,25 万页 BFS → KMODE_EXCEPTION_NOT_HANDLED)。
    // 默认 2000 页 = 两次安全跑通的单跳走查量级;要放大必须显式设环境变量并在
    // 任务书记录。超过预算即停,宁可少走不可蓝屏。
    let max_pages = env::var("NVOC_POWER_WALK_PAGES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(2000);
    const MAX_PAGES: usize = 250_000;
    let _ = MAX_PAGES;
    while let Some(page_va) = frontier.pop() {
        if scanned >= max_pages {
            println!("  达到安全预算 {max_pages} 页,停止走查(剩余队列 {})", frontier.len());
            break;
        }
        if !visited.insert(page_va) || visited.len() > MAX_PAGES {
            continue;
        }
        if page_va >= module.base && page_va < image_end {
            continue; // 镜像内页已由节扫描覆盖
        }
        let mut hole = Vec::new();
        let page = read_range(&phys, root, page_va, 4096, &mut hole);
        if !hole.is_empty() {
            continue;
        }
        scanned += 1;
        let scan = scan_page(&page);
        let is_changing =
            scan.value_hits.contains_key(&90000) || scan.value_hits.contains_key(&95000);
        if !scan.fingerprint.is_empty() || is_changing {
            if candidate_bytes.len() < 4096 {
                let mut b = [0u8; 4096];
                b.copy_from_slice(&page);
                candidate_bytes.insert(page_va, (b, scan));
            }
        } else if !scan.value_hits.is_empty() && value_only_pages.len() < 4096 {
            value_only_pages.insert(page_va, scan);
        }
        // 取本页指针入队
        for o in (0..4088).step_by(8) {
            let q = u64::from_le_bytes(page[o..o + 8].try_into().unwrap());
            if q >= 0xFFFF_8000_0000_0000 && (q < module.base || q >= image_end) {
                let t = q & !0xFFF;
                if !visited.contains(&t) {
                    frontier.push(t);
                }
            }
        }
        if scanned % 8192 == 0 {
            println!(
                "  已扫 {scanned} 页,队列 {},候选 {},值页 {}",
                frontier.len(),
                candidate_bytes.len(),
                value_only_pages.len()
            );
        }
    }
    println!("BFS 完成: 扫描 {scanned} 页,visited {}", visited.len());

    // 3) 候选页的来源指针边(后续回溯 Major/DriverGlobal 用)
    let cand_vas: HashSet<u64> = candidate_bytes.keys().copied().collect();
    let mut edges: HashMap<u64, Vec<u64>> = HashMap::new();
    for (src_va, target) in &pointers {
        let page_va = target & !0xFFF;
        if cand_vas.contains(&page_va) {
            edges.entry(page_va).or_default().push(*src_va);
        }
    }

    // 4) 序列化
    let cand_json: Vec<Value> = candidate_bytes
        .iter()
        .map(|(va, (bytes, scan))| {
            json!({
                "page_va": va,
                "fingerprint": scan.fingerprint,
                "f7_hits": scan.f7_hits.iter().take(64).collect::<Vec<_>>(),
                "value_hits": scan.value_hits.iter()
                    .map(|(k, v)| (format!("{k}"), v.iter().take(128).collect::<Vec<_>>()))
                    .collect::<HashMap<_, _>>(),
                "edges": edges.get(va).cloned().unwrap_or_default(),
                "bytes_hex": hex(bytes),
            })
        })
        .collect();
    let vo_json: Vec<Value> = value_only_pages
        .iter()
        .take(2048)
        .map(|(va, scan)| {
            json!({
                "page_va": va,
                "fingerprint": scan.fingerprint,
                "value_hits": scan.value_hits.iter()
                    .map(|(k, v)| (format!("{k}"), v.iter().take(64).collect::<Vec<_>>()))
                    .collect::<HashMap<_, _>>(),
            })
        })
        .collect();

    let report = json!({
        "tag": tag,
        "module_base": module.base,
        "walk_root": root,
        "sections": section_stats,
        "hole_pages": holes_total,
        "pointers_collected": pointers.len(),
        "pool_pages_scanned": scanned,
        "root_candidates": cand_json.iter()
            .filter(|p| !p["fingerprint"].as_array().unwrap().is_empty())
            .map(|p| json!({"page_va": p["page_va"], "fingerprint": p["fingerprint"], "edges": p["edges"]}))
            .collect::<Vec<_>>(),
        "candidate_pages": cand_json,
        "value_only_pages": vo_json,
    });
    let path = dir.join(format!("{tag}.snapshot.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!(
        "快照完成: 候选页 {} ,值页 {} ,root 指纹命中 {} → {}",
        candidate_bytes.len(),
        value_only_pages.len(),
        report["root_candidates"].as_array().unwrap().len(),
        path.display()
    );
}

fn diff(tag: &str, dir: &PathBuf) {
    let base_tag = env::var("NVOC_POWER_DIFF_BASE").unwrap_or_else(|_| "a".into());
    let base_path = dir.join(format!("{base_tag}.snapshot.json"));
    let base: Value =
        serde_json::from_str(&std::fs::read_to_string(&base_path).unwrap_or_else(|e| {
            panic!("读基线快照 {} 失败: {e}", base_path.display())
        }))
        .unwrap();
    let (_module, phys, root) = connect_walk();

    let mut changed_pages: Vec<Value> = Vec::new();
    let mut pages_checked = 0usize;
    for entry in base["candidate_pages"].as_array().unwrap() {
        let page_va = entry["page_va"].as_u64().unwrap();
        let old_bytes = hex_to_bytes(entry["bytes_hex"].as_str().unwrap());
        let mut hole = Vec::new();
        let new_bytes = read_range(&phys, root, page_va, 4096, &mut hole);
        if !hole.is_empty() {
            changed_pages.push(json!({"page_va": page_va, "status": "unreadable_now"}));
            continue;
        }
        pages_checked += 1;
        let mut changes = Vec::new();
        for o in (0..4092).step_by(4) {
            let a = u32::from_le_bytes(old_bytes[o..o + 4].try_into().unwrap());
            let b = u32::from_le_bytes(new_bytes[o..o + 4].try_into().unwrap());
            if a != b {
                changes.push(json!({"off": o, "old": a, "new": b}));
            }
        }
        // 字节级(非对齐)变化兜底
        let mut byte_changes = 0usize;
        for i in 0..4096 {
            if old_bytes[i] != new_bytes[i] {
                byte_changes += 1;
            }
        }
        if !changes.is_empty() || byte_changes != changes.len() * 4 {
            let fp = root_fingerprint(&new_bytes);
            let has_f7 = (0..4094).step_by(2).any(|o| {
                u16::from_le_bytes(new_bytes[o..o + 2].try_into().unwrap()) == F7_RECORD
            });
            changed_pages.push(json!({
                "page_va": page_va,
                "status": "changed",
                "dword_changes": changes.iter().take(256).collect::<Vec<_>>(),
                "n_dword_changes": changes.len(),
                "n_byte_changes": byte_changes,
                "root_fingerprint_now": fp,
                "has_f7_record": has_f7,
            }));
        }
    }
    // 值-only 页:只比对记录的偏移
    for entry in base["value_only_pages"].as_array().unwrap() {
        let page_va = entry["page_va"].as_u64().unwrap();
        let mut hole = Vec::new();
        let new_bytes = read_range(&phys, root, page_va, 4096, &mut hole);
        if !hole.is_empty() {
            continue;
        }
        pages_checked += 1;
        let mut changes = Vec::new();
        for (val_str, offs) in entry["value_hits"].as_object().unwrap() {
            let old_val: u32 = val_str.parse().unwrap();
            for off in offs.as_array().unwrap() {
                let o = off.as_u64().unwrap() as usize;
                let b = u32::from_le_bytes(new_bytes[o..o + 4].try_into().unwrap());
                if b != old_val {
                    changes.push(json!({"off": o, "old": old_val, "new": b}));
                }
            }
        }
        if !changes.is_empty() {
            changed_pages.push(json!({
                "page_va": page_va, "status": "changed_value_only",
                "dword_changes": changes.iter().take(256).collect::<Vec<_>>(),
            }));
        }
    }

    let report = json!({
        "base_tag": base_tag, "diff_tag": tag,
        "pages_checked": pages_checked,
        "changed_pages": changed_pages,
    });
    let path = dir.join(format!("diff_{base_tag}_{tag}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!(
        "diff 完成: 复检 {pages_checked} 页,变化页 {} → {}",
        changed_pages.len(),
        path.display()
    );
    for page in &changed_pages {
        println!(
            "  变化: page 0x{:X} [{}] {}",
            page["page_va"].as_u64().unwrap_or(0),
            page["status"].as_str().unwrap_or("?"),
            page["dword_changes"]
                .as_array()
                .map(|c| c.iter().map(|x| format!("+{:X}: {}→{}", x["off"].as_u64().unwrap(), x["old"], x["new"])).collect::<Vec<_>>().join(", "))
                .unwrap_or_default()
        );
    }
}

fn read_object(dir: &PathBuf) {
    let va = u64::from_str_radix(
        env::var("NVOC_POWER_READ_VA").expect("read 相位需 NVOC_POWER_READ_VA").trim_start_matches("0x"),
        16,
    ).unwrap();
    let len = env::var("NVOC_POWER_READ_LEN")
        .ok()
        .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x4000);
    let (_module, phys, root) = connect_walk();
    let mut hole = Vec::new();
    let buf = read_range(&phys, root, va, len, &mut hole);
    let field = |off: u64| -> Value {
        let o = off as usize;
        if o + 4 <= buf.len() {
            json!(u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()))
        } else {
            Value::Null
        }
    };
    let mut frames = Vec::new();
    for off in (0..len as u64).step_by(4096) {
        if let Ok(pa) = translate(&phys, root, va + off) {
            frames.push(json!({"va": va + off, "frame": pa & !0xFFF}));
        }
    }
    let report = json!({
        "va": va, "len": len, "holes": hole,
        "frames": frames,
        "init@3CE0": field(OFF_INIT) , "elig@3CE1": buf.get(OFF_INIT as usize +1).map(|b| *b as u32),
        "amountActive@3CE2": buf.get(OFF_INIT as usize +2).map(|b| *b as u32),
        "base@3CE4": field(OFF_BASE), "amount@3CE8": field(OFF_AMOUNT),
        "key@3CEC": field(OFF_KEY), "lower@3CF0": field(OFF_LOWER),
        "upper@3CF4": field(OFF_UPPER), "aux1@3CF8": field(OFF_UPPER+4), "aux2@3CFC": field(OFF_UPPER+8),
        "bytes_hex": hex(&buf),
    });
    let path = dir.join(format!("read_{va:016X}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    println!(
        "root 对象 @0x{va:016X}: init={} elig={} amountActive={} base={} amount={} key={} lower={} upper={} aux={}/{} → {}",
        report["init@3CE0"], report["elig@3CE1"], report["amountActive@3CE2"],
        report["base@3CE4"], report["amount@3CE8"], report["key@3CEC"],
        report["lower@3CF0"], report["upper@3CF4"], report["aux1@3CF8"], report["aux2@3CFC"],
        path.display()
    );
    for f in &frames {
        println!("  页 0x{:X} → 帧 0x{:X}", f["va"].as_u64().unwrap(), f["frame"].as_u64().unwrap());
    }
}

fn hex_to_bytes(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

/// 指纹/扫描逻辑的离线自检(不触驱动):合成一页 PowerRoot 尾部布局。
#[test]
fn scan_page_fingerprint_synthetic() {
    let mut buf = vec![0u8; 4096];
    // root 页内对齐:UPPER @ o=0xCF4 → base@0xCE4, key@0xCEC, init@0xCE0
    let put = |buf: &mut [u8], off: usize, v: u32| buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    put(&mut buf, 0xCE4, 100000);
    put(&mut buf, 0xCF4, 100000);
    put(&mut buf, 0xCEC, 0x0000_0005); // key=5(<0x40)
    buf[0xCE0] = 1;
    buf[0xCE1] = 0;
    buf[0xCE2] = 0;
    put(&mut buf, 0xCE8, 0); // amount=0(出厂)
    put(&mut buf, 0xCF0, 5000); // LOWER
    buf[0x200..0x204].copy_from_slice(&90000u32.to_le_bytes());
    buf[0x300..0x302].copy_from_slice(&0x3F7u16.to_le_bytes()); // F7 记录(独立槽)
    let scan = scan_page(&buf);
    assert_eq!(scan.fingerprint, vec![0xCF4], "root 指纹应精确命中 UPPER 槽");
    assert_eq!(scan.value_hits[&100000], vec![0xCE4, 0xCF4]);
    assert_eq!(scan.value_hits[&90000], vec![0x200]);
    assert_eq!(scan.value_hits[&5000], vec![0xCF0]);
    assert!(scan.f7_hits.contains(&0x300));

    // 负例:init≠1 不得误报;base=-1 哨兵(610 出厂可能态)仍应命中
    let mut bad2 = buf.clone();
    bad2[0xCE0] = 0;
    assert!(scan_page(&bad2).fingerprint.is_empty(), "init=0 不得命中");
    let mut sentinel = buf.clone();
    put(&mut sentinel, 0xCE4, 0xFFFFFFFF);
    assert_eq!(scan_page(&sentinel).fingerprint, vec![0xCF4], "base=-1 哨兵不应阻止命中");
}
