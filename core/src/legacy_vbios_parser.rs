//! Legacy（Fermi~Pascal 世代）vBIOS BIT 目录解析器：perf / boost / vmap /
//! boost-ladder 四表。
//!
//! 数值语义以 MaxwellBiosTweaker 1.36（GM200）/ KeplerBiosTweaker 1.27
//! （GK104 208576.rom）的 UI 显示为基准逐位校验：
//!
//! - 镜像基址 = **第一个 `55 AA` boot-sector magic 的位置**（KeplerBiosTweaker
//!   的 loader 语义：从 base 起搜 BIT 签名，BIT 条目 offset 与 P 表内全部
//!   u32 指针均为 **base 相对**）。三样本实测：GM200 base=0x0、GK104
//!   208576.rom base=0x600（镜像头是 NVGI，55AA 在 0x600）、GM204 Palit
//!   base=0x800。BIT 条目 6 字节 `(id, ver, len:u16, off:u16)`。
//! - `'P'` 条目 ver 2（len 0x68）：`+0x00` perf、`+0x20` vmap、`+0x30`
//!   boost、`+0x34` boost-ladder，均为 base 相对 u32 指针。
//! - perf v0x40：条目长 0x20 + 9×4B 子条目。子条目 u16 & 0x3fff = MHz 直存，
//!   高位是旗标位。域序 GPC XBAR L2C DDR SYS HUB MSD PWR DISP。
//! - boost v0x11：条目长 6 + 9×6B 子条目。条目级 +2/+4 = GPC min/max，
//!   子条目 = (domain, percent, min, max)。**频率单位为半 MHz**（u16 值
//!   ×0.5 = MHz）；nouveau 上游的 `×1000`（kHz）解释在 Maxwell 上不成立。
//!   domain 字节观测值：1=XBAR 2=L2C 4=GPC，0x10=空槽。
//! - vmap v0x20：条目长 0x22，(mode, link) + u32 min/max，µV 直存。
//! - boost-ladder v0x10（P+0x34；MBT "Boost Table" 页的 79 点 GPU Boost 2.0
//!   阶梯，经 ILSpy 反编译 MBT 的 `_E016` 模型 + 双 ROM 逐位验证）：头 6 字节
//!   (ver, hdr, mark_len, mark_cnt, entry_len, entry_cnt)。mark 记录
//!   mark_len(=8)B × mark_cnt(=6)：u16 pstate 编码（>>5 位域，同 boost 表
//!   f0）+ byte@+3 = 阶梯索引（pstate 边界点）。entry 记录 entry_len(=5)B ×
//!   entry_cnt(=79)：u16 半 MHz 频率 + byte@+4 = **vmap 电压索引**——阶梯点
//!   的电压 = vmap[idx]，即完整 V/F 曲线就存在 vBIOS 中。
//!
//! RE 过程：MBT 为 .NET+Babel 混淆程序集，ILSpy 反编译见
//! `~/ida-scratch/mbt-decomp/`（2026-09-07）。

use super::error::Error;

/// perf v0x40 子条目域序（与 MBT Clock States 页、nvoc 域命名一致）。
pub const PERF_DOMAINS: [&str; 9] = [
    "GPC", "XBAR", "L2C", "DDR", "SYS", "HUB", "MSD", "PWR", "DISP",
];

/// boost 子条目 domain 字节的观测语义。
pub fn boost_domain_name(domain: u8) -> &'static str {
    match domain {
        1 => "XBAR",
        2 => "L2C",
        4 => "GPC",
        0x10 => "-",
        _ => "?",
    }
}

/// P-state 原始编码的显示名。观测映射 15→P0 13→P2 10→P5 7→P8（即
/// `P{15-raw}`）；raw 为 0（非 pstate 槽位，如 boost E4/E5）时返回 None。
pub fn pstate_display_name(pstate_raw: u8) -> Option<String> {
    (pstate_raw > 0).then(|| format!("P{}", 15 - pstate_raw))
}

/// perf 单条目：一个 P-state 的 9 域基础频率。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerfEntry {
    /// 原始 P-state 编码，见 [`pstate_display_name`]。
    pub pstate_raw: u8,
    /// 按 [`PERF_DOMAINS`] 序的域频率（MHz）。
    pub freq_mhz: [u16; 9],
}

impl PerfEntry {
    pub fn name(&self) -> Option<String> {
        pstate_display_name(self.pstate_raw)
    }
}

