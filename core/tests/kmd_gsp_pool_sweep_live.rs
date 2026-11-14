//! GSP 模式内核池值扫查(只读决定性探针 v2,2026-10-29)。
//!
//! 背景:GSP 固件模式下 Major→PowerRoot 链接读零,且 Major/state 双锚域
//! (kmd_locate_trace_live)值共现全空 —— VA 可达域找不到活体三元组。本工具
//! 扫**内核池**找功率值签名:GSP heap、驱动大表、section 背板若在 host 侧,
//! 首选居所就是池。v1(物理全域扫)废弃:HKLM\HARDWARE\RESOURCEMAP 在现代
//! Windows 上不是完整 RAM 图(本机 62.9GB 只列出 ~1GB 残留,无 4GB 以上
//! 区段),盲扫物理地址失去安全边界。
//!
//! 流程:
//! 1. `NtQuerySystemInformation(SystemBigPoolInformation=0x42)` 枚举全部
//!    ≥1 页池块(VA/大小/4 字符池标签;VA bit0 = NonPaged 标志);
//! 2. 池标签直方图:按字节总量排序打 top 40 —— GSP heap 分配的标签/大小
//!    本身是下一步逆向的入料;≥16MB 的巨块单独点名;
//! 3. 逐块扫查:块内每页走查 VA→PA(pmxdrv + CR3,本文件自带 PTE 页缓存,
//!    否则每页 4 次 map/unmap 撑不住 GB 级)→ map_physical 单页扫 mW/µW
//!    活体值(目标取 NVAPI 活体窗,机器无关;判据与 board.rs 共现同构:
//!    一页 ≥2 种互异值),命中记帧号+偏移,首命中 hexdump;
//! 4. 预算:按块大小降序扫到 NVOC_SWEEP_MAX_POOL_GB(默认 8)为止;
//!    VERDICT 按覆盖面如实措辞(池 ≥1 页块全扫 ≠ 全 RAM:MDL 分配的
//!    MmAllocatePagesForMdl 系不在 big-pool 枚举内,池外命中要另行立项)。
//!
//! 运行(elevated,建议 --release):
//! ```text
//! cargo test --release -p nvoc-core --test kmd_gsp_pool_sweep_live -- --ignored --nocapture
//! ```
#![cfg(windows)]

use std::collections::HashMap;
use std::env;

use nvoc_core::kmd::layout_probe::probe as probe_layout;
use nvoc_core::kmd::pagewalk::{
    PeFingerprint, PhysicalMemory, discover_root, find_loaded_module, read_virtual, translate,
};
use nvoc_core::kmd::pmxdrv::{PmxDrv, PmxDrvPhysMem};

const PAGE: usize = 0x1000;
const SYSTEM_BIG_POOL_INFORMATION: i32 = 0x42;

/// 服务已由外部(sc start PMXDRV_NEW)拉起;本 harness 只连接不托管。
struct PmxDrvServiceGuard;

// ---- SYSTEM_BIGPOOL_INFORMATION 解析(x64:Count@0,pad,条目 24B:
//      VA|flags@0(bit0=NonPaged)、Size@8、Tag@16(+4 pad))

struct PoolBlock {
    va: u64,
    nonpaged: bool,
    size: u64,
    tag: [u8; 4],
}

fn query_big_pool() -> Vec<PoolBlock> {
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQuerySystemInformation(class: i32, info: *mut u8, len: u32, retlen: *mut u32) -> i32;
    }
    let mut len = 0u32;
    let mut buf: Vec<u8> = Vec::new();
    // big-pool 是动态信息类:两次调用之间池一长,首调拿到的尺寸就过时
    // (复现的 0xC0000004 正是出在第二次调用)—— 标准解法是重试环,每次
    // 取 ReturnLength 加 1/8 余量,8 轮内仍涨不停才放弃。
    for attempt in 0..8 {
        let mut ret = 0u32;
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_BIG_POOL_INFORMATION,
                buf.as_mut_ptr().cast(),
                len,
                &mut ret,
            )
        };
        if status == 0 {
            buf.truncate(len as usize);
            break;
        }
        if status == 0xC000_0004u32 as i32 {
            let next = ret.max(len).saturating_add(ret / 8).max(4096);
            if next > len {
                len = next;
            } else {
                len = len.saturating_add(len / 8).max(4096);
            }
            buf.resize(len as usize, 0);
            if attempt == 7 {
                panic!(
                    "NtQuerySystemInformation(big-pool) size kept growing for 8 attempts — system under heavy allocation churn"
                );
            }
            continue;
        }
        panic!("NtQuerySystemInformation(big-pool) failed: status {status:#x}");
    }
    let u32at = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    let u64at = |o: usize| u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
    let count = u32at(0) as usize;
    let mut blocks = Vec::with_capacity(count);
    for i in 0..count {
        let base = 8 + i * 24;
        if base + 24 > buf.len() {
            break;
        }
        let raw_va = u64at(base);
        blocks.push(PoolBlock {
            va: raw_va & !1,
            nonpaged: raw_va & 1 != 0,
            size: u64at(base + 8),
            tag: buf[base + 16..base + 20].try_into().unwrap(),
        });
    }
    blocks
}

fn tag_str(tag: &[u8; 4]) -> String {
    tag.iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
        .collect()
}

// ---- PTE 页缓存物理读(抄 power.rs::CachedPhys 思路:8 字节读全缓存,
//      走查的 PTE 重复访问压到零)

#[allow(dead_code)]
struct CachedPhys<'a> {
    drv: &'a PmxDrv,
    cache: std::cell::RefCell<HashMap<u64, [u8; PAGE]>>,
}

impl<'a> CachedPhys<'a> {
    fn new(drv: &'a PmxDrv) -> Self {
        Self {
            drv,
            cache: std::cell::RefCell::new(HashMap::new()),
        }
    }
}

impl PhysicalMemory for CachedPhys<'_> {
    fn read(&self, pa: u64, out: &mut [u8]) -> Result<(), nvoc_core::kmd::pagewalk::PhysError> {
        use nvoc_core::kmd::pagewalk::PhysError;
        if out.is_empty() || out.len() > PAGE {
            return Err(PhysError::BadLength(out.len()));
        }
        let frame = pa & !0xFFF;
        let off = (pa - frame) as usize;
        if off + out.len() > PAGE {
            return Err(PhysError::BadLength(out.len()));
        }
        let cached = out.len() == 8;
        if cached && let Some(page) = self.cache.borrow().get(&frame) {
            out.copy_from_slice(&page[off..off + 8]);
            return Ok(());
        }
        let mut page = [0u8; PAGE];
        let mapped = self
            .drv
            .map_physical(frame, 1)
            .map_err(|_| PhysError::Unreadable(pa))?;
        let window = unsafe { std::slice::from_raw_parts(mapped as *const u8, PAGE) };
        page.copy_from_slice(window);
        let _ = self.drv.unmap_physical(mapped);
        if cached {
            let mut map = self.cache.borrow_mut();
            if map.len() > 32_768 {
                map.clear();
            }
            map.insert(frame, page);
        }
        out.copy_from_slice(&page[off..off + out.len()]);
        Ok(())
    }
}

/// 一页命中:(帧号, [(值, 值名, 页内偏移组)])。
type PageHit = (u64, Vec<(u32, String, Vec<usize>)>);

fn scan_page(page: &[u8], targets: &[(u32, String)]) -> Vec<(u32, String, Vec<usize>)> {
    let mut found: Vec<(u32, String, Vec<usize>)> = Vec::new();
    for (wi, word) in page.chunks_exact(4).enumerate() {
        let v = u32::from_le_bytes(word.try_into().unwrap());
        for (tv, name) in targets {
            if v == *tv {
                match found.iter_mut().find(|(fv, _, _)| fv == tv) {
                    Some(entry) => entry.2.push(wi * 4),
                    None => found.push((*tv, name.clone(), vec![wi * 4])),
                }
            }
        }
    }
    found
}

/// 活体镜像 PE 头动态枚举可写节(与 kmd_volt_policy_diff_live 同款)。
fn writable_sections_for(
    phys: &dyn PhysicalMemory,
    root: u64,
    base: u64,
) -> Vec<(String, u64, usize)> {
    let head = read_virtual(phys, root, base, 4096).expect("live PE header read");
    let pe = u32::from_le_bytes(head[0x3C..0x40].try_into().unwrap()) as usize;
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
            out.push((name, base + va, vsize));
        }
    }
    out
}

fn va_lookup(pages: &[(u64, u64, Vec<u8>)], frame: u64, off: usize) -> String {
    for (va, f, _) in pages {
        if *f == frame {
            return format!("0x{:#016X}", va + off as u64);
        }
    }
    "n/a".into()
}

#[allow(dead_code)]
struct LiveRails {
    name: String,
    status_values: Vec<i32>,
}

