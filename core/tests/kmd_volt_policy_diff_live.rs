//! kmd 电压墙策略对象定位 harness(2026-11-14;50 系电压墙 kmd 内核道第一步)。
//!
//! 背景:Blackwell/621 上 melonVolt payload 写用户态被丢(probe C/D/F 臂实锤),
//! 唯一活车道 = legacy boost %(`ceiling = StandardMax+(BoostMax−StandardMax)·%`,
//! 100 % 钳在 VBIOS 墙)。要越过 VBIOS 墙必须内核态直改策略对象——本 harness
//! 在 2070/621.01(与 5060 Ti 同驱动版本,结构可移植)上定位电压策略块的
//! 内核地址与字段偏移。
//!
//! scan 阶段(单进程闭环,需要管理员 + PMXDRV 服务):
//!   1. nvapi 读活体六值/vboost(扰动基线);
//!   2. 活体镜像 PE 头动态枚举可写节(无跨代硬编码);
//!   3. 逐页扫 {vrm_wall, min_hold} 同页元组(双稀有值,噪声门)+ 可选内核
//!      指针一跳(NVOC_VOLT_KMD_HOP=1);
//!   4. 候选页快照 → boost 0 扰动(进程内 nvapi)→ 重扫 diff → 还原 boost;
//!   5. 分类:随扰动移动字段(target/current/回显)vs 静态字段(vbios/vrm 墙);
//!   6. JSON + 命中偏移 ±0x20 hexdump。
//!
//! write 阶段(env 四件套全设才动):NVOC_VOLT_WRITE_VA(16 进制完整 VA)+
//!   NVOC_VOLT_WRITE_OLD(期望现值)+ NVOC_VOLT_WRITE_NEW + NVOC_VOLT_WRITE_ARM=YES
//!   → translate→map→单 u32 volatile 写→同窗/重映射/页表三级读回→
//!   nvapi volt_rails status 随动读回→自动还原并终验。
//!
//! 判读:C/D 臂同款纪律——diff 空 ⇒ 策略对象不在镜像节(转池邻域/指针跳);
//! 随动字段即 target/current 内核存储;静态 1020000 副本即 vbios 墙补丁候选。

#![cfg(windows)]

use nvapi::PhysicalGpu;
use nvoc_core::kmd::board::{BoardLiveValues, locate_board_window_candidates};
use nvoc_core::kmd::layout_probe::probe as probe_layout;
use nvoc_core::kmd::pagewalk::{
    PeFingerprint, PhysicalMemory, discover_root, find_loaded_module, is_kernel_pointer,
    read_virtual, translate,
};
use nvoc_core::kmd::pmxdrv::{PmxDrv, PmxDrvPhysMem};
use serde_json::json;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::env;

// ---------------- 物理( borrowed from kmd_power_policy_diff_live ) ----------------

/// 读 HKLM\HARDWARE\RESOURCEMAP\System Resources\Physical Memory 提取 RAM
/// 范围(BSOD 教训:映射前先白名单,拒绝 MMIO/保留帧)。
fn ram_ranges() -> Vec<(u64, u64)> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegOpenKeyExW, RegQueryValueExW,
    };
    const PATH: &str = r"HARDWARE\RESOURCEMAP\System Resources\Physical Memory";
    let wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let mut buf = vec![0u8; 65536];
    let mut size = buf.len() as u32;
    let mut value_type: u32 = 0;
    let mut hkey: *mut std::ffi::c_void = std::ptr::null_mut();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(PATH).as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        )
    };
    if status != 0 {
        println!("  warning: Physical Memory key open failed 0x{status:X}, whitelist degraded");
        return Vec::new();
    }
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            wide(".Translated").as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            buf.as_mut_ptr(),
            &mut size,
        )
    };
    unsafe { RegCloseKey(hkey) };
    if status != 0 {
        println!("  warning: .Translated read failed 0x{status:X}, whitelist degraded");
        return Vec::new();
    }
    buf.truncate(size as usize);
    let le = |b: &[u8]| u64::from_le_bytes(b[..8].try_into().unwrap());
    let ranges = Vec::new();
    if buf.len() < 4 {
        return ranges;
    }
    let count = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
    let mut best: Option<(Vec<(u64, u64)>, &str)> = None;
    for (stride, union_off, label) in [
        (20usize, 8usize, "stride20/union8"),
        (20usize, 4usize, "stride20/union4"),
        (24, 8, "stride24/union8"),
    ] {
        let mut rs = Vec::new();
        let mut off = 4usize;
        for _ in 0..count {
            if off + 16 > buf.len() {
                break;
            }
            let partial = u32::from_le_bytes(buf[off + 12..off + 16].try_into().unwrap()) as usize;
            off += 16;
            for _ in 0..partial {
                if off + stride > buf.len() {
                    break;
                }
                if buf[off] == 3 {
                    let start = le(&buf[off + union_off..off + union_off + 8]);
                    let len = u32::from_le_bytes(
                        buf[off + union_off + 8..off + union_off + 12]
                            .try_into()
                            .unwrap(),
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
            "  RAM layout {label}: {} segments, {:.2} GiB, covers_low={covers_low}",
            rs.len(),
            total as f64 / (1 << 30) as f64
        );
        if covers_low && total > (1 << 30) {
            best = match &best {
                None => Some((rs, label)),
                Some((old, _)) => {
                    if total > old.iter().map(|(s, e)| e - s).sum::<u64>() {
                        Some((rs, label))
                    } else {
                        None
                    }
                }
            };
        }
    }
    if let Some((rs, label)) = best {
        println!("  RAM table using {label}");
        rs
    } else {
        println!("  warning: no usable RAM layout, whitelist degraded to no-filter");
        Vec::new()
    }
}

#[allow(dead_code)]
struct CachedPhys<'a> {
    inner: &'a PmxDrvPhysMem<'a>,
    cache: RefCell<HashMap<u64, [u8; 4096]>>,
    failed: RefCell<HashSet<u64>>,
    ram_ranges: Vec<(u64, u64)>,
    rejected: RefCell<usize>,
}

impl<'a> CachedPhys<'a> {
    fn new(inner: &'a PmxDrvPhysMem<'a>) -> Self {
        let ram = ram_ranges();
        Self {
            inner,
            cache: RefCell::new(HashMap::new()),
            failed: RefCell::new(HashSet::new()),
            rejected: RefCell::new(0),
            ram_ranges: ram,
        }
    }
    fn in_ram(&self, frame: u64) -> bool {
        self.ram_ranges.is_empty()
            || self
                .ram_ranges
                .iter()
                .any(|(s, e)| frame >= *s && frame < *e)
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
                out.copy_from_slice(&page[off..off + 8]);
                self.cache.borrow_mut().insert(frame, page);
                Ok(())
            }
            Err(e) => {
                self.failed.borrow_mut().insert(frame);
                Err(e)
            }
        }
    }
}