/// boost 单条目的域范围子条目。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoostDomain {
    pub domain: u8,
    pub percent: u8,
    /// 半 MHz 单位（×0.5 = MHz）。
    pub min_mhz_x2: u16,
    /// 半 MHz 单位。
    pub max_mhz_x2: u16,
}

/// boost 单条目：一个 P-state 的 GPC 提升范围与各域限制。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoostEntry {
    /// 原始 P-state 编码（(f0 & 0x1e0) >> 5）。
    pub pstate_raw: u8,
    /// 条目级 GPC min（半 MHz 单位）。
    pub min_mhz_x2: u16,
    /// 条目级 GPC max（半 MHz 单位）。
    pub max_mhz_x2: u16,
    pub domains: Vec<BoostDomain>,
}

impl BoostEntry {
    pub fn name(&self) -> Option<String> {
        pstate_display_name(self.pstate_raw)
    }

    /// 半 MHz 值转 MHz 浮点。
    pub fn mhz(mhz_x2: u16) -> f64 {
        f64::from(mhz_x2) / 2.0
    }
}

/// vmap 单条目：电压映射节点（µV 直存）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmapEntry {
    pub mode: u8,
    pub link: u8,
    pub min_uv: u32,
    pub max_uv: u32,
}

/// boost-ladder（GPU Boost 2.0 阶梯）的 pstate 边界标记。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LadderMark {
    /// 原始 u16 编码（pstate 位域同 boost 表 f0）。
    pub code_raw: u16,
    /// (code_raw & 0x1e0) >> 5。
    pub pstate_raw: u8,
    /// 该 pstate 边界所在的阶梯索引。
    pub ladder_index: u8,
}

/// 阶梯点：GPC 频率 + vmap 电压索引。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LadderEntry {
    /// 半 MHz 单位（×0.5 = MHz）。
    pub freq_mhz_x2: u16,
    /// 电压取自 [`LegacyVbios::vmap`] 的这个下标。
    pub vmap_index: u8,
}

/// boost-ladder v0x10 表：GPU Boost 2.0 的完整 GPC V/F 点阶梯。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoostLadder {
    pub ver: u8,
    pub marks: Vec<LadderMark>,
    pub entries: Vec<LadderEntry>,
}

/// 解析结果：三表 + 非致命告警（表版本不识别等降级为 warning）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyVbios {
    pub perf: Vec<PerfEntry>,
    pub boost: Vec<BoostEntry>,
    pub vmap: Vec<VmapEntry>,
    /// P+0x34 的 boost-ladder v0x10 表（Maxwell 观测必有；其它代际可能缺）。
    pub boost_ladder: Option<BoostLadder>,
    pub warnings: Vec<String>,
}

impl LegacyVbios {
    /// 阶梯点电压：(vmap min, max) µV。索引越界返回 None。
    pub fn ladder_voltage_uv(&self, vmap_index: u8) -> Option<(u32, u32)> {
        self.vmap
            .get(usize::from(vmap_index))
            .map(|v| (v.min_uv, v.max_uv))
    }
}

const BIT_SIG: [u8; 5] = [0xFF, 0xB8, b'B', b'I', b'T'];
/// 观测于 GM200/GM204 的已知 BIT 条目 id（'2' 'A'..'V' 'Z' 'i'）。
const KNOWN_BIT_IDS: [u8; 16] = [
    b'2', b'A', b'B', b'C', b'D', b'I', b'L', b'M', b'N', b'P', b'S', b'T', b'U', b'V', b'Z', b'i',
];

struct Reader<'a> {
    data: &'a [u8],
}

impl Reader<'_> {
    fn u8(&self, a: usize) -> Result<u8, Error> {
        self.data
            .get(a)
            .copied()
            .ok_or_else(|| Error::from("legacy vbios: offset out of bounds"))
    }

    fn u16(&self, a: usize) -> Result<u16, Error> {
        Ok(u16::from(self.u8(a)?) | (u16::from(self.u8(a + 1)?) << 8))
    }

    fn u32(&self, a: usize) -> Result<u32, Error> {
        Ok(u32::from(self.u16(a)?) | (u32::from(self.u16(a + 2)?) << 16))
    }
}