fn read_live(gpu: &nvapi::PhysicalGpu) -> LiveRails {
    let name = gpu.full_name().unwrap_or_default();
    let rails = gpu.volt_rails().expect("volt_rails");
    let status = *rails.status.first().expect("no volt-rail status entry");
    println!("live [{name}]: status{:?}", status.values);
    LiveRails {
        name,
        status_values: status.values.to_vec(),
    }
}

/// ptscan 阶段(NVOC_VOLT_PTSCAN=1):从 CR3 走全部内核页表 → 枚举所有
/// 已映射帧(池外 MDL carveout / direct map 一网打尽)→ slot2 扰动出活诱饵
/// (vrm+step 稀有值)→ 全帧扫 needle → 还原 → 复扫确认跟随(活体字段实锤)
/// → 报告 VA/PA。x64 四级页表手走,大页(2MB/1GB)按块映射扫。
fn ptscan_phase() {
    nvapi::initialize().expect("nvapi init");
    let Some(gpu) = nvapi::PhysicalGpu::enumerate()
        .ok()
        .and_then(|g| g.into_iter().next())
    else {
        panic!("NVAPI GPU unreachable");
    };
    let live = read_live(&gpu);
    let step: i32 = gpu
        .volt_devices()
        .ok()
        .and_then(|d| d.first().map(|d| d.step_uV))
        .filter(|s| *s > 0)
        .unwrap_or(6250) as i32;
    let vrm = live.status_values[3] as u32;
    let needle = vrm.wrapping_add(step as u32);

    let drv = PmxDrv::connect().expect("PMXDRV connect failed");
    let module = find_loaded_module("nvlddmkm.sys").expect("nvlddmkm.sys not loaded");
    let img = std::fs::read(&module.path).expect("disk image read");
    let fingerprint = PeFingerprint::from_image(&img).expect("bad PE");
    let phys = CachedPhys::new(&drv);
    let discovery = discover_root(&phys, module.base, &fingerprint);
    let root = discovery.unique_root().expect("root not unique");
    gpu.set_volt_rail_slot(0, 2, 0).expect("slot2 zeroing");
    std::thread::sleep(std::time::Duration::from_millis(200));
    let cur_slot2 = 0i32;
    let st = gpu
        .set_volt_rail_slot(0, 2, step)
        .expect("slot2 perturb refused");
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!("ptscan: slot2 perturbed (retained {st}), needle = {needle}");
    let offset_val = cur_slot2.wrapping_add(step) as u32; // slot2 偏移槽现值
    let needles = vec![needle, offset_val];
    println!("ptscan needles: wall={needle}, offset={offset_val}");

    const MAX_PHYS: u64 = 0x11_0000_0000;
    let read_page = |frame: u64| -> Option<[u8; 4096]> {
        if frame >= (MAX_PHYS >> 12) {
            return None;
        }
        let mut p = [0u8; 4096];
        phys.read(frame << 12, &mut p).ok()?;
        Some(p)
    };
    let entry = |page: &[u8; 4096], i: usize| -> u64 {
        u64::from_le_bytes(page[i * 8..i * 8 + 8].try_into().unwrap())
    };
    let rd_phys = |frame: u64, off: usize| -> Option<u32> {
        let va = drv.map_physical(frame << 12, 1).ok()?;
        if va == 0 || va == 0xBEEF {
            return None;
        }
        let page = unsafe { std::slice::from_raw_parts(va as *const u8, 4096) };
        let d = u32::from_le_bytes(page[off..off + 4].try_into().unwrap());
        let _ = drv.unmap_physical(va);
        Some(d)
    };

    let mut all_hits: Vec<(u64, usize)> = Vec::new();
    let mut chunks = 0u64;
    {
        let scan_chunk = |va: u64,
                          phys_base: u64,
                          npages: u32,
                          needles: &[u32],
                          total_hits: &mut Vec<(u64, usize)>,
                          chunks: &mut u64| {
            if phys_base == 0 || phys_base >= MAX_PHYS {
                return;
            }
            let Ok(mapped) = drv.map_physical(phys_base, npages) else {
                return;
            };
            if mapped == 0 || mapped == 0xBEEF {
                return;
            }
            let bytes =
                unsafe { std::slice::from_raw_parts(mapped as *const u8, npages as usize * 4096) };
            for (i, w) in bytes.chunks_exact(4).enumerate() {
                for (ni, &nd) in needles.iter().enumerate() {
                    if w == nd.to_le_bytes().as_slice() {
                        let hit_va = va + (i * 4) as u64;
                        eprintln!("NEEDLE[{ni}]-VA 0x{hit_va:016X}");
                        total_hits.push((hit_va, (i * 4) % 4096));
                    }
                }
            }
            let _ = drv.unmap_physical(mapped);
            *chunks += 1;
        };

        let Some(pml4) = read_page(root >> 12) else {
            panic!("PML4 read failed");
        };
        // direct map(PML4[511]=0xFFFF8000...)优先:覆盖全部 RAM(含 MDL carveout)
        let mut order: Vec<usize> = vec![511];
        for i in 256..511usize {
            order.push(i);
        }
        for &i4 in &order {
            let e4 = entry(&pml4, i4);
            if e4 & 1 == 0 {
                continue;
            }
            let Some(pdpt) = read_page((e4 >> 12) & 0xF_FFFF_FFFF) else {
                eprintln!("ptscan dbg: PML4[{i4}] PDPT read failed");
                continue;
            };
            let mut p3 = 0usize;
            for i3 in 0..512usize {
                if entry(&pdpt, i3) & 1 == 1 {
                    p3 += 1;
                }
            }
            eprintln!("ptscan: PML4[{i4}] PDPT present = {p3}");
            if p3 == 0 {
                continue;
            }
            for i3 in 0..512usize {
                let e3 = entry(&pdpt, i3);
                if e3 & 1 == 0 {
                    continue;
                }
                let va3 = 0xFFFF_8000_0000_0000u64 | ((i4 as u64) << 39) | ((i3 as u64) << 30);
                if e3 & (1 << 7) != 0 {
                    let gb_frame = (e3 >> 30) & 0x3F_FFFF;
                    for k in 0..512u64 {
                        scan_chunk(
                            va3 | (k << 21),
                            (gb_frame << 30) + k * 0x20_0000,
                            512,
                            &needles,
                            &mut all_hits,
                            &mut chunks,
                        );
                    }
                    continue;
                }
                let Some(pd) = read_page((e3 >> 12) & 0xF_FFFF_FFFF) else {
                    continue;
                };
                for i2 in 0..512usize {
                    let e2 = entry(&pd, i2);
                    if e2 & 1 == 0 {
                        continue;
                    }
                    let va2 = va3 | ((i2 as u64) << 21);
                    if e2 & (1 << 7) != 0 {
                        let f2 = (e2 >> 21) & 0x7F_FFFF;
                        scan_chunk(va2, f2 << 21, 512, &needles, &mut all_hits, &mut chunks);
                    } else {
                        let Some(pt) = read_page((e2 >> 12) & 0xF_FFFF_FFFF) else {
                            continue;
                        };
                        let mut i1 = 0usize;
                        while i1 < 512usize {
                            let e1 = entry(&pt, i1);
                            if e1 & 1 == 0 {
                                i1 += 1;
                                continue;
                            }
                            let mut run = 1usize;
                            while i1 + run < 512usize && entry(&pt, i1 + run) & 1 == 1 {
                                run += 1;
                            }
                            let f0 = (entry(&pt, i1) >> 12) & 0xF_FFFF_FFFF;
                            let contiguous = (1..run).all(|k| {
                                (entry(&pt, i1 + k) >> 12) & 0xF_FFFF_FFFF == f0 + k as u64
                            });
                            if contiguous {
                                scan_chunk(
                                    va2 | ((i1 as u64) << 12),
                                    f0 << 12,
                                    run as u32,
                                    &needles,
                                    &mut all_hits,
                                    &mut chunks,
                                );
                            } else {
                                for k in 0..run {
                                    let f = (entry(&pt, i1 + k) >> 12) & 0xF_FFFF_FFFF;
                                    scan_chunk(
                                        va2 | ((i1 + k) as u64) << 12,
                                        f << 12,
                                        1,
                                        &needles,
                                        &mut all_hits,
                                        &mut chunks,
                                    );
                                }
                            }
                            i1 += run;
                        }
                    }
                }
            }
            // 区域级即时验证:还原 slot2 → 逐命中复扫(回 vrm = 活体字段)→ 再扰动
            if !all_hits.is_empty() {
                gpu.set_volt_rail_slot(0, 2, 0)
                    .expect("region verify reset");
                std::thread::sleep(std::time::Duration::from_millis(250));
                let mut live_sites: Vec<(u64, usize)> = Vec::new();
                for (va, off) in &all_hits {
                    if let Ok(pa) = translate(&phys, root, *va) {
                        if let Some(d) = rd_phys(pa >> 12, (*va & 0xFFF) as usize) {
                            let tag = if d == 0 {
                                live_sites.push((*va, *off));
                                "LIVE (reverted — execution-layer offset field)"
                            } else if d == needle {
                                "stale/history (wall)"
                            } else if d == step as u32 {
                                "stale/history (offset)"
                            } else {
                                "other"
                            };
                            eprintln!("  verify 0x{va:016X} = {d} — {tag}");
                        }
                    } else {
                        eprintln!("  verify 0x{va:016X} = <untranslatable after reset>");
                    }
                }
                eprintln!(
                    "ptscan: PML4[{i4}] region done — {}/{} live offset fields so far (chunks {chunks})",
                    live_sites.len(),
                    all_hits.len()
                );
                // poke 臂:对本次区域发现的活体偏移字段,全部同步 patch 到
                // +100000 µV(+100 mV)→ nvapi status 随动验证 → 还原 0
                if !live_sites.is_empty() {
                    // patch 全部 → 100000
                    for &(va, _off) in &live_sites {
                        if let Ok(pa) = translate(&phys, root, va) {
                            let va_m = drv.map_physical(pa & !0xFFF, 1).expect("poke map");
                            unsafe {
                                ((va_m + (va & 0xFFF)) as *mut u32).write_volatile(100000u32);
                            }
                            let _ = drv.unmap_physical(va_m);
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let st_p = gpu
                        .volt_rails()
                        .expect("rails")
                        .status
                        .first()
                        .unwrap()
                        .values;
                    let raised = st_p[3] as i64 == vrm as i64 + 100000;
                    eprintln!(
                        "POKE: all live sites -> 100000 µV; nvapi vrm wall = {} (expect {}) — {}",
                        st_p[3],
                        vrm + 100000,
                        if raised {
                            "LANE CONFIRMED"
                        } else {
                            "no follow — copies insufficient or wrong field"
                        }
                    );
                    eprintln!("POKE full status: {st_p:?}");
                    // 还原 0
                    for &(va, _off) in &live_sites {
                        if let Ok(pa) = translate(&phys, root, va) {
                            let va_m = drv.map_physical(pa & !0xFFF, 1).expect("restore map");
                            unsafe {
                                ((va_m + (va & 0xFFF)) as *mut u32).write_volatile(0u32);
                            }
                            let _ = drv.unmap_physical(va_m);
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let st_r = gpu
                        .volt_rails()
                        .expect("rails")
                        .status
                        .first()
                        .unwrap()
                        .values[3];
                    eprintln!("POKE restored → vrm wall = {st_r} (expect {vrm})");
                }
                gpu.set_volt_rail_slot(0, 2, step)
                    .expect("region verify reperturb");
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
    }
    let _ = gpu.set_volt_rail_slot(0, 2, 0);
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!(
        "ptscan done: {chunks} chunks, {} needle sites; final status {:?}",
        all_hits.len(),
        gpu.volt_rails()
            .expect("rails")
            .status
            .first()
            .unwrap()
            .values
    );
    eprintln!("ptscan: ALL needle VAs: {:?}", all_hits);
}

#[test]
#[ignore = "requires embedded pmxdrv + elevated shell; page-table full-frame needle scan (run with --release)"]
fn kmd_volt_ptscan_live() {
    ptscan_phase();
}

#[test]
#[ignore = "requires embedded pmxdrv + elevated shell; patch ALL live offset-field copies (NVOC_VOLT_POKE6_VAS=comma VAs, NVOC_VOLT_POKE6=1)"]
fn kmd_volt_offset_patch_live() {
    let list = std::env::var("NVOC_VOLT_POKE6_VAS").expect("NVOC_VOLT_POKE6_VAS=va1,va2,...");
    nvapi::initialize().expect("nvapi init");
    let Some(gpu) = nvapi::PhysicalGpu::enumerate()
        .ok()
        .and_then(|g| g.into_iter().next())
    else {
        panic!("NVAPI GPU unreachable");
    };
    let vrm = gpu
        .volt_rails()
        .expect("rails")
        .status
        .first()
        .unwrap()
        .values[3] as u32;
    let drv = PmxDrv::connect().expect("PMXDRV connect failed");
    let module = find_loaded_module("nvlddmkm.sys").expect("nvlddmkm not loaded");
    let img = std::fs::read(&module.path).expect("disk image read");
    let fingerprint = PeFingerprint::from_image(&img).expect("bad PE");
    let phys = CachedPhys::new(&drv);
    let discovery = discover_root(&phys, module.base, &fingerprint);
    let walk_root = discovery.unique_root().expect("root not unique");

    let vasp: Vec<u64> = list
        .split(',')
        .filter_map(|x| u64::from_str_radix(x.trim().trim_start_matches("0x"), 16).ok())
        .collect();
    println!(
        "patch targets: {:?} (vrm base {vrm})",
        vasp.iter()
            .map(|v| format!("0x{v:016X}"))
            .collect::<Vec<_>>()
    );

    let rd = |va: u64| -> Option<u32> {
        let pa = translate(&phys, walk_root, va).ok()?;
        let mapped = drv.map_physical(pa & !0xFFF, 1).ok()?;
        if mapped == 0 || mapped == 0xBEEF {
            return None;
        }
        let off = (va & 0xFFF) as usize;
        let page = unsafe { std::slice::from_raw_parts(mapped as *const u8, 4096) };
        let d = u32::from_le_bytes(page[off..off + 4].try_into().unwrap());
        let _ = drv.unmap_physical(mapped);
        Some(d)
    };
    let wr = |va: u64, v: u32| {
        let pa = translate(&phys, walk_root, va).expect("translate");
        let mapped = drv.map_physical(pa & !0xFFF, 1).expect("map");
        unsafe {
            ((mapped + (va & 0xFFF)) as *mut u32).write_volatile(v);
        }
        let _ = drv.unmap_physical(mapped);
    };

    // 基线:当前 nvapi 偏移(slot2 现值),站点应全部等于它
    let cur_off = gpu
        .volt_rails()
        .expect("rails")
        .control
        .first()
        .unwrap()
        .values[2] as u32;
    for &va in &vasp {
        let d = rd(va).unwrap_or_else(|| panic!("site 0x{va:016X} unreadable"));
        assert_eq!(
            d, cur_off,
            "site 0x{va:016X} = {d}, expected {cur_off} — state drifted, redo scan"
        );
    }
    println!("identity OK: all sites = {cur_off} (= nvapi slot2 offset)");

    for &delta_uv in &[43750u32, 100000] {
        let target_val = cur_off.wrapping_add(delta_uv);
        for &va in &vasp {
            wr(va, target_val);
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        let st = gpu
            .volt_rails()
            .expect("rails")
            .status
            .first()
            .unwrap()
            .values;
        println!(
            "patch ALL sites -> {target_val}: nvapi vrm wall = {} (expect {})",
            st[3],
            vrm + target_val
        );
        println!("  full status: {st:?}");
        if st[3] as u32 == vrm + target_val {
            println!("  >>> WALL RAISED VIA KERNEL PATCH — lane confirmed <<<");
        }
        // 回基线
        for &va in &vasp {
            wr(va, cur_off);
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        let back = gpu
            .volt_rails()
            .expect("rails")
            .status
            .first()
            .unwrap()
            .values[3] as u32;
        println!("restored -> {back} (expect {vrm})");
        assert_eq!(back, vrm, "restore failed — patch manually!");
    }
    println!("poke6 done — slot2-independent kernel offset write lane validated");
}

#[test]
#[ignore = "requires embedded pmxdrv + elevated shell; verify candidate VAs follow nvapi slot2 writes (NVOC_PTSCAN_VERIFY holds comma-separated VAs)"]
fn kmd_volt_ptscan_verify_live() {
    let list = std::env::var("NVOC_PTSCAN_VERIFY").expect("NVOC_PTSCAN_VERIFY=va1,va2,...");
    nvapi::initialize().expect("nvapi init");
    let Some(gpu) = nvapi::PhysicalGpu::enumerate()
        .ok()
        .and_then(|g| g.into_iter().next())
    else {
        panic!("NVAPI GPU unreachable");
    };
    let vrm = gpu
        .volt_rails()
        .expect("rails")
        .status
        .first()
        .unwrap()
        .values[3] as u32;
    let drv = PmxDrv::connect().expect("PMXDRV connect failed");
    let module = find_loaded_module("nvlddmkm.sys").expect("nvlddmkm.sys not loaded");
    let img = std::fs::read(&module.path).expect("disk image read");
    let fingerprint = PeFingerprint::from_image(&img).expect("bad PE");
    let phys = CachedPhys::new(&drv);
    let discovery = discover_root(&phys, module.base, &fingerprint);
    let walk_root = discovery.unique_root().expect("root not unique");

    let rd_va = |va: u64| -> Option<u32> {
        let pa = translate(&phys, walk_root, va).ok()?;
        let mapped = drv.map_physical(pa & !0xFFF, 1).ok()?;
        if mapped == 0 || mapped == 0xBEEF {
            return None;
        }
        let off = (va & 0xFFF) as usize;
        let page = unsafe { std::slice::from_raw_parts(mapped as *const u8, 4096) };
        let d = u32::from_le_bytes(page[off..off + 4].try_into().unwrap());
        let _ = drv.unmap_physical(mapped);
        Some(d)
    };

    let candidates: Vec<u64> = list
        .split(',')
        .filter_map(|s| u64::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
        .collect();
    println!(
        "verify {} candidate VAs (vrm base = {vrm})",
        candidates.len()
    );
    for phase in [0u32, 6250, 0] {
        gpu.set_volt_rail_slot(0, 2, phase as i32)
            .expect("slot2 write");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let st = gpu
            .volt_rails()
            .expect("rails")
            .status
            .first()
            .unwrap()
            .values[3] as u32;
        println!("--- slot2 offset = {phase}, nvapi vrm wall = {st} ---");
        for &va in &candidates {
            match rd_va(va) {
                Some(d) => {
                    let follows = d == vrm + phase;
                    println!(
                        "  0x{va:016X} = {d} {}",
                        if follows {
                            "<— FOLLOWS nvapi write (execution-layer!)"
                        } else {
                            ""
                        }
                    );
                }
                None => println!("  0x{va:016X} = <unreadable>"),
            }
        }
    }
    println!("verify done — slot2 left at 0");
}

#[test]
#[ignore = "requires embedded pmxdrv + elevated shell; kernel-pool sweep for the voltage policy block (run with --release)"]
fn gsp_pool_value_sweep_live() {
    // ---- 活体电压值(slot2 扰动出活诱饵,静态副本不动)
    let Some(gpu) = nvapi::PhysicalGpu::enumerate()
        .ok()
        .and_then(|g| g.into_iter().next())
    else {
        panic!("NVAPI GPU unreachable — no live values to sweep for");
    };
    nvapi::initialize().expect("nvapi init");
    let rails = gpu.volt_rails().expect("volt_rails");
    let status = *rails.status.first().expect("no status entry");
    let control = *rails.control.first().expect("no control entry");
    let step: i32 = gpu
        .volt_devices()
        .ok()
        .and_then(|d| d.first().map(|d| d.step_uV))
        .filter(|s| *s > 0)
        .unwrap_or(6250) as i32;
    let vrm = status.values[3] as u32;
    let needle = vrm.wrapping_add(step as u32);
    let minhold = status.values[5] as u32;
    let target_wall = status.values[1] as u32;
    let vbios_wall = status.values[2] as u32;
    let targets: Vec<(u32, String)> = vec![
        (needle, "vrm_perturbed NEEDLE".into()),
        (minhold, "minhold".into()),
        (target_wall, "target_wall".into()),
        (vbios_wall, "vbios_wall".into()),
    ];
    println!("sweep targets: {targets:?}");

    // slot2 扰动(扫描期间保持;收尾还原并验证)
    let cur_slot2 = control.values[2];
    let retained = gpu
        .set_volt_rail_slot(0, 2, cur_slot2.wrapping_add(step))
        .expect("slot2 perturb refused");
    println!(
        "slot2 perturbed: {} -> {} (retained {retained}); needle live",
        cur_slot2,
        cur_slot2.wrapping_add(step)
    );
    let st_now = gpu
        .volt_rails()
        .expect("rails")
        .status
        .first()
        .unwrap()
        .values[3] as u32;
    assert_eq!(st_now, needle, "nvapi vrm wall did not follow");

    // ---- 传输 + 走查根(VA→PA 需要它;服务由外部拉起)
    let _svc = PmxDrvServiceGuard;
    let drv = PmxDrv::connect().expect("PMXDRV connect failed");
    let module = find_loaded_module("nvlddmkm.sys").expect("nvlddmkm.sys not loaded");
    let img = std::fs::read(&module.path).expect("failed to read in-service image");
    let layout = probe_layout(&img).expect("layout derivation failed");
    let fingerprint = PeFingerprint::from_image(&img).expect("bad PE header");
    let phys = CachedPhys::new(&drv);
    let discovery = discover_root(&phys, module.base, &fingerprint);
    let walk_root = discovery.unique_root().expect("page-table root not unique");
    println!(
        "walk root 0x{walk_root:016X} (nvlddmkm @ {:#x}, layout slot={:#x})",
        module.base, layout.global_slot_rva
    );

    // ---- 池枚举 + 标签直方图
    let all_blocks = query_big_pool();
    let mut blocks: Vec<PoolBlock> = all_blocks
        .iter()
        .map(|b| PoolBlock {
            va: b.va,
            nonpaged: b.nonpaged,
            size: b.size,
            tag: b.tag,
        })
        .collect();
    blocks.retain(|b| b.size >= PAGE as u64 && b.va != 0);
    let total_gb: f64 =
        blocks.iter().map(|b| b.size as f64).sum::<f64>() / 1024.0 / 1024.0 / 1024.0;
    println!(
        "big pool: {} blocks (≥1 page), {total_gb:.2} GiB total",
        blocks.len()
    );
    let mut hist: HashMap<[u8; 4], (usize, u64)> = HashMap::new();
    for b in &blocks {
        let e = hist.entry(b.tag).or_insert((0, 0));
        e.0 += 1;
        e.1 += b.size;
    }
    let mut hist: Vec<_> = hist.into_iter().collect();
    hist.sort_unstable_by_key(|(_, (c, sz))| (std::cmp::Reverse(*sz), std::cmp::Reverse(*c)));
    println!("tag histogram (top 40 by bytes):");
    for (tag, (c, sz)) in hist.iter().take(40) {
        println!(
            "  '{}': {c} blocks, {:.2} MiB",
            tag_str(tag),
            *sz as f64 / 1024.0 / 1024.0
        );
    }
    println!("giant blocks (≥16 MiB):");
    for b in blocks
        .iter()
        .filter(|b| b.size >= 16 * 1024 * 1024)
        .take(24)
    {
        println!(
            "  va {:#018x} {:.2} MiB tag '{}' {}",
            b.va,
            b.size as f64 / 1024.0 / 1024.0,
            tag_str(&b.tag),
            if b.nonpaged { "NP" } else { "P" }
        );
    }

    // ---- 预算 + 逐块扫查(按块大小降序)
    let max_gb: f64 = std::env::var("NVOC_SWEEP_MAX_POOL_GB")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v| v > 0.0)
        .unwrap_or(8.0);
    let max_bytes = (max_gb * 1024.0 * 1024.0 * 1024.0) as u64;
    blocks.sort_unstable_by_key(|b| std::cmp::Reverse(b.size));

    let mut hits: Vec<PageHit> = Vec::new();
    let mut single_value_pages = 0usize;
    let mut needle_only_pages: Vec<(u64, usize)> = Vec::new();
    let mut swept = 0u64;
    let mut gb_mark = 0u64;
    let mut untranslated = 0u64;
    for b in &blocks {
        if swept >= max_bytes {
            println!("budget cap {max_gb} GiB reached — stopping (residual blocks not swept)");
            break;
        }
        let pages = (b.size as usize) / PAGE;
        for pi in 0..pages {
            let va = b.va + (pi * PAGE) as u64;
            let Ok(pa) = translate(&phys, walk_root, va) else {
                untranslated += 1;
                continue;
            };
            let frame = pa & !0xFFF;
            let Ok(mapped) = drv.map_physical(frame, 1) else {
                untranslated += 1;
                continue;
            };
            let page = unsafe { std::slice::from_raw_parts(mapped as *const u8, PAGE) };
            let found = scan_page(page, &targets);
            let needle_offs: Vec<usize> = page
                .chunks_exact(4)
                .enumerate()
                .filter(|(_, c)| u32::from_le_bytes((*c).try_into().unwrap()) == needle)
                .map(|(i, _)| i * 4)
                .collect();
            if needle_offs.len() == 1 && found.len() < 2 {
                needle_only_pages.push((frame, needle_offs[0]));
            }
            if found.len() >= 2 {
                hits.push((frame, found));
            } else if found.len() == 1 {
                single_value_pages += 1;
            }
            let _ = drv.unmap_physical(mapped);
        }
        swept += b.size;
        if swept - gb_mark >= 1024 * 1024 * 1024 {
            gb_mark = swept;
            println!(
                "  … swept {:.2} GiB, hits {}, single-value pages {}, needle-only {}",
                swept as f64 / 1024.0 / 1024.0 / 1024.0,
                hits.len(),
                single_value_pages,
                needle_only_pages.len()
            );
        }
    }

    // ---- 报告
    println!(
        "sweep done: {:.2} GiB swept, {} hits (≥2 distinct live values / page), {} single-value pages, {} pages untranslatable/unmappable",
        swept as f64 / 1024.0 / 1024.0 / 1024.0,
        hits.len(),
        single_value_pages,
        untranslated
    );
    for (frame, vals) in hits.iter().take(16) {
        let vs = vals
            .iter()
            .map(|(v, name, offs)| {
                let os = offs
                    .iter()
                    .take(8)
                    .map(|o| format!("{o:#x}"))
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "{v} {name} @[{os}]{}",
                    if offs.len() > 8 { "…" } else { "" }
                )
            })
            .collect::<Vec<_>>()
            .join("  ");
        println!("  HIT frame {frame:#012x} (page-aligned): {vs}");
    }
    if hits.len() > 16 {
        println!("  …(+{} hits omitted)", hits.len() - 16);
    }
    if let Some((frame, _)) = hits.first()
        && let Ok(va) = drv.map_physical(*frame, 1)
    {
        let page = unsafe { std::slice::from_raw_parts(va as *const u8, PAGE) };
        println!("hexdump frame {frame:#012x} [0..0x80]:");
        for (row, chunk) in page[..0x80].chunks(16).enumerate() {
            println!(
                "    +{:04x}: {}",
                row * 16,
                chunk.iter().map(|b| format!("{b:02x}")).collect::<String>()
            );
        }
        let _ = drv.unmap_physical(va);
    }
    // ---- 小池块扫描(NVOC_VOLT_SMALLBLOCK=1):主 sweep 只扫 ≥1 页块;
    // 电压策略对象可能是 sub-page 小分配(<4KB)。slot2 扰动活着(needle
    // 1131250 + 基墙 1125000 + minhold/target/vbios 全值共现判据)。
    if env::var("NVOC_VOLT_SMALLBLOCK").as_deref() == Ok("1") {
        let small: Vec<&PoolBlock> = all_blocks
            .iter()
            .filter(|b| b.size < PAGE as u64 && b.size >= 32 && b.va != 0)
            .collect();
        println!(
            "SMALLBLOCK pass: {} sub-page blocks (32B..4KB)",
            small.len()
        );
        let mut sb_hits = 0usize;
        let mut sb_scanned = 0usize;
        for b in &small {
            let va = b.va & !0xFu64;
            let Ok(pa) = translate(&phys, walk_root, va) else {
                continue;
            };
            let frame = pa & !0xFFF;
            let Ok(mapped) = drv.map_physical(frame, 1) else {
                continue;
            };
            let in_page = (va & 0xFFF) as usize;
            let scan_len = (b.size as usize).min(PAGE - in_page);
            if scan_len < 8 {
                let _ = drv.unmap_physical(mapped);
                continue;
            }
            let page = unsafe { std::slice::from_raw_parts(mapped as *const u8, PAGE) };
            let region = &page[in_page..in_page + scan_len];
            sb_scanned += 1;
            let mut found = Vec::new();
            for &v in &[needle, minhold, target_wall, vbios_wall, vrm] {
                let offs: Vec<usize> = region
                    .chunks_exact(4)
                    .enumerate()
                    .filter(|(_, c)| u32::from_le_bytes((*c).try_into().unwrap()) == v)
                    .map(|(i, _)| i * 4)
                    .collect();
                if !offs.is_empty() {
                    found.push(format!(
                        "{v}@{:?}",
                        offs.iter().map(|o| format!("{o:#x}")).collect::<Vec<_>>()
                    ));
                }
            }
            if !found.is_empty() {
                sb_hits += 1;
                println!(
                    "SMALL-HIT va 0x{:016X} size {} tag '{}': {}",
                    b.va,
                    b.size,
                    tag_str(&b.tag),
                    found.join("  ")
                );
            }
            let _ = drv.unmap_physical(mapped);
        }
        println!(
            "SMALLBLOCK done: {} blocks scanned, {sb_hits} hits",
            sb_scanned
        );
    }

    // ---- fixedregion 臂(NVOC_VOLT_FIXEDREGION=1):墙值镜像帧跨重启稳定
    // (0x010EA10000 两代 GSP 状态同址)= 固定物理区域结构,非随机池分配。
    // dump 该帧 ±4 页 + slot2 +43750 全域 diff —— 执行层偏移字段若在同区
    // 域即现形。
    if env::var("NVOC_VOLT_FIXEDREGION").as_deref() == Ok("1") {
        let Some((frame0, _)) = hits.first() else {
            panic!("fixedregion: no wall-mirror hit page");
        };
        let frame0 = *frame0;
        println!("FIXEDREGION: anchor frame 0x{frame0:X}, dumping -4..+4 pages");
        let mut pages: Vec<(i64, Vec<u8>)> = Vec::new();
        for k in -4i64..=4 {
            let f = (frame0 as i64 + k * 0x1000) as u64;
            if f == 0 {
                continue;
            }
            if let Ok(mapped) = drv.map_physical(f, 1)
                && mapped != 0
                && mapped != 0xBEEF
            {
                let mut page = vec![0u8; PAGE];
                unsafe {
                    std::ptr::copy_nonoverlapping(mapped as *const u8, page.as_mut_ptr(), PAGE);
                }
                let _ = drv.unmap_physical(mapped);
                pages.push((k, page));
            }
        }
        gpu.set_volt_rail_slot(0, 2, 43750_i32)
            .expect("slot2 +43750");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let st = gpu
            .volt_rails()
            .expect("rails")
            .status
            .first()
            .unwrap()
            .values[3] as u32;
        println!("FIXEDREGION: slot2=+43750, nvapi vrm wall = {st} (base {vrm})");
        let mut any = 0usize;
        for (k, before) in &pages {
            let f = (frame0 as i64 + k * 0x1000) as u64;
            if let Ok(mapped) = drv.map_physical(f, 1)
                && mapped != 0
                && mapped != 0xBEEF
            {
                let page = unsafe { std::slice::from_raw_parts(mapped as *const u8, PAGE) };
                for (o, (a, b)) in before
                    .chunks_exact(4)
                    .zip(page.chunks_exact(4))
                    .enumerate()
                    .map(|(i, (a, b))| (i * 4, (a, b)))
                {
                    let va32 = u32::from_le_bytes(a.try_into().unwrap());
                    let vb32 = u32::from_le_bytes(b.try_into().unwrap());
                    if va32 != vb32 {
                        println!("    DELTA page {k:+} (0x{f:X})+0x{o:03X}: {va32} -> {vb32}");
                        any += 1;
                    }
                }
                let _ = drv.unmap_physical(mapped);
            }
        }
        // 还原 slot2 并复扫
        let _ = gpu.set_volt_rail_slot(0, 2, 0);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let mut reverted = 0usize;
        for (k, orig) in &pages {
            let f = (frame0 as i64 + k * 0x1000) as u64;
            if let Ok(mapped) = drv.map_physical(f, 1)
                && mapped != 0
                && mapped != 0xBEEF
            {
                let page = unsafe { std::slice::from_raw_parts(mapped as *const u8, PAGE) };
                for (o, (a, b)) in orig
                    .chunks_exact(4)
                    .zip(page.chunks_exact(4))
                    .enumerate()
                    .map(|(i, (a, b))| (i * 4, (a, b)))
                {
                    let va32 = u32::from_le_bytes(a.try_into().unwrap());
                    let vb32 = u32::from_le_bytes(b.try_into().unwrap());
                    if va32 != vb32 && vb32.wrapping_sub(va32) == 43750 {
                        reverted += 1;
                        println!(
                            "    RESTORE-FOLLOW page {k:+} (0x{f:X})+0x{o:03X}: {va32} vs {vb32} — offset field!"
                        );
                    }
                }
                let _ = drv.unmap_physical(mapped);
            }
        }
        println!(
            "FIXEDREGION VERDICT: {any} deltas during perturbation, {reverted} restore-following offset fields; final status {:?}",
            gpu.volt_rails()
                .expect("rails")
                .status
                .first()
                .unwrap()
                .values
        );
    }

    // ---- stepsdiff 臂(NVOC_VOLT_STEPDIFF=1):步数编码判别。池基线快照 →
    // slot2 +1 步 → delta==+1 站点集 A → +7 步 → delta==+7 站点集 B →
    // A∩B = 步数字段(µV 字段会在 A 出 6250/B 出 43750,被排除)。命中即
    // patch 步数 1→9(+56.25mV)验证。
    if env::var("NVOC_VOLT_STEPDIFF").as_deref() == Ok("1") {
        // 基线快照
        let mut base: Vec<(u64, Vec<u8>)> = Vec::new();
        for b in &blocks {
            let pages = (b.size as usize) / PAGE;
            for pi in 0..pages {
                let va = b.va + (pi * PAGE) as u64;
                if let Ok(pa) = translate(&phys, walk_root, va) {
                    let frame = pa & !0xFFF;
                    if let Ok(mapped) = drv.map_physical(frame, 1) {
                        let mut page = vec![0u8; PAGE];
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                mapped as *const u8,
                                page.as_mut_ptr(),
                                PAGE,
                            );
                        }
                        let _ = drv.unmap_physical(mapped);
                        base.push((va, page));
                    }
                }
            }
        }
        println!("STEPDIFF baseline: {} pages", base.len());

        let run = |steps: i32, base: &[(u64, Vec<u8>)]| -> Vec<(u64, usize, u32, u32)> {
            gpu.set_volt_rail_slot(0, 2, steps)
                .expect("slot2 step write");
            std::thread::sleep(std::time::Duration::from_millis(300));
            let mut out = Vec::new();
            for (va, before) in base {
                if let Ok(pa) = translate(&phys, walk_root, *va) {
                    let frame = pa & !0xFFF;
                    if let Ok(mapped) = drv.map_physical(frame, 1) {
                        let page = unsafe { std::slice::from_raw_parts(mapped as *const u8, PAGE) };
                        for (o, (a, b)) in before
                            .chunks_exact(4)
                            .zip(page.chunks_exact(4))
                            .enumerate()
                            .map(|(i, (a, b))| (i * 4, (a, b)))
                        {
                            let va32 = u32::from_le_bytes(a.try_into().unwrap());
                            let vb32 = u32::from_le_bytes(b.try_into().unwrap());
                            let d = vb32.wrapping_sub(va32);
                            if d == steps as u32 {
                                out.push((*va, o, va32, vb32));
                            }
                        }
                        let _ = drv.unmap_physical(mapped);
                    }
                }
            }
            println!("STEPDIFF run +{steps} step: {} sites", out.len());
            out
        };

        let run1 = run(1, &base);
        let run7 = run(7, &base);
        gpu.set_volt_rail_slot(0, 2, 0).expect("slot2 reset");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let s1: std::collections::HashSet<(u64, usize)> =
            run1.iter().map(|(v, o, _, _)| (*v, *o)).collect();
        let cross: Vec<&(u64, usize, u32, u32)> = run7
            .iter()
            .filter(|(v, o, _, _)| s1.contains(&(*v, *o)))
            .collect();
        println!(
            "STEPDIFF cross (delta 1 then 7, same site): {}",
            cross.len()
        );
        for (va, o, old, new) in &cross {
            println!("  STEP-FIELD va 0x{va:016X}+0x{o:03X}: {old} -> {new}");
        }
        if !cross.is_empty() {
            // 验证:patch 步数 1 -> 9(wall 应= base + 9*6250)
            for (va, o, _, _) in &cross {
                if let Ok(pa) = translate(&phys, walk_root, *va) {
                    let mapped = drv.map_physical(pa & !0xFFF, 1).expect("sd map");
                    unsafe {
                        ((mapped + (*o as u64)) as *mut u32).write_volatile(9u32);
                    }
                    let _ = drv.unmap_physical(mapped);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
            let st = gpu
                .volt_rails()
                .expect("rails")
                .status
                .first()
                .unwrap()
                .values[3] as u32;
            println!(
                "STEPDIFF patch steps->9: nvapi vrm wall = {st} (expect {} if execution-layer = base + 9*6250)",
                vrm + 9 * 6250
            );
            for (va, o, _, _) in &cross {
                if let Ok(pa) = translate(&phys, walk_root, *va) {
                    let mapped = drv.map_physical(pa & !0xFFF, 1).expect("sd restore");
                    unsafe {
                        ((mapped + (*o as u64)) as *mut u32).write_volatile(1u32);
                    }
                    let _ = drv.unmap_physical(mapped);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        let _ = gpu.set_volt_rail_slot(0, 2, 0);
        std::thread::sleep(std::time::Duration::from_millis(300));
        println!(
            "STEPDIFF done; final status {:?}",
            gpu.volt_rails()
                .expect("rails")
                .status
                .first()
                .unwrap()
                .values
        );
    }

    println!(
        "VERDICT (pool existence): {}",
        if !hits.is_empty() || !needle_only_pages.is_empty() {
            "voltage policy values FOUND in kernel pool — native-RM host-side mirror exists; next: identify consumer (pool tag) + field semantics before any write probe"
        } else {
            "voltage policy values NOT FOUND in kernel pool — state is elsewhere (MDL carveout / VRAM / image-section candidate from the major phase)"
        }
    );

    // ---- poke 臂(NVOC_VOLT_POKE=1):命中页 vbios 墙 +50 mV 单字段补丁 →
    // nvapi status 随动读回 → 还原。随动 = 执行层字段实锤。
    if env::var("NVOC_VOLT_POKE").as_deref() == Ok("1") {
        let Some((frame, vals)) = hits
            .iter()
            .find(|(_, vals)| vals.iter().any(|(v, _, _)| *v == vbios_wall))
            .map(|(f, vals)| (*f, vals.clone()))
        else {
            panic!("poke arm: no hit page carries the vbios wall");
        };
        let v_off = vals
            .iter()
            .find(|(v, _, _)| *v == vbios_wall)
            .and_then(|(_, _, offs)| offs.first())
            .expect("vbios offset");
        println!(
            "POKE: pa {frame:#012x}+{v_off:#04x} vbios wall {vbios_wall} -> {} (+50 mV)",
            vbios_wall + 50000
        );
        let mapped = drv.map_physical(frame, 1).expect("poke map failed");
        unsafe {
            ((mapped + *v_off as u64) as *mut u32).write_volatile(vbios_wall + 50000);
        }
        let _ = drv.unmap_physical(mapped);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let after = gpu
            .volt_rails()
            .expect("rails")
            .status
            .first()
            .unwrap()
            .values;
        println!(
            "nvapi status after poke: {after:?} (values[2] expect {})",
            vbios_wall + 50000
        );
        let followed = after[2] as u32 == vbios_wall + 50000;
        // 还原
        let mapped = drv.map_physical(frame, 1).expect("restore map failed");
        unsafe {
            ((mapped + *v_off as u64) as *mut u32).write_volatile(vbios_wall);
        }
        let _ = drv.unmap_physical(mapped);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let back = gpu
            .volt_rails()
            .expect("rails")
            .status
            .first()
            .unwrap()
            .values;
        println!("nvapi status after restore: {back:?}");
        println!(
            "POKE VERDICT: {}",
            if followed {
                "vbios wall field is EXECUTION-LAYER — kernel physical patch reaches the live wall"
            } else {
                "field did not follow — mirror/info row; keep hunting (consumer distinguishes)"
            }
        );
    }

    // ---- poke2 臂(NVOC_VOLT_POKE2=1):奇数偏移探针定位「偏移槽+基墙」,
    // 逐 site patch → nvapi 随动 → 还原。墙 = 基墙 + 偏移(GET 现算)模型下,
    // 偏移槽/基墙任一 patch 到位即抬墙。
    if env::var("NVOC_VOLT_POKE2").as_deref() == Ok("1") {
        let odd: u32 = 54321;
        let base_vrm = vrm; // 1125000(slot2=0 时)
        gpu.set_volt_rail_slot(0, 2, odd as i32)
            .expect("slot2 odd perturb");
        std::thread::sleep(std::time::Duration::from_millis(200));
        let st_now = gpu
            .volt_rails()
            .expect("rails")
            .status
            .first()
            .unwrap()
            .values[3] as u32;
        println!(
            "POKE2: slot2={odd}, nvapi vrm wall now {st_now} (base {base_vrm} + odd = {})",
            base_vrm + odd
        );
        let targets2: Vec<(u32, String)> = vec![
            (odd, "slot2_offset ODD NEEDLE".into()),
            (base_vrm, "vrm_base".into()),
            (minhold, "minhold".into()),
            (target_wall, "target_wall".into()),
            (vbios_wall, "vbios_wall".into()),
        ];
        #[allow(clippy::type_complexity)]
        let mut site_pages: Vec<(u64, Vec<(u32, String, Vec<usize>)>)> = Vec::new();
        for b in &blocks {
            let pages = (b.size as usize) / PAGE;
            for pi in 0..pages {
                let va = b.va + (pi * PAGE) as u64;
                let Ok(pa) = translate(&phys, walk_root, va) else {
                    continue;
                };
                let frame = pa & !0xFFF;
                let Ok(mapped) = drv.map_physical(frame, 1) else {
                    continue;
                };
                let page = unsafe { std::slice::from_raw_parts(mapped as *const u8, PAGE) };
                let found = scan_page(page, &targets2);
                let _ = drv.unmap_physical(mapped);
                if found.iter().any(|(v, _, _)| *v == odd) {
                    println!(
                        "POKE2-HIT frame {frame:#012x}: {}",
                        found
                            .iter()
                            .map(|(v, name, offs)| format!(
                                "{name}({v})@{:?}",
                                offs.iter()
                                    .take(6)
                                    .map(|o| format!("{o:#x}"))
                                    .collect::<Vec<_>>()
                            ))
                            .collect::<Vec<_>>()
                            .join("  ")
                    );
                    site_pages.push((frame, found));
                }
            }
        }
        // 逐 site patch 试验:偏移槽 54321 → 100000(wall 应=1125000+100000)
        let mut verified = 0usize;
        for (frame, found) in &site_pages {
            let Some((_, _, offs)) = found.iter().find(|(v, _, _)| *v == odd) else {
                continue;
            };
            for off in offs.iter().take(2) {
                let mapped = drv.map_physical(*frame, 1).expect("poke2 map");
                unsafe {
                    ((mapped + *off as u64) as *mut u32).write_volatile(100000u32);
                }
                let _ = drv.unmap_physical(mapped);
                std::thread::sleep(std::time::Duration::from_millis(250));
                let st = gpu
                    .volt_rails()
                    .expect("rails")
                    .status
                    .first()
                    .unwrap()
                    .values[3] as u32;
                println!(
                    "POKE2 patch pa {frame:#012x}+{off:#04x}: offset 54321 -> 100000; nvapi vrm wall = {st} (expect 2125000 if execution-layer)"
                );
                // 还原
                let mapped = drv.map_physical(*frame, 1).expect("poke2 restore");
                unsafe {
                    ((mapped + *off as u64) as *mut u32).write_volatile(odd);
                }
                let _ = drv.unmap_physical(mapped);
                std::thread::sleep(std::time::Duration::from_millis(250));
                let back = gpu
                    .volt_rails()
                    .expect("rails")
                    .status
                    .first()
                    .unwrap()
                    .values[3] as u32;
                if back == base_vrm + odd {
                    println!("  restored ({back})");
                    if st == base_vrm + 100000 {
                        verified += 1;
                    }
                } else {
                    println!("  restore mismatch: {back} — inspect");
                }
            }
        }
        // slot2 归零
        let _ = gpu.set_volt_rail_slot(0, 2, 0);
        std::thread::sleep(std::time::Duration::from_millis(200));
        println!(
            "POKE2 VERDICT: {verified} execution-layer offset-field site(s); final status {:?}",
            gpu.volt_rails()
                .expect("rails")
                .status
                .first()
                .unwrap()
                .values
        );
    }

    // ---- poke4 臂(NVOC_VOLT_POKE4=1):池级 delta-diff。奇数探针被 6250µV
    // 步进量化堵死 → 改用「全池快照 → slot2 +7步(43750) → 重扫 → 精确过滤
    // 差值==+43750 的 dword」——执行层偏移字段必现形。命中后逐 site patch
    // (+8 步)验证 → 还原。
    if env::var("NVOC_VOLT_POKE4").as_deref() == Ok("1") {
        let delta: u32 = 7 * 6250; // 43750,少见步数
        println!("POKE4: snapshot pool → slot2 +{delta} → delta-diff");

        // 全池逐页快照
        let mut snap: Vec<(u64, Vec<u8>)> = Vec::new();
        for b in &blocks {
            let pages = (b.size as usize) / PAGE;
            for pi in 0..pages {
                let va = b.va + (pi * PAGE) as u64;
                let Ok(pa) = translate(&phys, walk_root, va) else {
                    continue;
                };
                let frame = pa & !0xFFF;
                let Ok(mapped) = drv.map_physical(frame, 1) else {
                    continue;
                };
                let mut page = vec![0u8; PAGE];
                unsafe {
                    std::ptr::copy_nonoverlapping(mapped as *const u8, page.as_mut_ptr(), PAGE);
                }
                let _ = drv.unmap_physical(mapped);
                snap.push((frame, page));
            }
        }
        println!(
            "POKE4 snapshot: {} pages, {:.2} GiB",
            snap.len(),
            snap.len() as f64 * 4096.0 / (1 << 30) as f64
        );

        gpu.set_volt_rail_slot(0, 2, delta as i32)
            .expect("slot2 +43750 refused");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let st = gpu
            .volt_rails()
            .expect("rails")
            .status
            .first()
            .unwrap()
            .values[3] as u32;
        println!(
            "POKE4: slot2=+{delta}, nvapi vrm wall = {st} (expect {} if unclamped)",
            vrm + delta
        );

        // 重扫 + delta 过滤
        let mut sites: Vec<(u64, usize, u32, u32)> = Vec::new();
        for (frame, before) in &snap {
            let Ok(mapped) = drv.map_physical(*frame, 1) else {
                continue;
            };
            let page = unsafe { std::slice::from_raw_parts(mapped as *const u8, PAGE) };
            for (o, (a, b)) in before
                .chunks_exact(4)
                .zip(page.chunks_exact(4))
                .enumerate()
                .map(|(i, (a, b))| (i * 4, (a, b)))
            {
                let va = u32::from_le_bytes(a.try_into().unwrap());
                let vb = u32::from_le_bytes(b.try_into().unwrap());
                if va != vb && vb.wrapping_sub(va) == delta {
                    println!("    DELTA-HIT pa {frame:#012x}+{o:#04x}: {va} -> {vb}");
                    sites.push((*frame, o, va, vb));
                }
            }
            let _ = drv.unmap_physical(mapped);
        }
        println!("POKE4: {} delta sites", sites.len());

        // 逐 site 再抬 8 步验证 + 还原
        let mut verified = 0usize;
        for (frame, off, old, _) in &sites {
            let patched = old.wrapping_add(8 * 6250);
            let mapped = drv.map_physical(*frame, 1).expect("poke4 map");
            unsafe {
                ((mapped + *off as u64) as *mut u32).write_volatile(patched);
            }
            let _ = drv.unmap_physical(mapped);
            std::thread::sleep(std::time::Duration::from_millis(250));
            let st = gpu
                .volt_rails()
                .expect("rails")
                .status
                .first()
                .unwrap()
                .values[3] as u32;
            println!(
                "POKE4 patch pa {frame:#012x}+{off:#04x}: {old} -> {patched}; nvapi vrm wall = {st}"
            );
            // 还原
            let mapped = drv.map_physical(*frame, 1).expect("poke4 restore");
            unsafe {
                ((mapped + *off as u64) as *mut u32).write_volatile(*old);
            }
            let _ = drv.unmap_physical(mapped);
            std::thread::sleep(std::time::Duration::from_millis(250));
            let back = gpu
                .volt_rails()
                .expect("rails")
                .status
                .first()
                .unwrap()
                .values[3] as u32;
            let expect = vrm.wrapping_add(delta);
            if st == vrm.wrapping_add(8 * 6250) && back == expect {
                verified += 1;
                println!("  VERIFIED execution-layer (wall {st} then back {back})");
            } else {
                println!("  restored ({back})");
            }
        }
        let _ = gpu.set_volt_rail_slot(0, 2, 0);
        std::thread::sleep(std::time::Duration::from_millis(200));
        println!(
            "POKE4 VERDICT: {verified} execution-layer site(s); final status {:?}",
            gpu.volt_rails()
                .expect("rails")
                .status
                .first()
                .unwrap()
                .values
        );
    }

    // ---- stepdiff 臂(NVOC_VOLT_STEPDIFF=1):双步数交叉 diff。poke4 只过滤了
    // µV 差值,按步数存储的字段(+1/+7)会漏。基线快照(池+镜像可写节)→
    // µV 差值,按步数存储的字段(+1/+7)会漏。基线快照(池+镜像可写节)→
    // slot2 +1 步(6250)→ 扫 delta∈{+6250,+1} → slot2 +7 步(43750)→ 扫
    // delta(相对基线)∈{+43750,+7} → 交叉:两轮都命中的站点 = 步数字段
    // (delta 序列 1→7)或 µV 字段(6250→43750)。
    if env::var("NVOC_VOLT_STEPDIFF").as_deref() == Ok("1") {
        println!("STEPDIFF: baseline snapshot (pool + image writable sections)…");
        let mut pages: Vec<(u64, u64, Vec<u8>)> = Vec::new(); // (va, frame, bytes)
        for b in &blocks {
            let n = (b.size as usize) / PAGE;
            for pi in 0..n {
                let va = b.va + (pi * PAGE) as u64;
                if let Ok(pa) = translate(&phys, walk_root, va) {
                    let frame = pa & !0xFFF;
                    if let Ok(mapped) = drv.map_physical(frame, 1) {
                        let mut page = vec![0u8; PAGE];
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                mapped as *const u8,
                                page.as_mut_ptr(),
                                PAGE,
                            );
                        }
                        let _ = drv.unmap_physical(mapped);
                        pages.push((va, frame, page));
                    }
                }
            }
        }
        for (_name, va, len) in writable_sections_for(&phys, walk_root, module.base) {
            for o in (0..len).step_by(PAGE) {
                let pg = va + o as u64;
                if let Ok(page) = read_virtual(&phys, walk_root, pg, PAGE) {
                    let frame = translate(&phys, walk_root, pg)
                        .map(|p| p & !0xFFF)
                        .unwrap_or(0);
                    pages.push((pg, frame, page));
                }
            }
        }
        println!("STEPDIFF baseline: {} pages", pages.len());

        let run = |steps: u32| -> HashMap<(u64, usize), (u32, u32)> {
            gpu.set_volt_rail_slot(0, 2, (steps * 6250) as i32)
                .expect("slot2 step perturb");
            std::thread::sleep(std::time::Duration::from_millis(300));
            let mut out: HashMap<(u64, usize), (u32, u32)> = HashMap::new();
            for (_va, frame, before) in &pages {
                if let Ok(mapped) = drv.map_physical(*frame, 1) {
                    let page = unsafe { std::slice::from_raw_parts(mapped as *const u8, PAGE) };
                    for (o, (a, b)) in before
                        .chunks_exact(4)
                        .zip(page.chunks_exact(4))
                        .enumerate()
                        .map(|(i, (a, b))| (i * 4, (a, b)))
                    {
                        let va32 = u32::from_le_bytes(a.try_into().unwrap());
                        let vb32 = u32::from_le_bytes(b.try_into().unwrap());
                        let d = vb32.wrapping_sub(va32);
                        if steps > 0 && (d == steps * 6250 || d == steps) {
                            out.insert((*frame, o), (va32, vb32));
                        }
                    }
                    let _ = drv.unmap_physical(mapped);
                }
            }
            println!("  STEPDIFF run +{steps} steps: {} sites", out.len());
            out
        };

        let run1 = run(1);
        let run7 = run(7);
        gpu.set_volt_rail_slot(0, 2, 0).expect("slot2 reset");
        std::thread::sleep(std::time::Duration::from_millis(300));
        println!(
            "STEPDIFF final status: {:?}",
            gpu.volt_rails()
                .expect("rails")
                .status
                .first()
                .unwrap()
                .values
        );

        // 交叉:两轮同站点
        let mut both: Vec<_> = run1
            .iter()
            .filter(|(k, _)| run7.contains_key(k))
            .map(|(k, (a, b))| (*k, *a, *b, run7[k]))
            .collect();
        both.sort_by_key(|((frame, off), _, _, _)| (*frame, *off));
        println!(
            "STEPDIFF cross-hits (both rounds, same site): {}",
            both.len()
        );
        for ((frame, off), a1, b1, (_, b7)) in &both {
            let kind = if b7.wrapping_sub(*a1) == 37500 {
                "µV-offset field (6250→43750)"
            } else if b7.wrapping_sub(*a1) == 6 {
                "STEP-count field (1→7)"
            } else {
                "inconsistent"
            };
            let va_str = va_lookup(&pages, *frame, *off);
            println!(
                "  pa {frame:#012x}+{off:#04x} (va {va_str}): run1 {a1}->{b1}, run7 ->{b7} — {kind}"
            );
        }
        if both.is_empty() {
            println!(
                "STEPDIFF: no cross-hits — executed state is in MDL carveout / VRAM / other encoding"
            );
        }

        // ---- poke5(NVOC_VOLT_POKE5=1):对交叉命中站点做单位探针 + 定向抬墙。
        // slot2=0 时字段应回基值;patch +1 单位 → nvapi vrm 墙差值 = 单位大小;
        // 再按单位换算 +50 mV 定向抬升;boost 100% 联动观测;全程即改即还原。
        if env::var("NVOC_VOLT_POKE5").as_deref() == Ok("1") && !both.is_empty() {
            let ((frame, off), a1, _b1, _rb7) = both[0];
            let rd_site = || -> Option<u32> {
                let va = drv.map_physical(frame, 1).ok()?;
                if va == 0 || va == 0xBEEF {
                    return None;
                }
                let page = unsafe { std::slice::from_raw_parts(va as *const u8, PAGE) };
                let d = u32::from_le_bytes(page[off..off + 4].try_into().unwrap());
                let _ = drv.unmap_physical(va);
                Some(d)
            };
            let wr_site = |v: u32| {
                let va = drv.map_physical(frame, 1).expect("poke5 map");
                unsafe {
                    ((va + off as u64) as *mut u32).write_volatile(v);
                }
                let _ = drv.unmap_physical(va);
            };
            gpu.set_volt_rail_slot(0, 2, 0).expect("slot2 reset");
            std::thread::sleep(std::time::Duration::from_millis(300));
            let Some(base_val) = rd_site() else {
                panic!("poke5: site unreadable");
            };
            println!("POKE5 site pa {frame:#012x}+{off:#04x} = {base_val} (stock; run1 old {a1})");
            // +1 单位探针
            wr_site(base_val + 1);
            std::thread::sleep(std::time::Duration::from_millis(300));
            let st1 = gpu
                .volt_rails()
                .expect("rails")
                .status
                .first()
                .unwrap()
                .values[3] as i64;
            let unit = st1 - vrm as i64;
            println!("POKE5 +1 unit → nvapi vrm wall {st1} (base {vrm}) — unit = {unit} µV");
            wr_site(base_val);
            std::thread::sleep(std::time::Duration::from_millis(300));
            if unit <= 0 || unit > 6250 {
                println!(
                    "POKE5 VERDICT: unit {unit} µV out of sane band — site is not the wall-offset field; keep hunting"
                );
            } else {
                // 定向抬墙 +50 mV
                let units = (50000 + unit - 1) / unit;
                let raised = base_val + units as u32;
                wr_site(raised);
                std::thread::sleep(std::time::Duration::from_millis(300));
                let st2 = gpu
                    .volt_rails()
                    .expect("rails")
                    .status
                    .first()
                    .unwrap()
                    .values;
                println!(
                    "POKE5 +{units} units (site {base_val}->{raised}) → status {st2:?} (vrm wall expect {})",
                    vrm + 50000
                );
                // boost 100% 联动:target/effective 墙是否朝新 vrm 墙走
                gpu.set_core_voltage_boost(nvapi::Percentage(100))
                    .expect("boost 100");
                std::thread::sleep(std::time::Duration::from_millis(400));
                let st3 = gpu
                    .volt_rails()
                    .expect("rails")
                    .status
                    .first()
                    .unwrap()
                    .values;
                println!("POKE5 boost 100% → status {st3:?} (target/effective walls follow?)");
                // 全部还原
                gpu.set_core_voltage_boost(nvapi::Percentage(0))
                    .expect("boost restore");
                wr_site(base_val);
                std::thread::sleep(std::time::Duration::from_millis(300));
                let st4 = gpu
                    .volt_rails()
                    .expect("rails")
                    .status
                    .first()
                    .unwrap()
                    .values;
                println!("POKE5 restored → status {st4:?}");
                let ok = st4[3] as i64 == vrm as i64 && rd_site() == Some(base_val);
                println!(
                    "POKE5 VERDICT: {}",
                    if ok && st2[3] as i64 == vrm as i64 + 50000 {
                        "EXECUTION-LAYER CONFIRMED — kernel physical patch of this field raises the live vrm wall"
                    } else if ok {
                        "site restores cleanly but +50mV raise did not fully follow — partial semantics, inspect"
                    } else {
                        "restore failed — inspect immediately"
                    }
                );
            }
        }
    }

    // ---- slot2 还原 + needle 页跟随验证(活体字段实锤)
    let _ = gpu.set_volt_rail_slot(0, 2, cur_slot2);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let st_back = gpu
        .volt_rails()
        .expect("rails")
        .status
        .first()
        .unwrap()
        .values[3] as u32;
    println!("slot2 restored → nvapi vrm wall {st_back} (expect {vrm}); needle pages:");
    for (frame, off) in needle_only_pages
        .iter()
        .take(16)
        .map(|(f, o)| (*f, *o))
        .chain(hits.iter().filter_map(|(f, vals)| {
            vals.iter()
                .find(|(v, _, _)| *v == needle)
                .and_then(|(_, _, offs)| offs.first().map(|o| (*f, *o)))
        }))
    {
        if let Ok(va) = drv.map_physical(frame, 1)
            && va != 0
            && va != 0xBEEF
        {
            let page = unsafe { std::slice::from_raw_parts(va as *const u8, PAGE) };
            let d = u32::from_le_bytes(page[off..off + 4].try_into().unwrap());
            let tag: &str = if d == vrm {
                "REVERTED to stock vrm — LIVE FIELD (patch target)"
            } else if d == needle {
                "still perturbed — static copy or untracked"
            } else {
                "other value"
            };
            println!("  pa {frame:#012x}+{off:#04x}: {d} — {tag}");
            let _ = drv.unmap_physical(va);
        }
    }
    let _ = PmxDrvPhysMem::new(&drv);
}