fn connect_walk() -> (
    nvoc_core::kmd::pagewalk::LoadedModule,
    CachedPhys<'static>,
    u64,
    &'static PmxDrv,
) {
    let module = find_loaded_module("nvlddmkm.sys").expect("nvlddmkm.sys not in module list");
    println!(
        "nvlddmkm: base 0x{:016X}, image {}",
        module.base,
        module.path.display()
    );
    let disk_head = std::fs::read(&module.path).expect("failed to read disk image");
    let fingerprint = PeFingerprint::from_image(&disk_head).expect("disk image not PE32+");
    let drv: &'static PmxDrv = Box::leak(Box::new(
        PmxDrv::connect().expect("PMXDRV connect failed (service running?)"),
    ));
    let pm: &'static PmxDrvPhysMem<'static> = Box::leak(Box::new(PmxDrvPhysMem::new(drv)));
    let phys = CachedPhys::new(pm);
    let discovery = discover_root(&phys, module.base, &fingerprint);
    for event in &discovery.events {
        println!("  {event}");
    }
    let root = discovery.unique_root().expect("page-table root not unique");
    println!("page-table root: 0x{root:016X}");
    (module, phys, root, drv)
}

/// 逐页读一段内核 VA(缺页记洞)。
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

// ---------------- 活体 nvapi 读数 ----------------

#[allow(dead_code)]
struct LiveRails {
    name: String,
    status_values: Vec<i32>,
    control_values: Vec<i32>,
    control_rail_bit: u32,
    boost: Option<i32>,
}

fn read_live(gpu: &PhysicalGpu) -> LiveRails {
    let name = gpu.full_name().unwrap_or_default();
    let rails = gpu.volt_rails().expect("volt_rails");
    let status = *rails.status.first().expect("no volt-rail status entry");
    let control = *rails.control.first().expect("no volt-rail control entry");
    let boost = gpu.core_voltage_boost().ok().map(|p| p.0 as i32);
    println!(
        "live [{name}]: status{:?} control(rail{}){:?} boost={boost:?}",
        status.values, control.rail_bit, control.values
    );
    LiveRails {
        name,
        status_values: status.values.to_vec(),
        control_values: control.values.to_vec(),
        control_rail_bit: control.rail_bit,
        boost,
    }
}

// ---------------- 节表/扫描 ----------------

/// 活体 PE 头动态枚举可写节(IMAGE_SCN_MEM_WRITE=0x80000000;无跨代硬编码)。
fn writable_sections(phys: &dyn PhysicalMemory, root: u64, base: u64) -> Vec<(String, u64, usize)> {
    let head = read_virtual(phys, root, base, 4096).expect("live PE header read");
    let pe = u32::from_le_bytes(head[0x3C..0x40].try_into().unwrap()) as usize;
    assert_eq!(&head[pe..pe + 4], b"PE\0\0", "PE signature");
    let nsec = u16::from_le_bytes(head[pe + 6..pe + 8].try_into().unwrap()) as usize;
    let opt_size = u16::from_le_bytes(head[pe + 20..pe + 22].try_into().unwrap()) as usize;
    let table = pe + 24 + opt_size;
    let mut out = Vec::new();
    for i in 0..nsec {
        let e = table + 40 * i;
        if e + 40 > head.len() {
            break;
        }
        let name = String::from_utf8_lossy(&head[e..e + 8])
            .trim_end_matches('\0')
            .to_string();
        let vsize = u32::from_le_bytes(head[e + 8..e + 12].try_into().unwrap()) as usize;
        let va = u32::from_le_bytes(head[e + 12..e + 16].try_into().unwrap()) as u64;
        let chars = u32::from_le_bytes(head[e + 36..e + 40].try_into().unwrap());
        if chars & 0x8000_0000 != 0 && vsize > 0 && vsize < 32 * 1024 * 1024 {
            println!("  writable section {name}: RVA 0x{va:X} len 0x{vsize:X}");
            out.push((name, base + va, vsize));
        }
    }
    out
}