/// 镜像基址 = 第一个 `55 AA` boot-sector magic 的位置（KeplerBiosTweaker
/// 的 loader 语义——BIT 签名从 base 起搜，BIT 条目 offset 与 P 表内全部
/// u32 指针均为 base 相对；三样本实测：GM200 base=0x0、GK104 208576.rom
/// base=0x600、GM204 Palit base=0x800）。
pub fn find_image_base(data: &[u8]) -> Option<usize> {
    data.windows(2).position(|w| w == [0x55, 0xAA])
}

/// 从 base 起定位第一个 BIT 签名。
pub fn find_bit_signature(data: &[u8], base: usize) -> Option<usize> {
    data.get(base..)?
        .windows(BIT_SIG.len())
        .position(|w| w == BIT_SIG)
        .map(|p| base + p)
}

/// BIT 目录条目。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitEntry {
    pub id: u8,
    pub ver: u8,
    pub len: u16,
    /// 镜像相对偏移。
    pub offset: u16,
}

/// 扫描 BIT 目录。目录区可能含 len=0 的占位条目（如 'N' v0），故逐窗口
/// 收集全部形状合法的条目而非要求连续。`img` = base 相对寻址的基。
pub fn bit_entries(data: &[u8], sig: usize, img: usize) -> Result<Vec<BitEntry>, Error> {
    let r = Reader { data };
    let mut out = Vec::new();
    for a in sig + 5..sig + 0x80 {
        let id = r.u8(a)?;
        let ver = r.u8(a + 1)?;
        let len = r.u16(a + 2)?;
        let offset = r.u16(a + 4)?;
        let plausible = KNOWN_BIT_IDS.contains(&id)
            && ver <= 2
            && offset >= 0x40
            && len <= 0x1000
            && img + usize::from(offset) + usize::from(len) <= data.len();
        if plausible {
            out.push(BitEntry {
                id,
                ver,
                len,
                offset,
            });
        }
    }
    Ok(out)
}

fn parse_perf(r: &Reader, t: usize, warnings: &mut Vec<String>) -> Vec<PerfEntry> {
    let ver = match r.u8(t) {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!("perf: unreadable header: {e}"));
            return Vec::new();
        }
    };
    if ver != 0x40 {
        warnings.push(format!("perf: unsupported version {ver:#x}, skipped"));
        return Vec::new();
    }
    let hdr = r.u8(t + 1).unwrap_or(0);
    let len = r.u8(t + 2).unwrap_or(0);
    let ssz = r.u8(t + 3).unwrap_or(0);
    let snr = r.u8(t + 4).unwrap_or(0);
    let cnt = r.u8(t + 5).unwrap_or(0);
    if snr < 9 {
        warnings.push(format!("perf: snr={snr} < 9 domains, skipped"));
        return Vec::new();
    }
    let stride = usize::from(len) + usize::from(snr) * usize::from(ssz);
    let mut out = Vec::new();
    for e in 0..usize::from(cnt) {
        let base = t + usize::from(hdr) + e * stride;
        let sub0 = base + usize::from(len);
        if sub0 + 9 * usize::from(ssz) > r.data.len() {
            warnings.push(format!("perf: entry {e} out of bounds, truncated"));
            break;
        }
        let mut freq_mhz = [0u16; 9];
        for (s, freq) in freq_mhz.iter_mut().enumerate() {
            *freq = r.u16(sub0 + s * usize::from(ssz)).unwrap_or(0) & 0x3FFF;
        }
        out.push(PerfEntry {
            pstate_raw: r.u8(base).unwrap_or(0),
            freq_mhz,
        });
    }
    out
}