#[allow(dead_code)]
fn hexdump(buf: &[u8], center: usize, span: usize) -> String {
    let lo = center.saturating_sub(span);
    let hi = (center + span + 4).min(buf.len());
    let mut s = String::new();
    for o in (lo..hi).step_by(4) {
        if o + 4 > buf.len() {
            break;
        }
        let v = u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
        s.push_str(&format!("{o:04x}:{v:08x} "));
    }
    s
}

fn scan_phase() {
    nvapi::initialize().expect("nvapi init");
    let mut gpus = nvoc_gpu_list();
    let gpu = gpus.pop().expect("no gpu");
    let live = read_live(&gpu);
    let (module, phys, root, drv) = connect_walk();

    // 扫描值集:双稀有值元组锚 {vrm, minhold} + 单值锚(vbios 墙/boost)
    let vrm = live.status_values[3] as u32;
    let minhold = live.status_values[5] as u32;
    let vbios = live.status_values[2] as u32;
    let target = live.status_values[1] as u32;
    let boost = live.boost.unwrap_or(0).max(0) as u32;
    println!(
        "scan anchors: vrm={vrm} minhold={minhold} vbios={vbios} target={target} boost={boost}"
    );

    let sections = writable_sections(&phys, root, module.base);
    let mut candidates: Vec<(u64, Vec<u8>)> = Vec::new(); // (page_va, page bytes)
    let mut holes = Vec::new();
    let mut pages_scanned = 0usize;
    for (name, va, len) in &sections {
        for page_va in (0..*len as u64).step_by(4096).map(|o| va + o) {
            let page = read_range(&phys, root, page_va, 4096, &mut holes);
            pages_scanned += 1;
            let dwords: Vec<u32> = page
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                .collect();
            let has_vrm = dwords.contains(&vrm);
            let has_mh = dwords.contains(&minhold);
            if has_vrm && has_mh {
                println!(
                    "CANDIDATE {name}+0x{:X} (va 0x{page_va:016X}): vrm+minhold co-located",
                    page_va & 0xFFF
                );
                candidates.push((page_va, page));
            }
        }
    }
    println!(
        "scan: {pages_scanned} pages, {} holes, {} candidate pages",
        holes.len(),
        candidates.len()
    );

    // 可选:命中页内核指针一跳(NVOC_VOLT_KMD_HOP=1),捕「镜像对象→池对象」链
    if env::var("NVOC_VOLT_KMD_HOP").is_ok() {
        let mut hop_hits: Vec<(u64, Vec<u8>)> = Vec::new();
        for (page_va, page) in &candidates {
            let mut hops = 0usize;
            for o in (0..page.len() - 7).step_by(8) {
                let p = u64::from_le_bytes(page[o..o + 8].try_into().unwrap());
                if p < 0xFFFF_8000_0000_0000 || hops >= 16 {
                    continue;
                }
                hops += 1;
                let tp = read_range(&phys, root, p & !0xFFF, 4096, &mut holes);
                let td: Vec<u32> = tp
                    .chunks_exact(4)
                    .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                    .collect();
                if td.contains(&vrm) && td.contains(&minhold) {
                    println!(
                        "HOP-HIT page 0x{page_va:016X} +0x{o:X} -> 0x{:016X}",
                        p & !0xFFF
                    );
                    hop_hits.push((p & !0xFFF, tp));
                }
            }
        }
        candidates.extend(hop_hits);
    }

    // boost 扰动:原值 → 反向(0 或 100)→ diff → 还原
    let orig_boost = live.boost.unwrap_or(0).max(0);
    let perturb_boost: u32 = if orig_boost == 0 { 100 } else { 0 };
    println!("perturb: boost {orig_boost} -> {perturb_boost}");
    gpu.set_core_voltage_boost(nvapi::Percentage(perturb_boost))
        .expect("boost perturb write");
    std::thread::sleep(std::time::Duration::from_millis(300));
    let mut report =
        json!({"live": {"name": live.name, "status": live.status_values, "boost": orig_boost}});
    let mut moved_report = Vec::new();
    for (page_va, before) in &candidates {
        let after = read_range(&phys, root, *page_va, 4096, &mut holes);
        let mut diffs = Vec::new();
        for (o, (a, b)) in before
            .chunks_exact(4)
            .zip(after.chunks_exact(4))
            .enumerate()
            .map(|(i, (a, b))| (i * 4, (a, b)))
        {
            let va = u32::from_le_bytes(a.try_into().unwrap());
            let vb = u32::from_le_bytes(b.try_into().unwrap());
            if va != vb {
                diffs.push((o, va, vb));
            }
        }
        if !diffs.is_empty() {
            println!(
                "MOVED page 0x{page_va:016X}: {} dwords changed",
                diffs.len()
            );
            for (o, a, b) in &diffs {
                println!("    +{o:04x}: {a} -> {b}");
            }
            moved_report.push(json!({"page_va": format!("0x{page_va:016X}"),
                "diffs": diffs.iter().map(|(o, a, b)| json!({"off": o, "old": a, "new": b})).collect::<Vec<_>>()}));
        } else {
            println!("STATIC page 0x{page_va:016X}: unchanged by boost");
        }
    }
    gpu.set_core_voltage_boost(nvapi::Percentage(orig_boost as u32))
        .expect("boost restore");
    std::thread::sleep(std::time::Duration::from_millis(300));
    let rails_after = gpu.volt_rails().expect("volt_rails after restore");
    println!(
        "boost restored → status {:?} (expect target {} back)",
        rails_after.status.first().map(|s| s.values),
        target
    );
    report["candidates"] = json!(
        candidates
            .iter()
            .map(|(va, _)| format!("0x{va:016X}"))
            .collect::<Vec<_>>()
    );
    report["moved"] = json!(moved_report);
    report["holes"] = json!(holes.len());
    std::fs::write(
        "kmd-volt-policy-scan.json",
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .ok();
    println!("report -> kmd-volt-policy-scan.json");
    let _ = drv;
}

/// write 阶段:env 四件套门控的单 u32 补丁 + nvapi 随动读回 + 自动还原。
fn write_phase() {
    nvapi::initialize().expect("nvapi init");
    let mut gpus = nvoc_gpu_list();
    let gpu = gpus.pop().expect("no gpu");
    let va = u64::from_str_radix(
        env::var("NVOC_VOLT_WRITE_VA")
            .expect("needs NVOC_VOLT_WRITE_VA")
            .trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let old: u32 = env::var("NVOC_VOLT_WRITE_OLD")
        .expect("needs NVOC_VOLT_WRITE_OLD")
        .parse()
        .unwrap();
    let new: u32 = env::var("NVOC_VOLT_WRITE_NEW")
        .expect("needs NVOC_VOLT_WRITE_NEW")
        .parse()
        .unwrap();
    if env::var("NVOC_VOLT_WRITE_ARM").as_deref() != Ok("YES") {
        panic!("write arm not armed (NVOC_VOLT_WRITE_ARM=YES)");
    }
    let (_module, phys, root, drv) = connect_walk();
    let pa = translate(&phys, root, va).expect("VA translation failed");
    let frame = pa & !0xFFF;
    let off = (va & 0xFFF) as usize;
    println!("write 0x{va:016X} frame 0x{frame:X} +0x{off:X}: {old} -> {new}");
    let before = read_range(&phys, root, va & !0xFFF, 4096, &mut Vec::new());
    let cur = u32::from_le_bytes(before[off..off + 4].try_into().unwrap());
    if cur != old {
        panic!(
            "identity gate failed: current value {cur} != expected {old} — object drifted, redo scan"
        );
    }
    let mapped = drv.map_physical(frame, 1).expect("frame map failed");
    unsafe {
        ((mapped + off as u64) as *mut u32).write_volatile(new);
    }
    let w1 = unsafe { ((mapped + off as u64) as *const u32).read_volatile() };
    drv.unmap_physical(mapped).expect("unmap");
    let after = read_range(&phys, root, va & !0xFFF, 4096, &mut Vec::new());
    let w3 = u32::from_le_bytes(after[off..off + 4].try_into().unwrap());
    println!("readback: same-window={w1} pagetable={w3} (expect {new})");
    // nvapi 随动验证:status 六值 vs 写前
    let rails = gpu.volt_rails().expect("volt_rails after write");
    println!(
        "nvapi status after write: {:?}",
        rails.status.first().map(|s| s.values)
    );
    // 还原
    let mapped2 = drv
        .map_physical(frame, 1)
        .expect("frame map failed (restore)");
    unsafe {
        ((mapped2 + off as u64) as *mut u32).write_volatile(old);
    }
    drv.unmap_physical(mapped2).expect("unmap (restore)");
    let restored = read_range(&phys, root, va & !0xFFF, 4096, &mut Vec::new());
    let w4 = u32::from_le_bytes(restored[off..off + 4].try_into().unwrap());
    println!("restored = {w4} (expect {old})");
    if w4 != old {
        panic!("RESTORE FAILED — value {w4}, expected {old}; patch manually");
    }
}

fn nvoc_gpu_list() -> Vec<PhysicalGpu> {
    let mut gpus = PhysicalGpu::enumerate().expect("enumerate");
    // 本机 2070 是唯一 dGPU;若枚举多卡,取第一块有电压轨的
    std::mem::take(&mut gpus)
}

fn rd_u64(phys: &dyn PhysicalMemory, root: u64, va: u64) -> Option<u64> {
    let b = read_virtual(phys, root, va, 8).ok()?;
    Some(u64::from_le_bytes(b[0..8].try_into().unwrap()))
}

fn rd_u32(phys: &dyn PhysicalMemory, root: u64, va: u64) -> Option<u32> {
    let b = read_virtual(phys, root, va, 4).ok()?;
    Some(u32::from_le_bytes(b[0..4].try_into().unwrap()))
}

/// major 阶段:layout_probe(621 磁盘镜像) → state→表→Major 链 → 每个
/// Major 作锚跑 board.rs 全量定位扫查(root 页 + 指针一跳 + 池邻域;电压
/// 元组)→ boost 扰动差分分类候选页。
fn major_phase() {
    nvapi::initialize().expect("nvapi init");
    let mut gpus = nvoc_gpu_list();
    let gpu = gpus.pop().expect("no gpu");
    let live = read_live(&gpu);
    let target = live.status_values[1] as u32;
    let vbios = live.status_values[2] as u32;
    let vrm = live.status_values[3] as u32;
    let minhold = live.status_values[5] as u32;

    let (module, phys, root, _drv) = connect_walk();
    let img = std::fs::read(&module.path).expect("disk image read");
    let layout = probe_layout(&img).expect("layout probe failed on 621 (fail-closed)");
    println!(
        "layout: slot={:#x} state+{:#x} count=+{:#x} Major=+{:#x} stride={:#x} M→root={:#x}",
        layout.global_slot_rva,
        layout.state_table_off,
        layout.table_count_off,
        layout.entry_major_off,
        layout.entry_stride,
        layout.major_root_off
    );

    let state = rd_u64(&phys, root, module.base + layout.global_slot_rva)
        .filter(|v| is_kernel_pointer(*v))
        .expect("state slot invalid");
    let table = rd_u64(&phys, root, state + u64::from(layout.state_table_off))
        .filter(|v| is_kernel_pointer(*v))
        .expect("table ptr invalid");
    let count = rd_u32(&phys, root, table + u64::from(layout.table_count_off)).unwrap_or(0);
    println!("state 0x{state:016X} table 0x{table:016X} count={count}");
    let count = count.clamp(1, 8);

    let volt_values = BoardLiveValues {
        current_mw: target,
        default_mw: vbios,
        max_mw: vrm,
        min_mw: Some(minhold),
    };
    println!(
        "volt tuple targets: current/target={target} default/vbios={vbios} max/vrm={vrm} min/minhold={minhold}"
    );

    let mut all_hits: std::collections::BTreeMap<u64, Vec<(u32, Vec<usize>)>> = Default::default();
    for i in 0..count {
        let Some(major) = rd_u64(
            &phys,
            root,
            table
                + u64::from(layout.entry_major_off)
                + u64::from(layout.entry_stride) * u64::from(i),
        )
        .filter(|v| is_kernel_pointer(*v)) else {
            println!("  entry {i}: major invalid");
            continue;
        };
        println!("  entry {i}: major 0x{major:016X}");
        let scan = locate_board_window_candidates(&phys, root, major, &volt_values);
        for note in &scan.notes {
            println!("    note: {note}");
        }
        println!(
            "    scanned={} unreadable={} candidates={}",
            scan.pages_scanned,
            scan.pages_unreadable,
            scan.candidates.len()
        );
        for c in &scan.candidates {
            println!(
                "    HIT page 0x{:016X} max_off=0x{:X} cur={:?} def_off=0x{:X} min={:?} span=0x{:X}",
                c.page_va, c.hit.max_off, c.hit.cur_off, c.hit.def_off, c.hit.min_off, c.hit.span
            );
            all_hits.insert(c.page_va, Vec::new());
        }
    }

    if all_hits.is_empty() {
        println!(
            "NO tuple hits from any Major — policy object unreachable from Major ±1 hop; extend anchors next"
        );
        return;
    }

    // 扰动前快照(候选页字节)
    let mut holes = Vec::new();
    let mut pre: Vec<(u64, Vec<u8>)> = Vec::new();
    for page_va in all_hits.keys() {
        let page = read_range(&phys, root, *page_va, 4096, &mut holes);
        pre.push((*page_va, page));
    }

    // boost 扰动差分(候选页)
    let orig_boost = live.boost.unwrap_or(0).max(0);
    let perturb_boost: u32 = if orig_boost == 0 { 100 } else { 0 };
    println!("perturb: boost {orig_boost} -> {perturb_boost}");
    gpu.set_core_voltage_boost(nvapi::Percentage(perturb_boost))
        .expect("boost perturb");
    std::thread::sleep(std::time::Duration::from_millis(300));
    let mut moved = Vec::new();
    for (page_va, before) in &pre {
        let after = read_range(&phys, root, *page_va, 4096, &mut holes);
        let mut diffs = Vec::new();
        for (o, (a, b)) in before
            .chunks_exact(4)
            .zip(after.chunks_exact(4))
            .enumerate()
            .map(|(i, (a, b))| (i * 4, (a, b)))
        {
            let va = u32::from_le_bytes(a.try_into().unwrap());
            let vb = u32::from_le_bytes(b.try_into().unwrap());
            if va != vb {
                diffs.push((o, va, vb));
            }
        }
        if diffs.is_empty() {
            println!("STATIC page 0x{page_va:016X}");
            continue;
        }
        println!("MOVED page 0x{page_va:016X}: {} dwords", diffs.len());
        for (o, a, b) in &diffs {
            println!("    +{o:04x}: {a} -> {b}");
        }
        moved.push(json!({"page_va": format!("0x{page_va:016X}"),
            "diffs": diffs.iter().map(|(o, a, b)| json!({"off": o, "old": a, "new": b})).collect::<Vec<_>>()}));
    }
    gpu.set_core_voltage_boost(nvapi::Percentage(orig_boost as u32))
        .expect("boost restore");
    std::thread::sleep(std::time::Duration::from_millis(300));
    let report = json!({"live": {"name": live.name, "status": live.status_values, "boost": orig_boost},
        "hits": all_hits.iter().map(|(va, vals)| json!({"page_va": format!("0x{va:016X}"),
            "values": vals.iter().map(|(v, offs)| json!({"value": v, "offs": offs})).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        "moved": moved});
    std::fs::write(
        "kmd-volt-policy-major.json",
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .ok();
    println!("report -> kmd-volt-policy-major.json");
}

/// diff2 阶段:Turing 上 payload 写是活车道(2070 实证)→ 全镜像 before/after
/// diff 直接定位内核墙存储。两连击:①slot2(VRM 墙偏移 +6250µV)→ vrm/
/// effective 相关字段现形;②boost 0→100 → boost/target 字段现形。全部扰动
/// 即改即还原 + nvapi/kernel 双侧终验。
fn diff2_phase() {
    nvapi::initialize().expect("nvapi init");
    let mut gpus = nvoc_gpu_list();
    let gpu = gpus.pop().expect("no gpu");
    let live = read_live(&gpu);
    let vrm = live.status_values[3] as u32;
    let target = live.status_values[1] as u32;
    let step: i32 = gpu
        .volt_devices()
        .ok()
        .and_then(|d| d.first().map(|d| d.step_uV))
        .filter(|s| *s > 0)
        .unwrap_or(6250) as i32;

    let (module, phys, root, _drv) = connect_walk();

    // A: 621 RE 链直探:v4 = *(Major+0x2440),v5 = *(v4+0x6C0)(idalib 定案)
    let img = std::fs::read(&module.path).expect("disk image read");
    let layout = probe_layout(&img).expect("layout probe failed");
    let state = rd_u64(&phys, root, module.base + layout.global_slot_rva)
        .filter(|v| is_kernel_pointer(*v))
        .expect("state slot invalid");
    let table = rd_u64(&phys, root, state + u64::from(layout.state_table_off))
        .filter(|v| is_kernel_pointer(*v))
        .expect("table ptr invalid");
    let count = rd_u32(&phys, root, table + u64::from(layout.table_count_off))
        .unwrap_or(0)
        .clamp(1, 8);
    for i in 0..count {
        let Some(major) = rd_u64(
            &phys,
            root,
            table
                + u64::from(layout.entry_major_off)
                + u64::from(layout.entry_stride) * u64::from(i),
        )
        .filter(|v| is_kernel_pointer(*v)) else {
            continue;
        };
        let v4 = rd_u64(&phys, root, major + 0x2440).filter(|v| is_kernel_pointer(*v));
        let Some(v4) = v4 else {
            println!("chain probe entry {i}: *(Major+0x2440) not a kernel pointer");
            continue;
        };
        let v5 = rd_u64(&phys, root, v4 + 0x6C0).filter(|v| is_kernel_pointer(*v));
        let Some(v5) = v5 else {
            println!("chain probe entry {i}: v4 0x{v4:016X}, *(v4+0x6C0) not a kernel pointer");
            continue;
        };
        println!("chain probe entry {i}: Major 0x{major:016X} → v4 0x{v4:016X} → v5 0x{v5:016X}");
        let mut holes = Vec::new();
        for k in -4i64..=4 {
            let pg = (v5 as i64 + k * 0x1000) as u64;
            let page = read_range(&phys, root, pg, 4096, &mut holes);
            let mut found = Vec::new();
            for &label in &["target", "vbios", "vrm", "minhold"] {
                let v = match label {
                    "target" => target,
                    "vbios" => live.status_values[2] as u32,
                    "vrm" => vrm,
                    _ => live.status_values[5] as u32,
                };
                let offs: Vec<usize> = page
                    .chunks_exact(4)
                    .enumerate()
                    .filter(|(_, c)| u32::from_le_bytes((*c).try_into().unwrap()) == v)
                    .map(|(i, _)| i * 4)
                    .collect();
                if !offs.is_empty() {
                    found.push(format!(
                        "{label}({v})@{:?}",
                        offs.iter().map(|x| format!("0x{x:X}")).collect::<Vec<_>>()
                    ));
                }
            }
            if !found.is_empty() {
                println!(
                    "    v5{}0x{:X} page 0x{pg:016X}: {}",
                    if k >= 0 { "+" } else { "-" },
                    k.abs() * 4,
                    found.join(" ")
                );
            }
        }
    }

    // B: 全镜像 diff(slot2 扰动)
    let sections = writable_sections(&phys, root, module.base);
    let mut holes = Vec::new();
    let mut snap: Vec<(u64, Vec<u8>)> = Vec::new();
    for (name, va, len) in &sections {
        println!("snapshot {name} (0x{len:X})…");
        for page_va in (0..*len as u64).step_by(4096).map(|o| va + o) {
            let page = read_range(&phys, root, page_va, 4096, &mut holes);
            snap.push((page_va, page));
        }
    }
    println!("snapshot done: {} pages", snap.len());

    let perturb = |gpu: &PhysicalGpu, rail: u32, slot: usize, delta: i32| {
        let cur = gpu
            .volt_rails()
            .expect("rails")
            .control
            .first()
            .unwrap()
            .values[slot];
        let want = cur.wrapping_add(delta);
        match gpu.set_volt_rail_slot(rail, slot, want) {
            Ok(retained) => println!(
                "perturb slot{slot}: {} -> {} (retained {retained})",
                cur, want
            ),
            Err(e) => println!("perturb slot{slot} REFUSED: {e}"),
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };

    println!("=== slot2 (VRM wall offset +step) diff ===");
    perturb(&gpu, 0, 2, step);
    let rails_now = gpu.volt_rails().expect("rails");
    println!(
        "nvapi status now: {:?}",
        rails_now.status.first().map(|s| s.values)
    );
    let mut hits2 = Vec::new();
    for (page_va, before) in &snap {
        let after = read_range(&phys, root, *page_va, 4096, &mut holes);
        for (o, (a, b)) in before
            .chunks_exact(4)
            .zip(after.chunks_exact(4))
            .enumerate()
            .map(|(i, (a, b))| (i * 4, (a, b)))
        {
            let va = u32::from_le_bytes(a.try_into().unwrap());
            let vb = u32::from_le_bytes(b.try_into().unwrap());
            if va != vb {
                println!("    CHANGED 0x{page_va:016X}+0x{o:03X}: {va} -> {vb}");
                hits2.push((format!("0x{page_va:016X}+0x{o:X}"), va, vb));
            }
        }
    }
    // 还原 slot2
    let cur = gpu
        .volt_rails()
        .expect("rails")
        .control
        .first()
        .unwrap()
        .values[2];
    let _ = gpu.set_volt_rail_slot(0, 2, cur.wrapping_sub(step));
    std::thread::sleep(std::time::Duration::from_millis(200));
    println!(
        "slot2 restored, nvapi status: {:?}",
        gpu.volt_rails()
            .expect("rails")
            .status
            .first()
            .map(|s| s.values)
    );

    println!("=== boost 0->100 diff ===");
    gpu.set_core_voltage_boost(nvapi::Percentage(100))
        .expect("boost 100");
    std::thread::sleep(std::time::Duration::from_millis(300));
    for (page_va, before) in &snap {
        let after = read_range(&phys, root, *page_va, 4096, &mut holes);
        for (o, (a, b)) in before
            .chunks_exact(4)
            .zip(after.chunks_exact(4))
            .enumerate()
            .map(|(i, (a, b))| (i * 4, (a, b)))
        {
            let va = u32::from_le_bytes(a.try_into().unwrap());
            let vb = u32::from_le_bytes(b.try_into().unwrap());
            if va != vb {
                println!("    CHANGED 0x{page_va:016X}+0x{o:03X}: {va} -> {vb}");
                hits2.push((format!("boost 0x{page_va:016X}+0x{o:X}"), va, vb));
            }
        }
    }
    gpu.set_core_voltage_boost(nvapi::Percentage(0))
        .expect("boost restore");
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!(
        "boost restored, nvapi status: {:?}",
        gpu.volt_rails()
            .expect("rails")
            .status
            .first()
            .map(|s| s.values)
    );

    let report = json!({"hits": hits2.iter().map(|(va, a, b)| json!({"va": va, "old": a, "new": b})).collect::<Vec<_>>()});
    std::fs::write(
        "kmd-volt-policy-diff2.json",
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .ok();
    println!(
        "report -> kmd-volt-policy-diff2.json ({} changed sites)",
        hits2.len()
    );
}

/// ramscan 阶段:diff2 判定策略状态不在镜像节(池内存)→ 物理全 RAM 扫描。
/// slot2 扰动出活诱饵(vrm+step,稀有值)→ 白名单 RAM 分块映射扫 needle →
/// 同页 minhold 共现过滤 → 恢复 slot2 → 复扫确认字段跟随(活体字段实锤)
/// → 报告物理帧 + 全锚值偏移。
fn ramscan_phase() {
    nvapi::initialize().expect("nvapi init");
    let mut gpus = nvoc_gpu_list();
    let gpu = gpus.pop().expect("no gpu");
    let live = read_live(&gpu);
    let step: i32 = gpu
        .volt_devices()
        .ok()
        .and_then(|d| d.first().map(|d| d.step_uV))
        .filter(|s| *s > 0)
        .unwrap_or(6250) as i32;
    let vrm = live.status_values[3] as u32;
    let minhold = live.status_values[5] as u32;
    let needle = vrm.wrapping_add(step as u32);
    println!("perturb needle: vrm {vrm} -> {needle} (minhold co-anchor {minhold})");

    let (module, _phys, root, drv) = connect_walk();
    let _ = (module, root);

    // 扰动 slot2
    let cur_slot2 = gpu
        .volt_rails()
        .expect("rails")
        .control
        .first()
        .unwrap()
        .values[2];
    let st = gpu
        .set_volt_rail_slot(0, 2, cur_slot2.wrapping_add(step))
        .expect("slot2 perturb refused");
    println!("slot2 perturbed (retained {st})");
    let st_now = gpu
        .volt_rails()
        .expect("rails")
        .status
        .first()
        .unwrap()
        .values[3];
    assert_eq!(
        st_now as u32, needle,
        "nvapi vrm wall did not follow perturbation"
    );
    println!("nvapi vrm wall now {st_now} — needle live");

    let ranges = ram_ranges();
    let total: u64 = ranges.iter().map(|(s, e)| e - s).sum();
    println!(
        "RAM whitelist: {} segments, {:.2} GiB total",
        ranges.len(),
        total as f64 / (1 << 30) as f64
    );
    assert!(
        !ranges.is_empty(),
        "no usable RAM ranges — abort (blind scan = BSOD risk)"
    );

    const CHUNK_PAGES: u32 = 512; // 2 MiB
    let chunk_bytes = CHUNK_PAGES as u64 * 4096;
    let mut frames: Vec<(u64, usize)> = Vec::new(); // (frame pa, page offset of needle)
    let mut map_failures = 0usize;
    let _consecutive_fail = 0usize;
    let mut scanned_bytes = 0u64;
    for (seg_start, seg_end) in &ranges {
        let mut off = 0u64;
        let seg_len = seg_end - seg_start;
        let mut seg_fail = 0usize;
        while off + chunk_bytes <= seg_len {
            let pa = seg_start + off;
            match drv.map_physical(pa, CHUNK_PAGES) {
                Ok(va) if va != 0 && va != 0xBEEF => {
                    seg_fail = 0;
                    let slice = unsafe {
                        std::slice::from_raw_parts(va as *const u8, chunk_bytes as usize)
                    };
                    for (i, w) in slice.chunks_exact(4).enumerate() {
                        if w == needle.to_le_bytes().as_slice() {
                            let frame = pa + (i * 4 / 4096 * 4096) as u64;
                            let off_in_page = (i * 4) % 4096;
                            println!("NEEDLE-HIT pa 0x{frame:X}+0x{off_in_page:X}");
                            frames.push((frame, off_in_page));
                        }
                    }
                    scanned_bytes += chunk_bytes;
                    drv.unmap_physical(va).ok();
                }
                Ok(_) | Err(_) => {
                    map_failures += 1;
                    seg_fail += 1;
                    if seg_fail >= 32 {
                        println!(
                            "  segment 0x{seg_start:X}: 32 consecutive chunk failures, skipping rest"
                        );
                        break;
                    }
                }
            }
            off += chunk_bytes;
        }
    }
    println!(
        "scan done: {:.2} GiB scanned, {} needle pages, {} chunk map failures",
        scanned_bytes as f64 / (1 << 30) as f64,
        frames.len(),
        map_failures
    );

    // 同页 minhold 共现过滤 + 全锚值偏移
    #[allow(clippy::type_complexity)]
    let mut strong: Vec<(u64, Vec<(String, u32, Vec<usize>)>)> = Vec::new();
    for (frame, _) in &frames {
        let Ok(va) = drv.map_physical(*frame, 1) else {
            continue;
        };
        if va == 0 || va == 0xBEEF {
            continue;
        }
        let page = unsafe { std::slice::from_raw_parts(va as *const u8, 4096) };
        let dwords: Vec<u32> = page
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        drv.unmap_physical(va).ok();
        let find = |v: u32| -> Vec<usize> {
            dwords
                .iter()
                .enumerate()
                .filter(|(_, d)| **d == v)
                .map(|(i, _)| i * 4)
                .collect()
        };
        let mh = find(minhold);
        if mh.is_empty() {
            continue;
        }
        let fields = vec![
            ("vrm_perturbed".into(), needle, find(needle)),
            ("minhold".into(), minhold, mh),
            (
                "target".into(),
                live.status_values[1] as u32,
                find(live.status_values[1] as u32),
            ),
            (
                "vbios".into(),
                live.status_values[2] as u32,
                find(live.status_values[2] as u32),
            ),
            (
                "minhold6".into(),
                live.status_values[5] as u32,
                find(live.status_values[5] as u32),
            ),
        ];
        println!(
            "STRONG pa 0x{frame:X}: {}",
            fields
                .iter()
                .map(|(l, v, o)| format!(
                    "{l}({v})@{:?}",
                    o.iter().map(|x| format!("0x{x:X}")).collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>()
                .join("  ")
        );
        strong.push((*frame, fields));
    }

    // 恢复 slot2 → 复扫确认字段跟随
    let _ = gpu.set_volt_rail_slot(0, 2, cur_slot2);
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!(
        "restored, nvapi status: {:?}",
        gpu.volt_rails()
            .expect("rails")
            .status
            .first()
            .map(|s| s.values)
    );
    for (frame, _) in &strong {
        if let Ok(va) = drv.map_physical(*frame, 1)
            && va != 0
            && va != 0xBEEF
        {
            let page = unsafe { std::slice::from_raw_parts(va as *const u8, 4096) };
            let still = page
                .chunks_exact(4)
                .filter(|c| *c == needle.to_le_bytes().as_slice())
                .count();
            let back = page
                .chunks_exact(4)
                .filter(|c| *c == vrm.to_le_bytes().as_slice())
                .count();
            println!(
                "post-restore pa 0x{frame:X}: perturbed-value copies={still}, vrm-value copies={back}"
            );
            drv.unmap_physical(va).ok();
        }
    }
    let report = json!({"needle": needle, "frames": frames.iter().map(|(f, o)| json!({"pa": format!("0x{f:X}"), "off": o})).collect::<Vec<_>>(),
        "strong": strong.iter().map(|(f, fields)| json!({"pa": format!("0x{f:X}"),
            "fields": fields.iter().map(|(l, v, o)| json!({"label": l, "value": v, "offs": o})).collect::<Vec<_>>()})).collect::<Vec<_>>()});
    std::fs::write(
        "kmd-volt-ramscan.json",
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .ok();
    println!("report -> kmd-volt-ramscan.json");
}

#[test]
#[ignore = "requires PMXDRV service + elevated shell; phase via NVOC_VOLT_KMD_PHASE"]
fn kmd_volt_policy_diff() {
    let phase = env::var("NVOC_VOLT_KMD_PHASE").unwrap_or_else(|_| "scan".into());
    match phase.as_str() {
        "scan" => scan_phase(),
        "major" => major_phase(),
        "diff2" => diff2_phase(),
        "ramscan" => ramscan_phase(),
        "write" => write_phase(),
        other => panic!("unknown phase {other} (scan|major|diff2|ramscan|write)"),
    }
}