fn parse_boost(r: &Reader, t: usize, warnings: &mut Vec<String>) -> Vec<BoostEntry> {
    let ver = match r.u8(t) {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!("boost: unreadable header: {e}"));
            return Vec::new();
        }
    };
    if ver != 0x11 {
        warnings.push(format!("boost: unsupported version {ver:#x}, skipped"));
        return Vec::new();
    }
    let hdr = r.u8(t + 1).unwrap_or(0);
    let len = r.u8(t + 2).unwrap_or(0);
    let ssz = r.u8(t + 3).unwrap_or(0);
    let snr = r.u8(t + 4).unwrap_or(0);
    let cnt = r.u8(t + 5).unwrap_or(0);
    let stride = usize::from(len) + usize::from(snr) * usize::from(ssz);
    let mut out = Vec::new();
    for e in 0..usize::from(cnt) {
        let base = t + usize::from(hdr) + e * stride;
        if base + stride > r.data.len() {
            warnings.push(format!("boost: entry {e} out of bounds, truncated"));
            break;
        }
        let f0 = r.u16(base).unwrap_or(0);
        let mut domains = Vec::new();
        for s in 0..snr {
            let sub = base + usize::from(len) + usize::from(s) * usize::from(ssz);
            if sub + 6 > r.data.len() {
                continue;
            }
            let domain = r.u8(sub).unwrap_or(0);
            let min = r.u16(sub + 2).unwrap_or(0);
            let max = r.u16(sub + 4).unwrap_or(0);
            // 0x10 = 空槽（GM200/GM204 观测）；domain 0 且范围全零视为 padding。
            if domain == 0x10 || (domain == 0 && min == 0 && max == 0) {
                continue;
            }
            domains.push(BoostDomain {
                domain,
                percent: r.u8(sub + 1).unwrap_or(0),
                min_mhz_x2: min,
                max_mhz_x2: max,
            });
        }
        out.push(BoostEntry {
            pstate_raw: pstate_raw_from_f0(f0),
            min_mhz_x2: r.u16(base + 2).unwrap_or(0),
            max_mhz_x2: r.u16(base + 4).unwrap_or(0),
            domains,
        });
    }
    out
}

fn parse_vmap(r: &Reader, t: usize, warnings: &mut Vec<String>) -> Vec<VmapEntry> {
    let ver = match r.u8(t) {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!("vmap: unreadable header: {e}"));
            return Vec::new();
        }
    };
    if ver != 0x20 {
        warnings.push(format!("vmap: unsupported version {ver:#x}, skipped"));
        return Vec::new();
    }
    let hdr = r.u8(t + 1).unwrap_or(0);
    let len = r.u8(t + 2).unwrap_or(0);
    let cnt = r.u8(t + 3).unwrap_or(0);
    let mut out = Vec::new();
    for i in 0..usize::from(cnt) {
        let base = t + usize::from(hdr) + i * usize::from(len);
        if base + usize::from(len) > r.data.len() {
            warnings.push(format!("vmap: entry {i} out of bounds, truncated"));
            break;
        }
        out.push(VmapEntry {
            mode: r.u8(base).unwrap_or(0),
            link: r.u8(base + 1).unwrap_or(0),
            min_uv: r.u32(base + 2).unwrap_or(0),
            max_uv: r.u32(base + 6).unwrap_or(0),
        });
    }
    out
}

// cast note: (f0 & 0x1E0) >> 5 is a 5-bit field; truncation impossible.
#[allow(clippy::cast_possible_truncation)]
fn pstate_raw_from_f0(f0: u16) -> u8 {
    ((f0 & 0x1E0) >> 5) as u8
}

/// boost-ladder v0x10 @ P+0x34：mark 8B×N + 阶梯 5B×N（见模块文档）。
fn parse_boost_ladder(r: &Reader, t: usize, warnings: &mut Vec<String>) -> Option<BoostLadder> {
    let ver = match r.u8(t) {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!("boost-ladder: unreadable header: {e}"));
            return None;
        }
    };
    if ver != 0x10 {
        warnings.push(format!(
            "boost-ladder: unsupported version {ver:#x}, skipped"
        ));
        return None;
    }
    let hdr = r.u8(t + 1).unwrap_or(0);
    let mark_len = r.u8(t + 2).unwrap_or(0);
    let mark_cnt = r.u8(t + 3).unwrap_or(0);
    let entry_len = r.u8(t + 4).unwrap_or(0);
    let entry_cnt = r.u8(t + 5).unwrap_or(0);

    let mut marks = Vec::new();
    for i in 0..usize::from(mark_cnt) {
        let base = t + usize::from(hdr) + i * usize::from(mark_len);
        if base + usize::from(mark_len) > r.data.len() {
            warnings.push(format!("boost-ladder: mark {i} out of bounds, truncated"));
            break;
        }
        let code_raw = r.u16(base).unwrap_or(0);
        marks.push(LadderMark {
            code_raw,
            pstate_raw: pstate_raw_from_f0(code_raw),
            ladder_index: r.u8(base + 3).unwrap_or(0),
        });
    }

    let entries_base = t + usize::from(hdr) + usize::from(mark_cnt) * usize::from(mark_len);
    let mut entries = Vec::new();
    for i in 0..usize::from(entry_cnt) {
        let base = entries_base + i * usize::from(entry_len);
        if base + usize::from(entry_len) > r.data.len() {
            warnings.push(format!("boost-ladder: entry {i} out of bounds, truncated"));
            break;
        }
        entries.push(LadderEntry {
            freq_mhz_x2: r.u16(base).unwrap_or(0),
            vmap_index: r.u8(base + 4).unwrap_or(0),
        });
    }

    Some(BoostLadder {
        ver,
        marks,
        entries,
    })
}

/// 解析 legacy vBIOS 镜像（完整 ROM 或已裁剪的镜像数据）。
pub fn parse(data: &[u8]) -> Result<LegacyVbios, Error> {
    let img =
        find_image_base(data).ok_or_else(|| Error::from("legacy vbios: 55 AA base not found"))?;
    let r = Reader { data };
    let sig = find_bit_signature(data, img)
        .ok_or_else(|| Error::from("legacy vbios: BIT signature not found"))?;
    let entries = bit_entries(data, sig, img)?;
    let p = entries
        .iter()
        .find(|e| e.id == b'P' && e.ver == 2)
        .ok_or_else(|| Error::from("legacy vbios: BIT 'P' v2 entry not found"))?;
    let ptab = img + usize::from(p.offset);
    let mut warnings = Vec::new();

    fn table_addr(
        r: &Reader,
        img: usize,
        ptab: usize,
        off: usize,
        name: &str,
        warnings: &mut Vec<String>,
    ) -> Result<usize, Error> {
        let ptr = r.u32(ptab + off)?;
        if ptr == 0 {
            warnings.push(format!("{name}: no pointer in P table"));
            return Ok(r.data.len());
        }
        let t = img + usize::try_from(ptr).map_err(|_| Error::from("legacy vbios: bad pointer"))?;
        if t >= r.data.len() {
            warnings.push(format!("{name}: pointer {ptr:#x} out of bounds"));
        }
        Ok(t)
    }

    let perf_t = table_addr(&r, img, ptab, 0x00, "perf", &mut warnings)?;
    let vmap_t = table_addr(&r, img, ptab, 0x20, "vmap", &mut warnings)?;
    let boost_t = table_addr(&r, img, ptab, 0x30, "boost", &mut warnings)?;
    let ladder_t = table_addr(&r, img, ptab, 0x34, "boost-ladder", &mut warnings)?;
    let perf = parse_perf(&r, perf_t, &mut warnings);
    let vmap = parse_vmap(&r, vmap_t, &mut warnings);
    let boost = parse_boost(&r, boost_t, &mut warnings);
    let boost_ladder = (ladder_t < r.data.len())
        .then(|| parse_boost_ladder(&r, ladder_t, &mut warnings))
        .flatten();
    Ok(LegacyVbios {
        perf,
        vmap,
        boost,
        boost_ladder,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个最小合成 vBIOS：BIT 目录 + P 表 + perf/boost/vmap，
    /// 数值取自 GM200 实测解码结果。
    fn synthetic() -> Vec<u8> {
        let mut d = vec![0u8; 0x1000];
        let put = |d: &mut Vec<u8>, a: usize, b: &[u8]| d[a..a + b.len()].copy_from_slice(b);
        let u16le = |v: u16| v.to_le_bytes();
        let u32le = |v: u32| v.to_le_bytes();

        // 镜像基址 = 第一个 55 AA（find_image_base 语义）
        put(&mut d, 0, &[0x55, 0xAA]);
        put(&mut d, 0x1D0, &BIT_SIG);
        // 目录：'P' v2 len 0x68 @ 0x2d9（镜像相对）
        put(&mut d, 0x1DC + 6 * 8, &[b'P', 2]);
        put(&mut d, 0x1DC + 6 * 8 + 2, &u16le(0x68));
        put(&mut d, 0x1DC + 6 * 8 + 4, &u16le(0x2D9));

        // P 表 @ 0x2d9：perf @ 0x600, vmap @ 0x700, boost @ 0x6b8, ladder @ 0x780
        put(&mut d, 0x2D9, &u32le(0x600));
        put(&mut d, 0x2D9 + 0x20, &u32le(0x700));
        put(&mut d, 0x2D9 + 0x30, &u32le(0x6B8));
        put(&mut d, 0x2D9 + 0x34, &u32le(0x780));

        // perf v0x40 @ 0x600：hdr 0x23, len 0x20, ssz 4, snr 9, cnt 1
        put(&mut d, 0x600, &[0x40, 0x23, 0x20, 0x04, 0x09, 0x01]);
        let pe = 0x600 + 0x23;
        put(&mut d, pe, &[15]); // P0
        for (s, mhz) in [
            0x04a6u16, 0x0438, 0x0438, 0x0ed8, 0x0438, 0x0438, 0x021c, 0x0144, 0x021c,
        ]
        .iter()
        .enumerate()
        {
            put(&mut d, pe + 0x20 + s * 4, &u16le(*mhz));
        }

        // boost v0x11 @ 0x6b8：hdr 6, len 6, ssz 6, snr 9, cnt 1（E3 实测字节）
        put(&mut d, 0x6B8, &[0x11, 0x06, 0x06, 0x06, 0x09, 0x01]);
        let be = 0x6B8 + 6;
        put(&mut d, be, &u16le(0x01E0));
        put(&mut d, be + 2, &u16le(0x04A6)); // 595.0 MHz
        put(&mut d, be + 4, &u16le(0x0B2D)); // 1430.5 MHz
        for (s, (dom, pct)) in [(4u8, 92u8), (1, 90), (2, 88)].iter().enumerate() {
            let sub = be + 6 + s * 6;
            put(&mut d, sub, &[*dom, *pct]);
            let (mn, mx) = (0x0438u16, 0x0B2D);
            put(&mut d, sub + 2, &u16le(mn));
            put(&mut d, sub + 4, &u16le(mx));
        }

        // vmap v0x20 @ 0x700：hdr 0x0f, len 0x22, cnt 1
        put(&mut d, 0x700, &[0x20, 0x0F, 0x22, 0x01]);
        put(&mut d, 0x70F, &[0]);
        put(&mut d, 0x70F + 2, &u32le(725_000));
        put(&mut d, 0x70F + 6, &u32le(800_000));

        // boost-ladder v0x10 @ 0x780：ver 0x10, hdr 9, mark_len 8, mark_cnt 1,
        // entry_len 5, entry_cnt 2（形状照抄 GM200 实测，数值缩短）
        put(&mut d, 0x780, &[0x10, 0x09, 0x08, 0x01, 0x05, 0x02]);
        put(&mut d, 0x789, &u16le(0x01E0)); // P0 编码
        put(&mut d, 0x789 + 3, &[0x01]); // 边界在阶梯 idx 1
        put(&mut d, 0x791, &u16le(0x04A6)); // 595.0 MHz
        put(&mut d, 0x791 + 4, &[0]); // vmap[0]
        put(&mut d, 0x796, &u16le(0x0B2D)); // 1430.5 MHz
        put(&mut d, 0x796 + 4, &[0]);
        d
    }

    #[test]
    fn synthetic_full_parse() {
        let d = synthetic();
        let vb = parse(&d).expect("parse");
        assert_eq!(vb.warnings, Vec::<String>::new());

        assert_eq!(vb.perf.len(), 1);
        let p = vb.perf[0];
        assert_eq!(p.pstate_raw, 15);
        assert_eq!(p.name().as_deref(), Some("P0"));
        assert_eq!(
            p.freq_mhz,
            [1190, 1080, 1080, 3800, 1080, 1080, 540, 324, 540]
        );
        assert_eq!(p.freq_mhz[0], 1190); // GPC

        assert_eq!(vb.boost.len(), 1);
        let b = &vb.boost[0];
        assert_eq!(b.pstate_raw, 15);
        assert_eq!(BoostEntry::mhz(b.min_mhz_x2), 595.0);
        assert_eq!(BoostEntry::mhz(b.max_mhz_x2), 1430.5);
        assert_eq!(b.domains.len(), 3);
        assert_eq!(boost_domain_name(b.domains[0].domain), "GPC");
        assert_eq!(b.domains[0].percent, 92);
        assert_eq!(BoostEntry::mhz(b.domains[0].min_mhz_x2), 540.0);

        assert_eq!(vb.vmap.len(), 1);
        assert_eq!(vb.vmap[0].min_uv, 725_000);
        assert_eq!(vb.vmap[0].max_uv, 800_000);

        let ladder = vb.boost_ladder.as_ref().expect("ladder table");
        assert_eq!(ladder.ver, 0x10);
        assert_eq!(ladder.marks.len(), 1);
        assert_eq!(ladder.marks[0].pstate_raw, 15);
        assert_eq!(ladder.marks[0].ladder_index, 1);
        assert_eq!(ladder.entries.len(), 2);
        assert_eq!(ladder.entries[0].freq_mhz_x2, 0x04A6);
        assert_eq!(ladder.entries[1].freq_mhz_x2, 0x0B2D);
        assert_eq!(vb.ladder_voltage_uv(0), Some((725_000, 800_000)));
        assert_eq!(vb.ladder_voltage_uv(9), None);
    }

    #[test]
    fn rejects_missing_or_corrupt() {
        assert!(parse(&vec![0u8; 0x400]).is_err(), "no BIT sig");
        let mut d = synthetic();
        // 破坏 'P' 条目 id
        let pdir = 0x1DC + 6 * 8;
        d[pdir] = b'X';
        assert!(parse(&d).is_err(), "no P entry");
    }

    #[test]
    fn unsupported_table_versions_become_warnings() {
        let mut d = synthetic();
        d[0x6B8] = 0x12; // boost 未知版本
        let vb = parse(&d).expect("parse");
        assert!(vb.boost.is_empty());
        assert!(vb.warnings.iter().any(|w| w.contains("boost")));
    }

    /// 机会性真文件测试：仓库工作树若存在 reverse/ 下的实测 ROM 则全量解析。
    #[test]
    fn real_roms_when_present() {
        // (path, gpc_p0, ddr_p0, boost_max_x2, ladder_idx6_x2, ladder_cnt,
        //  p8_mark_idx, p0_mark_idx) — 数值来自 MBT/KBT UI 与 Python 解码实测
        for (
            path,
            want_gpc_p0,
            want_ddr_p0,
            want_boost_max_x2,
            want_ladder_x2,
            want_cnt,
            want_p8_idx,
            want_p0_idx,
        ) in [
            (
                "../reverse/GM200.bin",
                1190u16,
                3800u16,
                0x0B2Du16,
                1190u16,
                79usize,
                5u8,
                74u8,
            ),
            (
                "../reverse/Palit.GTX980.4096.141009.rom",
                1080,
                3505,
                0x0AE1,
                1064,
                79,
                5,
                74,
            ),
            // GK104：KBT 1.27 "Boost Table" 页 UI 逐位对照（idx0=324.0、
            // idx6=614.5、idx51=1202.0 绿行；P8@0..P0@51，idx52=0.0 终止点
            // 被窗口排除）。perf P0：GPC=1098 / DDR=3500（7000 MT/s）。
            (
                "../reverse/208576.rom",
                1098,   // Clock States P0 GPC
                3500,   // P0 DDR
                0x0964, // boost P0 max = 1202.0 MHz
                1229,   // idx6 = 614.5 MHz
                53,     // 52 实点 + 0.0 终止
                0,
                51,
            ),
        ] {
            let Ok(d) = std::fs::read(path) else {
                eprintln!("skip: {path} not present");
                continue;
            };
            let vb = parse(&d).expect("parse real rom");
            let p0 = vb
                .perf
                .iter()
                .find(|p| p.pstate_raw == 15)
                .expect("P0 perf entry");
            assert_eq!(p0.freq_mhz[0], want_gpc_p0, "{path} GPC");
            assert_eq!(p0.freq_mhz[3], want_ddr_p0, "{path} DDR");
            let b3 = vb
                .boost
                .iter()
                .find(|b| b.pstate_raw == 15)
                .expect("P0 boost entry");
            assert_eq!(b3.max_mhz_x2, want_boost_max_x2, "{path} boost max");
            // 阶梯表：工具 UI 值逐位对照
            let ladder = vb.boost_ladder.as_ref().expect("ladder table");
            assert_eq!(ladder.entries.len(), want_cnt, "{path} ladder count");
            assert_eq!(ladder.entries[6].freq_mhz_x2, want_ladder_x2, "{path} idx6");
            let p8 = ladder
                .marks
                .iter()
                .find(|m| m.pstate_raw == 7)
                .expect("P8 mark");
            assert_eq!(p8.ladder_index, want_p8_idx, "{path} P8 boundary index");
            let p0m = ladder
                .marks
                .iter()
                .find(|m| m.pstate_raw == 15)
                .expect("P0 mark");
            assert_eq!(p0m.ladder_index, want_p0_idx, "{path} P0 boundary index");
        }
    }
}
