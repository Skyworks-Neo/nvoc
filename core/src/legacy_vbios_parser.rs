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
//! # Pascal+ Virtual P-State 表（VP 表）
//!
//! Pascal 起 BIT 'P' 表被 VP（Virtual P-State）表族取代（open-gpu-doc
//! BIOS-Information-Table：BIT_PERF_PTRS v2 的 "Virtual P-State Table
//! Pointer"）。VP 表不在 BIT 指针链上，只能模式扫描定位——语义来自
//! JadeRover Nvidia-vBIOS-Clock-Power-Tweaker（`reverse/Nvidia-vBIOS-Clock-
//! Power-Tweaker-main`）的逆向 + notebooktalk.net topic/3040，**待实机
//! ROM 逐位校准**（decode 输出保留 raw 值）：
//!
//! - 头 3 字节 `20 XX 01`：`XX` = 头长编码兼代际，0x10/0x12=Pascal、
//!   0x13=Turing、0x15=Ampere、0x17=Ada；Blackwell 无 VP 表。双镜像 ROM
//!   可命中 2 份。
//! - **两种变体**：消费级 Pascal 有 profile 数组（紧贴头、ID 0x07→0x0F
//!   向头递增）；GP100 服务器卡（P100 逐位实测）为**仅阶梯变体**——无
//!   profile 数组（回走 10 步无 0x07 → profiles 返回空），且 ladder 只有
//!   2 频点 × 2 副本（服务器卡无 GPU Boost 阶梯）、无 mem 字段。
//! - 布局：`[profiles × N][20 XX 01][0x0F][ladder 条目 × N][footers × N]
//!   [marker 00 00 10 0E][FF 填充][VP 段校验字节]`。
//! - profile（Pascal 57B / 其它 65B）：`+0` ID（观测 0x07..0x0F 连续，与
//!   pstate_raw 编码空间一致 → 0x07=P8 … 0x0F=P0，0xFF=空槽）；Pascal 字段
//!   偏移 [7,13,15,19,25] = limit1/limit2/mem_short/mem_long/limit3。
//! - ladder 条目 41B 步进，以 freq=0 终止：`[-1]=0x0F 分母` + `+0` u32 LE
//!   频率。**解码分世代**：Pascal = u32/32768（15 位小数定点）——CPR 新旧
//!   两代读法与保存路径共同验证：高 u16 = floor(MHz/2)（CPR 的
//!   "clock_value/2"），低 u16 = 15 位小数 frac（奇数 MHz → 0x8000，即 CPR
//!   误当"旗标"的 `±32768` 现象）；完整精度只在 u32。**Turing+（XX=0x13+
//!   ，XX 实为头长非代际）= 低 14 位直存 MHz**（raw 两份副本 bits[13:0]/
//!   bits[27:14]；2070 1410/1620 与活卡 VFP base/boost 精确一致、3090
//!   1695、4090 2520——"/32768" 旧读法在 Turing+ 上得一半频率）。
//! - mem：首条目 +8 的 u16 = MHz 直存，高两位 0x4000/0x8000 为旗标
//!   （&0x3FFF 剥离）；VP 点**不带电压**（Maxwell 阶梯每点带 vmap 索引，
//!   Pascal 的电压在 BIT 另表，故电压编辑在 CPR 中"不可行"）。
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
const KNOWN_BIT_IDS: [u8; 27] = [
    b'2', b'A', b'B', b'C', b'D', b'I', b'L', b'M', b'N', b'P', b'S', b'T', b'U', b'V', b'Z', b'i',
    // 下半区 token（kepler-ada hexpat 的 26-token 枚举补全）：'p'=Falcon
    // ucode 表、'n'=DCB ptrs、'u'=UEFI、'x'=MXM 等。
    b'c', b'd', b'p', b'u', b'x', b'R', b'b', b'm', b'n', b's', b'E',
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

/// boost-ladder v0x10 @ P+0x34：mark 8B/4B×N + 阶梯 5B×N（见模块文档）。
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
    // Record-shape hardening: only observed on Maxwell (hdr 9, mark 8×6,
    // entry 5×79), GeForce Kepler 780Ti (hdr 7, mark 8×4, entry 5×53) and
    // Quadro Kepler K4000 (hdr 6, mark 4×4, entry 5×63 — GK106 的 GPU Boost
    // 阶梯同样在此，16 个非零点 324.0..810.5 MHz，KeplerBiosTweaker 逐字节
    // 对照实证)。Other generations may keep a NON-ladder table at P+0x34
    // whose version byte happens to be 0x10 — reject anything off-shape
    // instead of parsing garbage (Pascal/GP104's P+0x34 pointer is 0 and
    // stays clean either way).
    if !matches!(mark_len, 4 | 8) || entry_len != 5 || !(1..=8).contains(&mark_cnt) || entry_cnt < 8
    {
        warnings.push(format!(
            "boost-ladder: off-shape header (mark_len={mark_len} \
             mark_cnt={mark_cnt} entry_len={entry_len} entry_cnt={entry_cnt}), \
             skipped"
        ));
        return None;
    }

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

// ── Pascal+ Virtual P-State（VP）表 ─────────────────────────────────────

/// VP 头 `20 XX 01` 的布局族。**`XX` 实为头长字节而非代际**——GA102 同芯
/// 不同 build 观测 0x13 与 0x15 并存、AD102 观测 0x15 与 0x17 并存
///（VBIOS_sample 40 ROM 实测）；仅 0x10/0x12 与 Pascal 绑定成立。0x13+
/// 统一按 Turing+ 布局处理（profile 65B、阶梯 MHz 低 14 位直存）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpGeneration {
    /// XX = 0x10/0x12（GP104/GP102/GP100/P104 实测）。
    Pascal,
    /// XX = 0x13/0x15/0x17（TU106/TU102/GA102/AD102 实测；头长 19/21/23B）。
    TuringPlus,
}

impl VpGeneration {
    fn from_header_len(len: u8) -> Option<Self> {
        match len {
            0x10 | 0x12 => Some(Self::Pascal),
            0x13 | 0x15 | 0x17 => Some(Self::TuringPlus),
            _ => None,
        }
    }

    /// 显示名。
    pub fn name(self) -> &'static str {
        match self {
            Self::Pascal => "Pascal",
            Self::TuringPlus => "Turing+",
        }
    }

    /// profile 记录长度（字节）。
    pub fn profile_len(self) -> usize {
        if self == Self::Pascal { 57 } else { 65 }
    }

    /// profile 内 5 字段偏移：limit1 / limit2 / mem_short / mem_long / limit3。
    fn profile_field_offsets(self) -> [usize; 5] {
        if self == Self::Pascal {
            [7, 13, 15, 19, 25]
        } else {
            // Turing+ 布局未经实机逐位校准（CPR 观测值）。
            [9, 15, 17, 21, 39]
        }
    }

    /// 阶梯条目频率解码：Pascal = u32 ÷ 2^15（15 位小数定点，
    /// GP104 1506.09/P100 1189.57 实证）；**Turing+ = 低 14 位直存 MHz**
    ///（raw 存两份 MHz：bits[13:0] 与 bits[27:14]；2070 1410/1620 与活卡
    /// VFP base/boost 精确一致、3090 1695、4090 2520——旧 "/32768" 读法
    /// 会得一半频率）。
    fn freq_mhz(self, raw: u32) -> f64 {
        match self {
            Self::Pascal => f64::from(raw) / 32768.0,
            Self::TuringPlus => f64::from(raw & 0x3FFF),
        }
    }
}

/// VP profile 一条：一个虚拟 P-state 档位的时钟上限与内存时钟。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VpProfile {
    /// profile ID（观测 0x07..0x0F，同 pstate_raw 编码 → 0x07=P8 … 0x0F=P0；
    /// 0xFF = 空槽）。
    pub id: u8,
    /// 绝对文件偏移（profile 起点，诊断/将来编辑用）。
    pub offset: usize,
    /// 三档 boost 上限原始 u16（Pascal：半 MHz 直存，×2 = MHz）。
    pub limit_raw: [u16; 3],
    /// mem clock 短副本原始 u16（MHz 直存）。
    pub mem_short_raw: u16,
    /// mem clock 长副本原始 u16（旗标位 0x4000/0x8000 + MHz）。
    pub mem_long_raw: u16,
}

impl VpProfile {
    /// 空槽（ID 0xFF）。
    pub fn is_empty(&self) -> bool {
        self.id == 0xFF
    }

    /// Pascal 语义的三档上限（MHz = raw × 2；单位经 CPR 保存路径
    /// `custom/2` 回写验证）。非 Pascal 布局未校准，返回 raw×2 仅供参考。
    pub fn limit_mhz(&self) -> [f64; 3] {
        self.limit_raw.map(|r| f64::from(r) * 2.0)
    }

    /// DRAM 频率（MHz）= (`mem_long_raw & 0x3FFF`) × 2（CPR "Mem clock"
    /// 列；+19 字段带旗标、存半频）。
    pub fn mem_clock_mhz(&self) -> u16 {
        (self.mem_long_raw & 0x3FFF) * 2
    }

    /// ×2 显示值（MHz）= `mem_short_raw & 0x3FFF`（CPR "Mem clock DDR" 列；
    /// +15 字段直存 ×2 值）。
    pub fn mem_clock_ddr_mhz(&self) -> u16 {
        self.mem_short_raw & 0x3FFF
    }
}

/// VP ladder 一个阶梯点（41B 条目的频率字段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VpLadderEntry {
    /// 绝对文件偏移（条目起始）。
    pub offset: usize,
    /// 条目前一字节的分母（观测 0x0F = 15 位小数声明；仅首条目保证）。
    pub denominator: u8,
    /// 条目起始 u32 LE。Pascal = MHz × 2^15（高 u16 = floor(MHz/2)，低
    /// u16 = frac）；**Turing+ = 低 14 位直存 MHz**（bit15 旗标 + MHz，
    /// 高 u16 为第二副本）。
    pub raw: u32,
    /// 所属布局族（决定 raw 解码）。
    pub generation: VpGeneration,
}

impl VpLadderEntry {
    /// 频率（MHz）：按世代解码（见 [`VpGeneration::freq_mhz`]）。
    pub fn freq_mhz(&self) -> f64 {
        self.generation.freq_mhz(self.raw)
    }
}

/// 一份 VP 表（双镜像 ROM 会解析出两份，内容应一致）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpTable {
    pub generation: VpGeneration,
    /// `20 XX 01` 头的绝对偏移。
    pub header_offset: usize,
    /// 阶梯起点（紧跟 0x0F 分母之后的首条目偏移）。
    pub ladder_offset: usize,
    /// profiles，从最后一个向前收集（顺序 = 文件顺序）。
    pub profiles: Vec<VpProfile>,
    pub entries: Vec<VpLadderEntry>,
    /// footers，顺序 = 文件顺序（CPR GUI 以回走序展示，两者互为倒序）。
    pub footers: Vec<VpFooter>,
    /// 首条目 +8 的内存频率原始 u16（旗标 + MHz）。
    pub mem_clock_raw: u16,
}

impl VpTable {
    /// 内存时钟（MHz，`&0x3FFF` 剥旗标）。
    pub fn mem_clock_mhz(&self) -> u16 {
        self.mem_clock_raw & 0x3FFF
    }

    /// 非空 profile 数。
    pub fn non_empty_profiles(&self) -> usize {
        self.profiles.iter().filter(|p| !p.is_empty()).count()
    }
}

const VP_ENTRY_STRIDE: usize = 41;
const VP_MAX_ENTRIES: usize = 128;
const VP_MAX_PROFILES: usize = 10;

/// 扫描全部 VP 表（模式定位，不依赖 BIT 目录）。双镜像 ROM 返回 2 份。
///
/// 判定三条件（CPR 同款）：`20 XX 01` 头、分母字节 0x0F、首条目按世代
/// 解码 ∈ (100, 2000) MHz（Pascal = /32768；Turing+ = &0x3FFF）。profiles/
/// ladder 停止条件异常时截断该表（不报错）。
pub fn find_vp_tables(data: &[u8]) -> Vec<VpTable> {
    let mut out = Vec::new();
    if data.len() < 4 {
        return out;
    }
    for header_offset in 0..data.len() - 2 {
        let [b0, b1, b2] = [
            data[header_offset],
            data[header_offset + 1],
            data[header_offset + 2],
        ];
        if b0 != 0x20 || b2 != 0x01 {
            continue;
        }
        let Some(generation) = VpGeneration::from_header_len(b1) else {
            continue;
        };
        // 阶梯起点 = 头后（头长 = XX+1 字节含 0x01），0x0F 分母在其前一字节。
        let ladder_offset = header_offset + usize::from(b1) + 1;
        let Some(table) = parse_vp_table(data, generation, header_offset, ladder_offset) else {
            continue;
        };
        out.push(table);
    }
    out
}

fn parse_vp_table(
    data: &[u8],
    generation: VpGeneration,
    header_offset: usize,
    ladder_offset: usize,
) -> Option<VpTable> {
    // 分母 + 首条目频率双验证（CPR 同款），过滤随机字节伪命中。
    if data.get(ladder_offset.wrapping_sub(1))? != &0x0F {
        return None;
    }
    let first_raw = u32::from(data[ladder_offset])
        | (u32::from(data[ladder_offset + 1]) << 8)
        | (u32::from(data[ladder_offset + 2]) << 16)
        | (u32::from(data[ladder_offset + 3]) << 24);
    let first_mhz = generation.freq_mhz(first_raw);
    // Pascal 定点钟上限 2000；Turing+ 整数钟直存（4090 boost 2520、
    // 5090 ~2900），上限放宽到 3500。
    let mhz_cap = if generation == VpGeneration::Pascal {
        2000.0
    } else {
        3500.0
    };
    if !(100.0..mhz_cap).contains(&first_mhz) {
        return None;
    }

    // ladder：41B 步进，freq=0 终止。
    let mut entries = Vec::new();
    for i in 0..VP_MAX_ENTRIES {
        let offset = ladder_offset + i * VP_ENTRY_STRIDE;
        let denominator = *data.get(offset.wrapping_sub(1))?;
        let raw = u32::from(data.get(offset).copied()?)
            | (u32::from(data.get(offset + 1).copied()?) << 8)
            | (u32::from(data.get(offset + 2).copied()?) << 16)
            | (u32::from(data.get(offset + 3).copied()?) << 24);
        let freq_mhz = generation.freq_mhz(raw);
        if raw == 0 || freq_mhz > 5000.0 {
            break;
        }
        entries.push(VpLadderEntry {
            offset,
            denominator,
            raw,
            generation,
        });
    }
    if entries.is_empty() {
        return None;
    }

    // profiles：从头位置向前按 profile_len 回走，ID 0x07 = 首条（停止条件）。
    // 10 步内未见 0x07 = 无 profile 数组的变体表（实测 GP100 服务器卡 P100：
    // 仅 ladder；消费级 Pascal 观测恒有 0x07 收尾）——返回空 profiles 而
    // 非垃圾（CPR 同场景会吐垃圾，其注释自认 "Might not be the case"）。
    let profile_len = generation.profile_len();
    let field_off = generation.profile_field_offsets();
    let mut walked = Vec::new();
    let mut saw_first = false;
    for i in 1..=VP_MAX_PROFILES {
        let offset = header_offset.checked_sub(i * profile_len)?;
        let id = *data.get(offset)?;
        let fields = |a: usize| -> u16 {
            let p = offset + a;
            u16::from(data.get(p).copied().unwrap_or(0))
                | (u16::from(data.get(p + 1).copied().unwrap_or(0)) << 8)
        };
        walked.push(VpProfile {
            id,
            offset,
            limit_raw: [
                fields(field_off[0]),
                fields(field_off[1]),
                fields(field_off[4]),
            ],
            mem_short_raw: fields(field_off[2]),
            mem_long_raw: fields(field_off[3]),
        });
        if id == 0x07 {
            saw_first = true;
            break;
        }
    }
    if !saw_first {
        walked.clear();
    }
    walked.reverse();

    // mem clock：首条目 +8（CPR：频率"含奇数"的 REAL 值）。
    let mem_base = ladder_offset + 8;
    let mem_clock_raw = u16::from(data.get(mem_base).copied().unwrap_or(0))
        | (u16::from(data.get(mem_base + 1).copied().unwrap_or(0)) << 8);

    let footers = parse_vp_footers(data, ladder_offset);

    Some(VpTable {
        generation,
        header_offset,
        ladder_offset,
        profiles: walked,
        entries,
        footers,
        mem_clock_raw,
    })
}

/// VP footer 一条（阶梯之后的内存频率数组；每个非空 profile 对应一到两条）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VpFooter {
    /// footer ID（0xFF = 空槽；其余同 profile ID 编码空间）。
    pub id: u8,
    /// footer 起始绝对偏移。
    pub offset: usize,
    /// +9 u16：×2 显示值（旗标 + 频率；`&0x3FFF` 剥旗标）。
    pub mem_x2_raw: u16,
    /// +11 u16：半频值（×2 = DRAM MHz）。
    pub mem_half_raw: u16,
}

impl VpFooter {
    /// 空槽（ID 0xFF）。
    pub fn is_empty(&self) -> bool {
        self.id == 0xFF
    }

    /// DRAM 频率（MHz）= `mem_half_raw × 2`（CPR "Mem clock" 列）。
    pub fn mem_clock_mhz(&self) -> u16 {
        self.mem_half_raw * 2
    }

    /// ×2 显示值（MHz）= `mem_x2_raw & 0x3FFF`（CPR "Mem clock DDR" 列）。
    pub fn mem_clock_ddr_mhz(&self) -> u16 {
        self.mem_x2_raw & 0x3FFF
    }
}

const VP_FOOTER_STRIDE: usize = 41;
const VP_FOOTER_MAX: usize = 20;

/// 从阶梯起点搜 `00 00 10 0E` marker，marker+2 起按 41B 回走收集 footers。
/// 停止条件（CPR 同款）：byte@+1 非零或 ID=0x07（均要求 i>3），或走满 20 条。
/// 返回文件序（与 CPR GUI 的回走展示序互为倒序）。
fn parse_vp_footers(data: &[u8], ladder_offset: usize) -> Vec<VpFooter> {
    let Some(marker) = data[ladder_offset.min(data.len())..]
        .windows(4)
        .position(|w| w == [0x00, 0x00, 0x10, 0x0E])
        .map(|p| ladder_offset + p)
    else {
        return Vec::new();
    };
    let start = marker + 2;
    let mut walked = Vec::new();
    for i in 1..=VP_FOOTER_MAX {
        let Some(offset) = start.checked_sub(i * VP_FOOTER_STRIDE) else {
            break;
        };
        let Some(id) = data.get(offset).copied() else {
            break;
        };
        let b1 = data.get(offset + 1).copied().unwrap_or(1);
        if b1 == 0 {
            let u16at = |a: usize| {
                u16::from(data.get(a).copied().unwrap_or(0))
                    | (u16::from(data.get(a + 1).copied().unwrap_or(0)) << 8)
            };
            walked.push(VpFooter {
                id,
                offset,
                mem_x2_raw: u16at(offset + 9),
                mem_half_raw: u16at(offset + 11),
            });
        }
        if i > 3 && (id == 0x07 || b1 != 0) || i == VP_FOOTER_MAX {
            break;
        }
    }
    walked.reverse();
    walked
}

// ── PERF_PTRS_V2 槽位图（modern u32 布局，Kepler-Ada ImHex pattern 逆向）──

/// BIT 'P'（PERF_PTRS）的 modern u32 槽位名表。布局来自
/// kepler-ada-nvidia-vbios-visualizer-2025_02.hexpat 的
/// `BIT_DATA_PERF_PTRS_V2`（45 槽到 +0xE8；`data_size >= 156/224/232/236`
/// 门控扩展段——阈值 = 条件块结束偏移，同 hexpat 约定）。
///
/// **指针需 EFI 间隙重定位**（PC 响应区之后的指针 + EFI 镜像长度，与
/// falcon 同规则）。GP104 实测：重定位后全部非零槽位落明文
/// （VirtualPState→0x2D643 = VP 扫描命中位，交叉验证；Fan/Thermal 槽位
/// 全部落 0x30xxx 明文）。Maxwell（len=104）旧布局槽位语义错位——
/// +0x10=nouveau 热表、+0x20=vmap、+0x30=boost v0x11、+0x34=boost-ladder，
/// 勿按 modern 名解读；其指针多在 PC 镜像内、无需重定位。
const PERF_PTR_SLOTS: [(usize, &str); 45] = [
    (0x00, "PerfTable"),
    (0x04, "MemClockTable"),
    (0x08, "MemTweakTable"),
    (0x0C, "PowerControl"),
    (0x10, "ThermalControl"),
    (0x14, "ThermalDevice"),
    (0x18, "ThermalCooler"),
    (0x1C, "SettingsScript"),
    (0x20, "VoltageDesc"),
    (0x24, "SpbSensorParam"),
    (0x28, "PowerSensors"),
    (0x2C, "PowerCapping"),
    (0x30, "PstateClkRange"),
    (0x34, "VoltageFreq"),
    (0x38, "VirtualPState"),
    (0x3C, "PowerTopology"),
    (0x40, "PowerEquation"),
    (0x44, "PerfTestSpec"),
    (0x48, "ThermalChannel"),
    (0x4C, "ThermalAdjustment"),
    (0x50, "ThermalPolicy"),
    (0x54, "PstateMclkFreq"),
    (0x58, "FanCooler"),
    (0x5C, "FanPolicy"),
    (0x60, "DIDT"),
    (0x64, "FanTest"),
    (0x68, "VoltageRail"),
    (0x6C, "VoltageDevice"),
    (0x70, "VoltagePolicy"),
    (0x74, "LpwrIdx"),
    (0x78, "LpwrPcie"),
    (0x7C, "LpwrPciePlatform"),
    (0x80, "LpwrGr"),
    (0x84, "LpwrMs"),
    (0x88, "LpwrDi"),
    (0x8C, "LpwrGc6"),
    (0x90, "LpwrPsi"),
    (0x94, "ThermalMonitor"),
    (0x98, "Overclocking"),
    (0x9C, "LpwrNvlink"),
    (0xA0, "PerfCfSensor"),
    (0xA4, "PerfCfTopology"),
    (0xA8, "PerfCfController"),
    (0xAC, "PerfCfPolicy"),
    (0xB0, "IllumDevice"),
];

/// 旧布局（Fermi..Maxwell，P token len < 0x9C）的槽位名覆盖。语义来自
/// KeplerBiosTweaker 逆向（_E027 ctor 的槽位分发：ptr[11]=Fan、
/// ptr[12]=BoostStates、ptr[13]=BoostLadder、ptr[14]=BaseBoost；K4000
/// 字节逐槽验证）+ GM200 记录（+0x20=vmap、+0x30=boost、+0x34=ladder）。
/// 未覆盖槽位保持 modern 名（前三槽 PerfTable/MemClockTable/MemTweakTable
/// 两代同名且 K4000 实证可解）。
const PERF_PTR_SLOTS_LEGACY: [(usize, &str); 5] = [
    (0x20, "VoltageMap"),
    (0x2C, "FanSettings"),
    (0x30, "BoostStates"),
    (0x34, "BoostLadder"),
    (0x38, "BaseBoost"),
];

// ── Clock States / Boost States（Kepler/Maxwell per-Pstate 时钟域）────────

/// perf 表 v0x40 的时钟域顺序（KeplerBiosTweaker 域名字典；K4000
/// P08/P05/P00 三态逐域字节验证——P08 GPC/XBAR/L2C/SYS/HUB=648、
/// DDR/PWR=324、MSD=405、DISP=540 与 KBT Clock States 页一致）。
pub const PERF_DOMAIN_NAMES: [&str; 9] = [
    "GPC", "XBAR", "L2C", "DDR", "SYS", "HUB", "MSD", "PWR", "DISP",
];

/// 一个 P-state 的各时钟域（Clock States）。频率 MHz 直存，值 = 域 u32
/// & 0xFFF——高 4 位为旗标（K4000 P00 实测 0x444A & 0xFFF = 1098 =
/// KBT UI 值；魔改卡旗标位被改写而值不变）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerfState {
    /// 条目绝对偏移。
    pub offset: usize,
    /// pstate 原始编码（byte@0：0x07=P8、0x0A=P5、0x0F=P0；0xFF=空槽已滤）。
    pub pstate_code: u8,
    /// byte@2：vmap 电压索引（vmap[vid] = 该态电压范围）。
    pub vmap_index: u8,
    /// 各域频率 MHz，顺序 = [`PERF_DOMAIN_NAMES`]。
    pub domains_mhz: Vec<u32>,
}

/// Boost States 一个域的 min/max（半 MHz×2 编码，同阶梯点）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoostStateRange {
    pub domain: &'static str,
    pub min_mhz_x2: u16,
    pub max_mhz_x2: u16,
}

/// Boost States 一个 P-state 组（K4000 实测 P00/P05/P08 三组，与 KBT
/// Boost States 页逐值一致：P00 GPC 1098/1621 = 549.0/810.5 MHz）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoostStateGroup {
    pub pstate_code: u8,
    pub ranges: Vec<BoostStateRange>,
}

/// BIT 'P' 表定位（img 基址 + ptab 绝对偏移）。
fn perf_ptab(data: &[u8], what: &str) -> Result<(usize, usize), Error> {
    let img = find_image_base(data)
        .ok_or_else(|| Error::from(format!("{what}: 55 AA base not found")))?;
    let sig = find_bit_signature(data, img)
        .ok_or_else(|| Error::from(format!("{what}: BIT signature not found")))?;
    let entries = bit_entries(data, sig, img)?;
    let p = entries
        .iter()
        .find(|e| e.id == b'P')
        .ok_or_else(|| Error::from(format!("{what}: BIT 'P' entry not found")))?;
    Ok((img, img + usize::from(p.offset)))
}

/// 解码 Clock States（perf 表 v0x40 @ P+0x00，Kepler/Maxwell 旧布局）。
/// 表头 `[ver=0x40][hdr][entry_len][cnt][dom_count][dom_size]`；条目 =
/// `[pstate_code u8][?][vmap_idx u8][?]…` + dom_count 个域值。Pascal+ perf
/// 为 ver 0x50/0x60 族 → None。
pub fn find_perf_states(data: &[u8]) -> Result<Option<Vec<PerfState>>, Error> {
    let (img, ptab) = perf_ptab(data, "clock states")?;
    let r = Reader { data };
    let ptr = r.u32(ptab)?;
    if ptr == 0 {
        return Ok(None);
    }
    let t = img + usize::try_from(ptr).map_err(|_| Error::from("clock states: bad pointer"))?;
    if t + 6 > data.len() || data[t] != 0x40 {
        return Ok(None);
    }
    let hdr = usize::from(r.u8(t + 1)?);
    let entry_len = usize::from(r.u8(t + 2)?);
    let cnt = usize::from(r.u8(t + 3)?);
    let dom_count = usize::from(r.u8(t + 4)?);
    let dom_size = usize::from(r.u8(t + 5)?);
    if dom_count == 0
        || dom_count > PERF_DOMAIN_NAMES.len()
        || !(2..=8).contains(&dom_size)
        || entry_len == 0
    {
        return Ok(None);
    }
    let stride = entry_len + dom_count * dom_size;
    let mut out = Vec::new();
    for i in 0..cnt {
        let e = t + hdr + i * stride;
        if e + stride > data.len() {
            break;
        }
        let code = data[e];
        if code == 0xFF {
            continue;
        }
        let domains = (0..dom_count)
            .map(|k| {
                let a = e + entry_len + k * dom_size;
                let raw = u32::from(data[a])
                    | u32::from(data.get(a + 1).copied().unwrap_or(0)) << 8
                    | u32::from(data.get(a + 2).copied().unwrap_or(0)) << 16
                    | u32::from(data.get(a + 3).copied().unwrap_or(0)) << 24;
                raw & 0xFFF
            })
            .collect();
        out.push(PerfState {
            offset: e,
            pstate_code: code,
            vmap_index: data[e + 2],
            domains_mhz: domains,
        });
    }
    if out.is_empty() {
        Ok(None)
    } else {
        Ok(Some(out))
    }
}

/// 解码 Boost States（P+0x30 槽，表头 ver 0x11）：hdr 字节后是
/// `(u16 tag, u16 min_x2, u16 max_x2)` 三元组流。GPC 三元组 tag =
/// pstate_code<<5（0xE0→P8、0x140→P5、0x1E0→P0）开启新组；其余域 tag 低
/// 字节 = 域 id（1=L2C、2=XBAR、4=SYS，高位随 pstate 变化忽略）。
/// min/max 双零 = 流结束。
pub fn find_boost_states(data: &[u8]) -> Result<Option<Vec<BoostStateGroup>>, Error> {
    let (img, ptab) = perf_ptab(data, "boost states")?;
    let r = Reader { data };
    let ptr = r.u32(ptab + 0x30)?;
    if ptr == 0 {
        return Ok(None);
    }
    let t = img + usize::try_from(ptr).map_err(|_| Error::from("boost states: bad pointer"))?;
    if t + 6 > data.len() || data[t] != 0x11 {
        return Ok(None);
    }
    let hdr = usize::from(r.u8(t + 1)?);
    let mut groups: Vec<BoostStateGroup> = Vec::new();
    let mut a = t + hdr;
    for _ in 0..64 {
        if a + 6 > data.len() {
            break;
        }
        let tag = r.u16(a)?;
        let min_x2 = r.u16(a + 2)?;
        let max_x2 = r.u16(a + 4)?;
        if min_x2 == 0 && max_x2 == 0 {
            break;
        }
        let ps = tag >> 5;
        if (tag & 0x1F) == 0 && (1..=15).contains(&ps) {
            groups.push(BoostStateGroup {
                pstate_code: ps as u8,
                ranges: vec![BoostStateRange {
                    domain: "GPC",
                    min_mhz_x2: min_x2,
                    max_mhz_x2: max_x2,
                }],
            });
        } else if let Some(g) = groups.last_mut() {
            let domain = match tag & 0xFF {
                1 => "L2C",
                2 => "XBAR",
                4 => "SYS",
                _ => "Unknown",
            };
            g.ranges.push(BoostStateRange {
                domain,
                min_mhz_x2: min_x2,
                max_mhz_x2: max_x2,
            });
        } else {
            break;
        }
        a += 6;
    }
    if groups.is_empty() {
        Ok(None)
    } else {
        Ok(Some(groups))
    }
}

/// 一个非零 PERF_PTR 槽位及其目标表头探测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerfPtrSlot {
    /// P 表内槽位偏移。
    pub slot_offset: usize,
    /// modern 布局槽位名（见 [`PERF_PTR_SLOTS`] 语义边界说明）。
    pub name: &'static str,
    /// P 表内 u32 原值（镜像相对）。
    pub target: u32,
    /// 绝对文件偏移（img + target）。
    pub abs_offset: usize,
    /// 目标处 `[ver, hdr, len, cnt]`（可读时）。
    pub header: Option<[u8; 4]>,
    /// 头部合理性粗判：ver ≤ 0x50 且 hdr ≤ 0x40 且 cnt ≤ 0x400 且
    /// target+4+hdr+cnt×len 不越界。false ≠ 无效（可能指向压缩区）。
    pub plausible: bool,
}

/// BIT 'P' 槽位图解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerfPtrMap {
    /// P 表绝对偏移。
    pub ptab_offset: usize,
    /// P token 的 data_size（决定槽位数量）。
    pub p_len: u16,
    /// 非零槽位（按偏移序）。
    pub slots: Vec<PerfPtrSlot>,
}

/// 解析 BIT 'P' 的 modern u32 槽位图（所有非零槽 + 目标表头探测）。
///
/// 与 [`find_thermal`] 共享定位路径（BIT P 目录），但读的是**槽位图**而
/// 非单个指针。Pascal 上扇/热槽位的目标通常在压缩区（`plausible: false`
/// 且 header 为乱字节）——这是数据结构只存在于 PMU/Falcon 侧的直接证据，
/// 非解析失败。
pub fn find_perf_ptr_map(data: &[u8]) -> Result<PerfPtrMap, Error> {
    let img =
        find_image_base(data).ok_or_else(|| Error::from("perf ptrs: 55 AA base not found"))?;
    let r = Reader { data };
    let sig = find_bit_signature(data, img)
        .ok_or_else(|| Error::from("perf ptrs: BIT signature not found"))?;
    let entries = bit_entries(data, sig, img)?;
    let p = entries
        .iter()
        .find(|e| e.id == b'P')
        .ok_or_else(|| Error::from("perf ptrs: BIT 'P' entry not found"))?;
    let ptab = img + usize::from(p.offset);
    let p_len = p.len;
    let (pc_end, gap) = efi_gap_after_pc(data, img);
    // Pre-Pascal（旧 22/26 槽布局）用 legacy 槽名覆盖——0x20 起语义完全
    // 不同（modern 名会把 vmap 标成 VoltageDesc、阶梯标成 VoltageFreq）。
    let legacy_layout = p_len < 0x9C;
    let mut slots = Vec::new();
    for &(off, name) in &PERF_PTR_SLOTS {
        if off + 4 > usize::from(p_len) {
            break;
        }
        let name = if legacy_layout {
            PERF_PTR_SLOTS_LEGACY
                .iter()
                .find(|(o, _)| *o == off)
                .map_or(name, |(_, n)| *n)
        } else {
            name
        };
        let target = r.u32(ptab + off)?;
        if target == 0 {
            continue;
        }
        let Ok(rel) = usize::try_from(target) else {
            continue;
        };
        // EFI 间隙重定位（与 falcon 同规则，hexpat section1_address 语义）：
        // 越过 PC 镜像末尾的指针 + EFI 镜像长度。GP104 实测 21 槽全部落明文
        // （VirtualPState 重定位后 = VP 扫描命中的 0x2D643，交叉验证）。
        let raw_abs = img + rel;
        let abs_offset = if raw_abs > pc_end.saturating_sub(1) {
            raw_abs + gap
        } else {
            raw_abs
        };
        let mut slot = PerfPtrSlot {
            slot_offset: off,
            name,
            target,
            abs_offset,
            header: None,
            plausible: false,
        };
        if let Some(h) = data.get(abs_offset..abs_offset + 4) {
            let (ver, hdr, len, cnt) = (h[0], h[1], h[2], h[3]);
            slot.header = Some([ver, hdr, len, cnt]);
            slot.plausible = ver <= 0x50
                && hdr <= 0x40
                && usize::from(cnt) <= 0x400
                && abs_offset + 4 + usize::from(hdr) + usize::from(cnt) * usize::from(len)
                    <= data.len();
        }
        slots.push(slot);
    }
    Ok(PerfPtrMap {
        ptab_offset: ptab,
        p_len,
        slots,
    })
}

// ── Falcon ucode 表（BIT 'p' v2/len4；kepler-ada hexpat 完整布局）────────

/// Falcon ucode 应用 ID（hexpat `UCodeApplicationID`）。
pub fn falcon_app_name(app: u8) -> Option<&'static str> {
    Some(match app {
        0x01 => "PRE_OS",
        0x02 => "GC6_DEVINIT_ENGINE",
        0x03 => "GC6_DEVINIT_COMPACTION",
        0x04 => "PRIMARY_DEVINIT_ENGINE",
        0x05 => "FIRMWARE_SEC_LIC",
        0x08 => "LS_UDE",
        0x09 => "HULK",
        0x13 => "DEVINIT_FMC",
        0x14 => "POSTLTSSM",
        _ => return None,
    })
}

/// Falcon ucode 目标 ID（hexpat `UCodeTargetID`）。
pub fn falcon_target_name(target: u8) -> Option<&'static str> {
    Some(match target {
        0x01 => "PMU",
        0x02 => "DPU",
        0x03 => "FECS",
        _ => return None,
    })
}

/// Falcon ucode 描述符（`FALCON_UCODE_DESC_V1` 头部字段；IMEM/DMEM blob
/// 不提取——那是加密/压缩载荷，RE 属 falcon 工具链范畴）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FalconUcodeDesc {
    /// 首字节 bit0：是否带 4 字节 version/crypt 头。
    pub has_version_crypt: bool,
    /// version/crypt 头里的版本（无头时 0）。
    pub version: u8,
    pub stored_size: u32,
    pub uncompressed_size: u32,
    /// 代码入口点。
    pub virtual_entry: u32,
    pub imem_phys_base: u32,
    pub imem_load_size: u32,
    pub dmem_phys_base: u32,
    pub dmem_load_size: u32,
}

/// Falcon ucode 表一条目（`FALCON_UCODE_TABLE_ENTRY_V1`，6B）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FalconUcodeEntry {
    pub application_id: u8,
    pub target_id: u8,
    pub application: Option<&'static str>,
    pub target: Option<&'static str>,
    /// 描述符指针（表内相对，见 [`FalconTable::table_offset`]）。
    pub desc_ptr: u32,
    /// DescPtr 非零时的描述符头。
    pub desc: Option<FalconUcodeDesc>,
}

/// Falcon ucode 表（PMU/devinit/安全许可证固件清单）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FalconTable {
    /// 表绝对偏移（已经过 EFI 间隙重定位）。
    pub table_offset: usize,
    /// 原始指针值（镜像相对）。
    pub raw_ptr: u32,
    pub desc_version: u8,
    pub desc_size: u8,
    pub entries: Vec<FalconUcodeEntry>,
}

/// 计算 PC 镜像之后的 EFI 间隙（hexpat `adjust_skip_efi` 语义）：
/// PC 响应区后若紧跟 EFI 头（`55 AA`），其镜像长度在 +2（u16 × 512B），
/// VN 链从间隙之后开始；指向间隙之后的指针需加间隙长度。
fn efi_gap_after_pc(data: &[u8], base: usize) -> (usize, usize) {
    if base + 0x1A > data.len() {
        return (0, 0);
    }
    let pcir_rel = usize::from(u16::from(data[base + 0x18]) | u16::from(data[base + 0x19]) << 8);
    let pcir = base + pcir_rel;
    if pcir + 0x12 > data.len() {
        return (0, 0);
    }
    // PCIR 签名 4B 后 +0x0C 处才是 image_length_in_512（即 pcir+0x10）。
    let pc_end =
        base + usize::from(u16::from(data[pcir + 0x10]) | u16::from(data[pcir + 0x11]) << 8) * 512;
    if pc_end + 4 > data.len() || data[pc_end] != 0x55 || data[pc_end + 1] != 0xAA {
        return (pc_end, 0);
    }
    let efi_len = usize::from(u16::from(data[pc_end + 2]) | u16::from(data[pc_end + 3]) << 8) * 512;
    (pc_end, efi_len)
}

/// 定位 Falcon ucode 表（BIT 'p'，仅 data_version==2 && data_size==4）。
///
/// 指针需经 EFI 间隙重定位（hexpat `adjust_skip_efi`）：目标越过 PC 镜像
/// 末尾时加 EFI 镜像长度——四块实测 ROM（GP104×2/GM200/P100）全部命中
/// `ver=0x01 hdr=6 entry=6`，PMU 固件清单（PRE_OS + PRIMARY_DEVINIT +
/// FW_SEC_LIC）逐位一致。
pub fn find_falcon_table(data: &[u8]) -> Result<Option<FalconTable>, Error> {
    let img = find_image_base(data).ok_or_else(|| Error::from("falcon: 55 AA base not found"))?;
    let r = Reader { data };
    let sig = find_bit_signature(data, img)
        .ok_or_else(|| Error::from("falcon: BIT signature not found"))?;
    let entries = bit_entries(data, sig, img)?;
    let p = entries
        .iter()
        .find(|e| e.id == b'p' && e.ver == 2 && e.len == 4)
        .ok_or_else(|| Error::from("falcon: BIT 'p' v2/len4 token not found"))?;
    let ptr = r.u32(img + usize::from(p.offset))?;
    if ptr == 0 {
        return Ok(None);
    }
    let (pc_end, gap) = efi_gap_after_pc(data, img);
    let raw_abs = img + usize::try_from(ptr).map_err(|_| Error::from("falcon: bad pointer"))?;
    let table_offset = if raw_abs > pc_end.saturating_sub(1) {
        raw_abs + gap
    } else {
        raw_abs
    };
    if table_offset + 6 > data.len() {
        return Ok(None);
    }
    // 表头：ver=0x01 hdr=6 entry=6（hexpat FALCON_UCODE_TABLE_HDR_V1 常量）。
    if r.u8(table_offset)? != 0x01 || r.u8(table_offset + 1)? != 6 || r.u8(table_offset + 2)? != 6 {
        return Ok(None);
    }
    let entry_count = r.u8(table_offset + 3)?;
    let desc_version = r.u8(table_offset + 4)?;
    let desc_size = r.u8(table_offset + 5)?;
    let mut out = Vec::new();
    for i in 0..usize::from(entry_count).min(32) {
        let e = table_offset + 6 + i * 6;
        if e + 6 > data.len() {
            break;
        }
        let application_id = r.u8(e)?;
        let target_id = r.u8(e + 1)?;
        let desc_ptr = r.u32(e + 2)?;
        let mut desc = None;
        if desc_ptr != 0 {
            // DescPtr 为表内相对（hexpat: @parent + DescPtr - falcon_table_ptr）。
            let Ok(rel_ptr) = usize::try_from(ptr) else {
                continue;
            };
            let Ok(rel_desc) = usize::try_from(desc_ptr) else {
                continue;
            };
            let d = table_offset + rel_desc - rel_ptr;
            // 字段最多用到 version/crypt 头(4) + 11 个 u32 的前 10 个(40)。
            let has_version_crypt = data.get(d).copied().unwrap_or(0) & 1 != 0;
            let field_base = d + if has_version_crypt { 4 } else { 0 };
            if field_base + 40 <= data.len() {
                let rd = |a: usize| {
                    u32::from(data[a])
                        | u32::from(data[a + 1]) << 8
                        | u32::from(data[a + 2]) << 16
                        | u32::from(data[a + 3]) << 24
                };
                desc = Some(FalconUcodeDesc {
                    has_version_crypt,
                    version: if has_version_crypt { data[d + 1] } else { 0 },
                    stored_size: rd(field_base),
                    uncompressed_size: rd(field_base + 4),
                    virtual_entry: rd(field_base + 8),
                    imem_phys_base: rd(field_base + 12),
                    imem_load_size: rd(field_base + 16),
                    dmem_phys_base: rd(field_base + 32),
                    dmem_load_size: rd(field_base + 36),
                });
            }
        }
        out.push(FalconUcodeEntry {
            application_id,
            target_id,
            application: falcon_app_name(application_id),
            target: falcon_target_name(target_id),
            desc_ptr,
            desc,
        });
    }
    Ok(Some(FalconTable {
        table_offset,
        raw_ptr: ptr,
        desc_version,
        desc_size,
        entries: out,
    }))
}

// ── NVGI 头（程序员 dump 专有；GP104.rom 实测校准）───────────────────────

/// NVGI 根头（完整 flash dump 开头；NvAPI 镜像直接以 PC boot sector 开头
/// 无此头）。
///
/// 字节布局 = 3 个 IFR 寄存器（kernel_gsp_vbios_tu102.c 的
/// NV_PBUS_IFR_FMT_FIXED0/1/2）。GP104.rom 逐位校准：`4E 56 47 49 | 44 |
/// 02 10 | 80 | 7c 08 00 80 | de 10 | 9e 11` → FIXED1=0x80100244（版本
/// bits[15:8]=0x02 Pascal、头长 bits[23:16]=0x10、+4 可变字节 0x44、旗标
/// 0x80）、FIXED2=0x8000087C（total 低 24 位=0x87C）、xve_sub_vendor=
/// 0x10DE（NVIDIA）、xve_subsystem_id=0x119E（GP104M）。⚠ 旧实现误把
/// +4 当版本、+5 u16 当 fixed_data_size——Turing 1MB dump 实测版本=0x03
/// 在 +5（hexpat "version==3" 判 Turing-Ada 用的正是本字节）。hexpat 用
/// 自身版本哈希数组凑偏移（ver%754），Rust 直接硬编码实测偏移。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NvgiHeader {
    /// +5 字节：IFR FIXED1 bits[15:8]。实测 0x01=Maxwell、0x02=Pascal、
    /// **0x03=全部 Turing/Ampere/Ada**（hexpat "version==3 ⇒ Turing-Ada"
    /// 对应本字节；⚠ 旧实现误读 +4——那是逐文件可变字节）。
    pub version: u8,
    /// +6 字节：IFR FIXED1 bits[23:16] = NVGI 头长（0x10 Maxwell/Pascal、
    /// 0x24 Turing+；hexpat 的 "u16 fixed_data_size@5" 亦误）。
    pub header_len: u8,
    /// +4 字节：FIXED1 低 8 位（逐文件可变：0xFB/0xD4/0x38…，未定名）。
    pub fixed_byte: u8,
    /// +7 字节：FIXED1 bits[31:24] 旗标（GP104 观测 0x80，Turing+ 全 0）。
    pub flags_hi: u8,
    /// u32 低 24 位。
    pub total_data_size: u32,
    /// u32 高 8 位。
    pub flags: u8,
    pub xve_sub_vendor: u16,
    pub xve_subsystem_id: u16,
}

/// 解析文件开头的 NVGI 头（magic 不符返回 None——bare PC ROM / Blackwell
/// `4C` 容器 / Hopper+ 新格式）。
pub fn parse_nvgi(data: &[u8]) -> Option<NvgiHeader> {
    if data.len() < 16 || &data[0..4] != b"NVGI" {
        return None;
    }
    let rd16 = |a: usize| u16::from(data[a]) | u16::from(data[a + 1]) << 8;
    // 布局 = 3 个 IFR 寄存器（open-gpu-kernel-modules kernel_gsp_vbios_tu102.c
    // 的 NV_PBUS_IFR_FMT_FIXED0/1/2）：FIXED1 = [31:24]旗标|[23:16]头长|
    // [15:8]版本|[7:0]可变字节，FIXED2 = [31:24]旗标|[23:0]total。
    let fixed1 = u32::from(data[4])
        | u32::from(data[5]) << 8
        | u32::from(data[6]) << 16
        | u32::from(data[7]) << 24;
    let flags_total = u32::from(data[8])
        | u32::from(data[9]) << 8
        | u32::from(data[10]) << 16
        | u32::from(data[11]) << 24;
    Some(NvgiHeader {
        version: ((fixed1 >> 8) & 0xFF) as u8,
        header_len: ((fixed1 >> 16) & 0xFF) as u8,
        fixed_byte: (fixed1 & 0xFF) as u8,
        flags_hi: ((fixed1 >> 24) & 0xFF) as u8,
        total_data_size: flags_total & 0x00FF_FFFF,
        flags: (flags_total >> 24) as u8,
        xve_sub_vendor: rd16(12),
        xve_subsystem_id: rd16(14),
    })
}

// ── RFFS/RFRD flash 目录（NVGI 容器尾部；hexpat 布局首次实测定案）────────

/// RFFS 头（flash 状态 ledger 的目录头，magic "RFFS"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RffsHeader {
    /// 实测 1=Turing、2=Ampere/Ada。
    pub version: u16,
    /// 实测恒 12。
    pub header_size: u16,
    /// 实测恒 32（hexpat 未注明；ledger 条目 32B 非 16B）。
    pub entry_size: u16,
    /// 实测恒 4096（128 条）。
    pub ledger_size: u16,
}

/// RFRD（ROM directory，magic "RFRD"）——整片 flash 的分区表。
/// 所有偏移是**绝对 flash 偏移**（full dump 下直接可用，无需重定位）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RomDirectory {
    /// 实测 2=Turing、3=Ampere/Ada。
    pub version: u16,
    /// 实测 0x20（Turing）/0x28（Ampere+）。
    pub struct_size: u16,
    /// PC option ROM 镜像基址（该偏移处实测必为 55 AA）。
    pub pci_option_rom_offset: u32,
    pub pci_option_rom_size: u32,
    /// InfoROM 偏移（该处实测为 "JFFS" v2 魔数）与长度。
    pub inforom_offset: u32,
    pub inforom_size: u32,
    /// bootloader ucode 偏移（Turing=0x40、GA102=0xFFFFFFFF、Ada=NVGI 副本处）。
    pub bootloader_ucode_offset: u32,
    pub secondary_base: u32,
}

/// NVGI 容器的 flash 目录整体（Turing+ full dump 专有；Pascal 及以前、
/// bare PC ROM、Blackwell 均无）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlashDirectory {
    /// FlashStatus 指针对所在偏移（= NVGI total_data_size）。
    pub status_offset: usize,
    pub data_offset: u32,
    pub header_offset: u32,
    pub rffs: RffsHeader,
    /// RFRD 结构所在偏移（= data_offset + ledger_size）。
    pub rom_dir_offset: usize,
    pub rom_dir: RomDirectory,
    /// 55 AA 校验：`rom_dir.pci_option_rom_offset` 处是否真的是 55 AA。
    pub pci_rom_magic_ok: bool,
}

/// 定位 NVGI 容器尾部的 RFFS/RFRD flash 目录。链路（10/10 实测）：
/// `total_data_size → {u32 data_offset, u32 header_offset}` → RFFS 头 @
/// header_offset → ledger @ data_offset → **RFRD @ data_offset +
/// ledger_size**（不经过 header_offset 路径；open-gpu-kernel-modules 同款）。
pub fn find_flash_directory(data: &[u8]) -> Option<FlashDirectory> {
    let nvgi = parse_nvgi(data)?;
    let total = nvgi.total_data_size as usize;
    if total + 8 > data.len() {
        return None;
    }
    let rd32 = |a: usize| -> u32 {
        u32::from(data[a])
            | u32::from(data.get(a + 1).copied().unwrap_or(0)) << 8
            | u32::from(data.get(a + 2).copied().unwrap_or(0)) << 16
            | u32::from(data.get(a + 3).copied().unwrap_or(0)) << 24
    };
    let data_offset = rd32(total);
    let header_offset = rd32(total + 4);
    // RFFS 头 @ header_offset（magic "RFFS"）。
    let ho = header_offset as usize;
    if ho + 12 > data.len() || &data.get(ho..ho + 4)? != b"RFFS" {
        return None;
    }
    let rd16 =
        |a: usize| u16::from(data[a]) | u16::from(data.get(a + 1).copied().unwrap_or(0)) << 8;
    let rffs = RffsHeader {
        version: rd16(ho + 4),
        header_size: rd16(ho + 6),
        entry_size: rd16(ho + 8),
        ledger_size: rd16(ho + 10),
    };
    // RFRD @ data_offset + ledger_size。
    let ro = data_offset as usize + usize::from(rffs.ledger_size);
    if ro + 0x20 > data.len() || &data.get(ro..ro + 4)? != b"RFRD" {
        return None;
    }
    let rom_dir = RomDirectory {
        version: rd16(ro + 4),
        struct_size: rd16(ro + 6),
        pci_option_rom_offset: rd32(ro + 8),
        pci_option_rom_size: rd32(ro + 12),
        inforom_offset: rd32(ro + 16),
        inforom_size: rd32(ro + 20),
        bootloader_ucode_offset: rd32(ro + 24),
        secondary_base: rd32(ro + 28),
    };
    let pci_rom_magic_ok = rom_dir.pci_option_rom_offset as usize + 2 <= data.len()
        && data[rom_dir.pci_option_rom_offset as usize..rom_dir.pci_option_rom_offset as usize + 2]
            == [0x55, 0xAA];
    Some(FlashDirectory {
        status_offset: total,
        data_offset,
        header_offset,
        rffs,
        rom_dir_offset: ro,
        rom_dir,
        pci_rom_magic_ok,
    })
}

// ── Blackwell（50 系）容器 ──────────────────────────────────────────────
// 40 ROM 实测（VBIOS_sample 2026-09）：与 NVGI 容器完全不同——文件头
// `4C C0`/`4C FF`（0x4C 恒定，第二字节桌面/移动无分布规律）、0x1000 起
// NVFW 压缩固件区、0x33C00+ 起 55AA 镜像链（8 张 PCIR）、有效内容恒止于
// 0x1D2DD0（非 512 对齐 = ImHex hexpat 失败根因）。BIT 表保留但 token
// offset 相对 55AA 镜像基址；'P' 槽指针需 per-build delta 重定位。

/// Blackwell 容器签名（`4C C0` 或 `4C FF`）。
pub fn is_blackwell_container(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0x4C && (data[1] == 0xC0 || data[1] == 0xFF)
}

/// 'P'/'C' 槽 raw → 文件偏移的重定位锚（20/20 实测）：
/// `delta = pos(40 38 66 14 12 0f 0d 0d) − u32@P+0x2C`，delta 恒 0x400 对齐。
const BLACKWELL_DELTA_MARK: [u8; 8] = [0x40, 0x38, 0x66, 0x14, 0x12, 0x0F, 0x0D, 0x0D];

/// 功率记录锚（image3 TLV，20/20 命中）：@+7 预算档、@+0xB RATED mW、
/// @+0xF MAX mW（移动端 Rated/Max 与文件名逐一吻合；5090 桌面 575W 锚
/// 未在样本出现）。
const BLACKWELL_POWER_ANCHOR: [u8; 5] = [0x01, 0x2B, 0x07, 0x00, 0x0C];

/// Blackwell 功率记录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlackwellPower {
    pub anchor_offset: usize,
    /// @+7：预算档（移动端观测 5000，桌面 98.06 观测 1）。
    pub budget_raw: u32,
    /// @+0xB：RATED 功率（mW）。
    pub rated_mw: u32,
    /// @+0xF：MAX 功率（mW）。
    pub max_mw: u32,
}

/// Blackwell PERF_PTRS 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlackwellPerf {
    /// 'P' 表绝对偏移。
    pub table_offset: usize,
    /// 78 个 u32 槽 raw 值。
    pub slots: Vec<u32>,
    /// raw + delta = 文件偏移；经 MARK 锚求出（0x400 对齐校验通过）。
    pub delta: Option<u32>,
}

/// Blackwell 容器摘要（BIT + 'P' 槽重定位 + 功率记录 + 热/风策略）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlackwellInfo {
    pub bit_offset: usize,
    /// 55AA 镜像基址（BIT token offset 的基准）。
    pub image_base: usize,
    /// token 数（实测 20；新增首 token id 0x32）。
    pub token_count: usize,
    pub perf: Option<BlackwellPerf>,
    pub power: Option<BlackwellPower>,
    /// ThermalPolicy（P+0x50→delta）slowdown 温度 ×3 通道（°C；
    /// 5060Ti 87/5080es 97/5090es 89 实测）。
    pub slowdown_c: Vec<u8>,
    /// FanPolicy（P+0x5C→delta）曲线（(°C, RPM) 点，编码与旧世代同）。
    pub fan_curves: Vec<FanCurvePoint>,
    /// FanCooler（P+0x58→delta）每风扇 (min,max) RPM 对。
    pub fan_cooler_pairs: Vec<(u16, u16)>,
}

impl BlackwellInfo {
    /// 'P' 槽 raw → 文件偏移（delta 就绪时）。
    pub fn resolve(&self, raw: u32) -> Option<usize> {
        let delta = self.perf.as_ref()?.delta? as usize;
        Some(raw as usize + delta)
    }
}

/// 解析 Blackwell 容器（非 Blackwell 返回 None；缺 delta/功率记录时字段
/// 留空而非报错）。
pub fn find_blackwell(data: &[u8]) -> Option<BlackwellInfo> {
    if !is_blackwell_container(data) {
        return None;
    }
    // BIT：全文唯一 `FF B8 "BIT\0"`。
    let bit_offset = data
        .windows(6)
        .position(|w| w == [0xFF, 0xB8, b'B', b'I', b'T', 0])?;
    // 镜像基址：自 BIT 向下按 0x200 找 55AA。
    let mut image_base = None;
    let mut a = bit_offset & !0x1FF;
    while a >= 0x200 {
        if data[a - 0x200..a - 0x200 + 2] == [0x55, 0xAA] {
            image_base = Some(a - 0x200);
            break;
        }
        a -= 0x200;
    }
    let image_base = image_base?;
    let r = Reader { data };
    let entries = bit_entries(data, bit_offset, image_base).ok()?;
    let mut info = BlackwellInfo {
        bit_offset,
        image_base,
        token_count: entries.len(),
        ..BlackwellInfo::default()
    };

    // 'P' 表：78 × u32。
    let p_entry = entries.iter().find(|e| e.id == b'P')?;
    let p_off = image_base + usize::from(p_entry.offset);
    if p_off + usize::from(p_entry.len) > data.len() {
        return Some(info);
    }
    let slot_count = usize::from(p_entry.len) / 4;
    let slots: Vec<u32> = (0..slot_count)
        .map(|i| r.u32(p_off + i * 4).unwrap_or(0))
        .collect();
    // delta：MARK 锚 − u32@P+0x2C（板级配置记录指针）。
    let mut delta = None;
    if let Some(mark_pos) = data
        .windows(BLACKWELL_DELTA_MARK.len())
        .position(|w| w == BLACKWELL_DELTA_MARK)
    {
        let raw = r.u32(p_off + 0x2C).unwrap_or(0) as usize;
        if raw > 0 && mark_pos >= raw {
            let d = mark_pos - raw;
            if d.is_multiple_of(0x400) {
                delta = Some(d as u32);
            }
        }
    }
    info.perf = Some(BlackwellPerf {
        table_offset: p_off,
        slots,
        delta,
    });

    // 功率记录（不依赖 delta——TLV 记录区直接全文扫锚）。
    for anchor in find_all(data, &BLACKWELL_POWER_ANCHOR, 0) {
        if anchor + 0x13 > data.len() {
            continue;
        }
        let budget = r.u32(anchor + 5 + 2).unwrap_or(0);
        let rated = r.u32(anchor + 0xB).unwrap_or(0);
        let max = r.u32(anchor + 0xF).unwrap_or(0);
        if (5_000..=1_000_000).contains(&rated) && rated <= max && max <= 1_000_000 {
            info.power = Some(BlackwellPower {
                anchor_offset: anchor,
                budget_raw: budget,
                rated_mw: rated,
                max_mw: max,
            });
            break;
        }
    }

    let Some(perf) = &info.perf else {
        return Some(info);
    };
    let Some(d) = delta else {
        return Some(info);
    };
    let resolve = |slot: u32| -> Option<usize> {
        let raw = *perf.slots.get(slot as usize / 4)? as usize;
        let p = raw + d as usize;
        (p < data.len()).then_some(p)
    };

    // ThermalPolicy（槽 +0x50）：头 `20 0c 30 0f`，slowdown 3×u16@+0x0E。
    if let Some(t) = resolve(0x50)
        && data.get(t..t + 4) == Some(&[0x20, 0x0C, 0x30, 0x0F][..])
        && t + 0x14 <= data.len()
    {
        for k in 0..3 {
            let v = r.u16(t + 0x0E + k * 2).unwrap_or(0);
            if v != 0 && v != 0xFFFF {
                info.slowdown_c.push((v >> 5) as u8);
            }
        }
    }
    // FanPolicy（槽 +0x5C）：头 `20 04 33 0b`，曲线 @+0x14 (t<<5, rpm)。
    if let Some(t) = resolve(0x5C)
        && data.get(t..t + 4) == Some(&[0x20, 0x04, 0x33, 0x0B][..])
        && t + 0x14 + 24 <= data.len()
    {
        let mut prev_t = 0u16;
        let mut prev_r = 0u16;
        for k in 0..6u32 {
            let temp = r.u16(t + 0x14 + k as usize * 4).unwrap_or(0);
            let rpm = r.u16(t + 0x14 + k as usize * 4 + 2).unwrap_or(0);
            // 合理性门与旧世代一致：温度 20..120°C、RPM ≤ 8000、递增。
            if !(640..=3840).contains(&temp)
                || !temp.is_multiple_of(32)
                || rpm == 0
                || rpm > 8000
                || temp <= prev_t
                || rpm <= prev_r
            {
                break;
            }
            info.fan_curves.push(FanCurvePoint {
                temp_c: f64::from(temp) / 32.0,
                rpm,
            });
            prev_t = temp;
            prev_r = rpm;
        }
    }
    // FanCooler（槽 +0x58）：每风扇 (min,max) RPM u16 对。
    if let Some(t) = resolve(0x58) {
        for k in 0..4u32 {
            if t + k as usize * 4 + 4 > data.len() {
                break;
            }
            let min = r.u16(t + k as usize * 4).unwrap_or(0);
            let max = r.u16(t + k as usize * 4 + 2).unwrap_or(0);
            if min == 0 || min > max || max > 8000 {
                break;
            }
            info.fan_cooler_pairs.push((min, max));
        }
    }
    Some(info)
}

// ── 显存表族（hexpat MEMORY_CLOCK/MEMTWEAK/MEMORY_INFO 布局移植）─────────

/// 内存时钟频段一条（`MEMORY_CLOCK_BASE_ENTRY_11` + 其 strap 副本）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryClockEntry {
    /// 频段下限（原始 u16；hexpat 未注明单位——实测 GP104 首段 0..540，
    /// 与 GDDR5 内存时钟 MHz 同量级，未定案前只出 raw）。
    pub freq_min_raw: u16,
    pub freq_max_raw: u16,
    /// 内存 init 脚本指针（镜像相对）。
    pub script_ptr: u32,
    pub flags0: u8,
    pub fbpa_config: u32,
    pub fbpa_config1: u32,
    pub flags1: u8,
    /// MPLL SS 频率增量（0.01% 单位，最大 2.55%）。
    pub ref_mplls_freq_delta: u8,
    /// 每 strap 一份的副本（`MEMORY_CLOCK_STRAP_ENTRY_11`；>12B 截断）。
    pub straps: Vec<[u8; 12]>,
}

/// 内存时钟频段表（P+0x04；GPU Boost 前的显存频率档位 + 每 strap 训练/
/// 时序副本）。GP104 实测 11 频段×10 strap。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryClockTable {
    pub table_offset: usize,
    pub ver: u8,
    /// FBVDD/Q 切换后的延迟（µs）。
    pub fbvdd_settle_us: u8,
    /// Perf 内存脚本列表指针（镜像相对）。
    pub script_list_ptr: u32,
    /// HeaderSize ≥ 0x1A 时的 Cmd 脚本扩展。
    pub cmd_script_list_ptr: Option<u32>,
    pub entries: Vec<MemoryClockEntry>,
}

/// 定位并解析内存时钟表（BIT 'P' +0x04，EFI 间隙重定位同 falcon 规则）。
pub fn find_memory_clock_table(data: &[u8]) -> Result<Option<MemoryClockTable>, Error> {
    let Some(t) = perf_slot_target(data, 0x04)? else {
        return Ok(None);
    };
    if t + 6 > data.len() {
        return Ok(None);
    }
    let r = Reader { data };
    let ver = r.u8(t)?;
    let hdr = usize::from(r.u8(t + 1)?);
    let besz = usize::from(r.u8(t + 2)?);
    let sesz = usize::from(r.u8(t + 3)?);
    let sec = usize::from(r.u8(t + 4)?);
    let cnt = usize::from(r.u8(t + 5)?);
    let fbvdd_settle_us = r.u8(t + 7)?;
    let script_list_ptr = r.u32(t + 0x10)?;
    // Base 条目 stride = besz + sec×sesz（hexpat inline settings 数组）。
    let stride = besz + sec * sesz;
    let mut entries = Vec::new();
    for i in 0..cnt.min(64) {
        let e = t + hdr + i * stride;
        if e + besz > data.len() {
            break;
        }
        let mut straps = Vec::new();
        for s in 0..sec {
            let sp = e + besz + s * sesz;
            if sp + sesz > data.len() {
                break;
            }
            let mut row = [0u8; 12];
            let n = sesz.min(12);
            row[..n].copy_from_slice(&data[sp..sp + n]);
            straps.push(row);
        }
        entries.push(MemoryClockEntry {
            freq_min_raw: r.u16(e)?,
            freq_max_raw: r.u16(e + 2)?,
            script_ptr: r.u32(e + 4)?,
            flags0: r.u8(e + 8)?,
            fbpa_config: r.u32(e + 9)?,
            fbpa_config1: r.u32(e + 13)?,
            flags1: r.u8(e + 17)?,
            ref_mplls_freq_delta: r.u8(e + 18)?,
            straps,
        });
    }
    Ok(Some(MemoryClockTable {
        table_offset: t,
        ver,
        fbvdd_settle_us,
        script_list_ptr,
        cmd_script_list_ptr: (hdr >= 0x1A).then(|| r.u32(t + 0x15)).transpose()?,
        entries,
    }))
}

/// 内存时序表（P+0x08；`MEMTWEAK_HEADER_20` + 条目）。时序字段 hexpat 也
/// 仅存原始字节（`MEMTWEAK_BASE_ENTRY_20` 命名未解码），照实携带；GP104
/// 实测 64 组 × 68B + 扩展寄存器组。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryTweakTable {
    pub table_offset: usize,
    pub ver: u8,
    pub base_entry_size: u8,
    pub extended_entry_size: u8,
    pub extended_entry_count: u8,
    /// 每组 = BaseEntrySize 字节 + ExtEntryCount×ExtEntrySize 寄存器。
    pub entries: Vec<Vec<u8>>,
}

/// 定位并解析内存时序表（BIT 'P' +0x08）。
pub fn find_memory_tweak_table(data: &[u8]) -> Result<Option<MemoryTweakTable>, Error> {
    let Some(t) = perf_slot_target(data, 0x08)? else {
        return Ok(None);
    };
    if t + 6 > data.len() {
        return Ok(None);
    }
    let r = Reader { data };
    let ver = r.u8(t)?;
    let hdr = usize::from(r.u8(t + 1)?);
    let besz = usize::from(r.u8(t + 2)?);
    let ees = usize::from(r.u8(t + 3)?);
    let eec = usize::from(r.u8(t + 4)?);
    let cnt = usize::from(r.u8(t + 5)?);
    let stride = besz + eec * ees;
    let mut entries = Vec::new();
    for i in 0..cnt.min(256) {
        let e = t + hdr + i * stride;
        if e + stride > data.len() {
            break;
        }
        entries.push(data[e..e + stride].to_vec());
    }
    Ok(Some(MemoryTweakTable {
        table_offset: t,
        ver,
        base_entry_size: r.u8(t + 2)?,
        extended_entry_size: r.u8(t + 3)?,
        extended_entry_count: r.u8(t + 4)?,
        entries,
    }))
}

/// 内存变体位域（`MemoryVariant`，strap 选择 u32 的位域分解）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryVariant {
    /// 显存类型（[`memory_type_name`]）。
    pub mem_type: u8,
    pub strap: u8,
    pub index: u8,
    /// 厂商（[`memory_vendor_name`]）。
    pub vendor_id: u8,
    pub rev_id: u8,
    /// 密度（[`memory_density_name`]）。
    pub density: u8,
    /// 单/双面（clamshell，按类型有不同单面值）。
    pub org: u8,
    pub feature: u8,
}

/// 显存类型名（hexpat `memory_strap_info_parse` 枚举）。
pub fn memory_type_name(t: u8) -> Option<&'static str> {
    Some(match t {
        0x1 => "DDR3",
        0x2 => "GDDR3",
        0x3 => "GDDR5",
        0x4 => "SDDR4",
        0x5 => "HBM1",
        0x6 => "HBM2",
        0x7 => "LPDDR4",
        0x8 => "GDDR5X",
        0x9 => "GDDR6",
        0xA => "GDDR6X",
        0xB => "LPDDR4X",
        0xC => "HBM3",
        0xE => "GDDR4",
        _ => return None,
    })
}

/// 显存厂商名（同上枚举）。
pub fn memory_vendor_name(v: u8) -> Option<&'static str> {
    Some(match v {
        0x1 => "Samsung",
        0x2 => "Qimonda",
        0x3 => "Elpida",
        0x4 => "Etron",
        0x5 => "Nanya",
        0x6 => "Hynix",
        0x7 => "ProMOS",
        0x8 => "WinBond",
        0x9 => "ESMT",
        0xF => "Micron",
        _ => return None,
    })
}

/// 显存密度名（同上枚举）。
pub fn memory_density_name(d: u8) -> Option<&'static str> {
    Some(match d {
        0 => "256Mb",
        1 => "512Mb",
        2 => "1Gb",
        3 => "2Gb",
        4 => "4Gb",
        5 => "8Gb",
        6 => "16Gb",
        _ => return None,
    })
}

/// 内存 info 条目（`MEMORY_INFO_ENTRY_10`；EntrySize ≥ 22 的热/功耗扩展
/// ——tj_max/fan 上限、热策略、每 strap 功率修正斜率/截距(mW)）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryInfoEntry {
    pub variant: MemoryVariant,
    pub partition_group: u8,
    /// 13 位 mclk_bg_enable_freq + 3 位 flags。
    pub mclk_flags_raw: u16,
    /// EntrySize ≥ 22 扩展。
    pub tj_max_fan_limit: Option<u8>,
    pub therm_policy: Option<u16>,
    pub pwr_adjustment_slope: Option<u32>,
    /// 毫瓦。
    pub pwr_adjustment_intercept_mw: Option<i32>,
}

/// 内存 info 表（BIT 'M' → MemInfoTblPtr；strap→显存变体映射）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryInfoTable {
    pub table_offset: usize,
    pub strap_count: u8,
    /// strap 翻译表（镜像相对指针 + u8 数组）。
    pub strap_translation: Vec<u8>,
    pub entries: Vec<MemoryInfoEntry>,
}

/// 定位并解析内存 info 表（BIT 'M' v2 布局：strap_count u8 + xlat u16 +
/// meminfo u16 + training u32 + pattern u32 + partinfo u32）。
pub fn find_memory_info(data: &[u8]) -> Result<Option<MemoryInfoTable>, Error> {
    let img = find_image_base(data).ok_or_else(|| Error::from("meminfo: 55 AA base not found"))?;
    let r = Reader { data };
    let sig = find_bit_signature(data, img)
        .ok_or_else(|| Error::from("meminfo: BIT signature not found"))?;
    let entries = bit_entries(data, sig, img)?;
    let m = entries
        .iter()
        .find(|e| e.id == b'M' && e.ver == 2)
        .ok_or_else(|| Error::from("meminfo: BIT 'M' v2 token not found"))?;
    let mtab = img + usize::from(m.offset);
    let strap_count = r.u8(mtab)?;
    let xlat = usize::from(r.u16(mtab + 1)?);
    let minfo_rel = usize::from(r.u16(mtab + 3)?);
    if minfo_rel == 0 {
        return Ok(None);
    }
    let t = img + minfo_rel;
    if t + 5 > data.len() {
        return Ok(None);
    }
    let hdr = usize::from(r.u8(t + 1)?);
    let elen = usize::from(r.u8(t + 2)?);
    let cnt = usize::from(r.u8(t + 3)?);
    let mut out = Vec::new();
    for i in 0..cnt.min(64) {
        let e = t + hdr + i * elen;
        if e + elen > data.len() || elen < 7 {
            break;
        }
        let var = r.u32(e)?;
        out.push(MemoryInfoEntry {
            variant: MemoryVariant {
                mem_type: (var & 0xF) as u8,
                strap: ((var >> 4) & 0xF) as u8,
                index: ((var >> 8) & 0xF) as u8,
                vendor_id: ((var >> 12) & 0xF) as u8,
                rev_id: ((var >> 16) & 0xF) as u8,
                density: ((var >> 20) & 0xF) as u8,
                org: ((var >> 24) & 7) as u8,
                feature: ((var >> 27) & 0x1F) as u8,
            },
            partition_group: r.u8(e + 4)?,
            mclk_flags_raw: r.u16(e + 5)?,
            tj_max_fan_limit: (elen >= 22).then(|| r.u8(e + 0xB)).transpose()?,
            therm_policy: (elen >= 22).then(|| r.u16(e + 0xC)).transpose()?,
            pwr_adjustment_slope: (elen >= 22).then(|| r.u32(e + 0xE)).transpose()?,
            pwr_adjustment_intercept_mw: (elen >= 22)
                .then(|| r.u32(e + 0x12))
                .transpose()?
                .map(|v| v as i32),
        });
    }
    let strap_translation = (xlat != 0 && strap_count > 0)
        .then(|| {
            let x = img + xlat;
            (x < data.len())
                .then(|| data[x..(x + usize::from(strap_count)).min(data.len())].to_vec())
        })
        .flatten()
        .unwrap_or_default();
    Ok(Some(MemoryInfoTable {
        table_offset: t,
        strap_count,
        strap_translation,
        entries: out,
    }))
}

/// 取 PERF_PTR 指定槽位的重定位目标（内部共享）。
fn perf_slot_target(data: &[u8], slot: usize) -> Result<Option<usize>, Error> {
    let img =
        find_image_base(data).ok_or_else(|| Error::from("mem table: 55 AA base not found"))?;
    let r = Reader { data };
    let sig = find_bit_signature(data, img)
        .ok_or_else(|| Error::from("mem table: BIT signature not found"))?;
    let entries = bit_entries(data, sig, img)?;
    let p = entries
        .iter()
        .find(|e| e.id == b'P')
        .ok_or_else(|| Error::from("mem table: BIT 'P' entry not found"))?;
    let ptab = img + usize::from(p.offset);
    if usize::from(p.len) < slot + 4 {
        return Ok(None);
    }
    let ptr = r.u32(ptab + slot)?;
    if ptr == 0 {
        return Ok(None);
    }
    let (pc_end, gap) = efi_gap_after_pc(data, img);
    let raw = img + usize::try_from(ptr).map_err(|_| Error::from("mem table: bad pointer"))?;
    let abs = if raw > pc_end.saturating_sub(1) {
        raw + gap
    } else {
        raw
    };
    Ok(Some(abs))
}

// ── BIOS 身份（GPU-Z General 面板同源信息）───────────────────────────────

/// BIOS 身份信息（BIT 'B'/'i'/'S' 三 token + x86 头日期；GPU-Z General
/// 面板逐字段对照：GP104 实卡 version="86.04.50.40.4A"、build=
/// "2017-07-03"、message="GV-N1070G1"、board="GP104 Board"）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BiosIdentity {
    /// BIT 'B'：Version u32 的 BCD 段 + OemVersion hex → "86.04.50.40.4A"。
    pub version: Option<String>,
    /// BIT 'B' OemVersion 原始字节。
    pub oem_version: Option<u8>,
    /// x86 头部 0x36 起 "FT07/03/17"（2 字母前缀 + MM/DD/YY）→
    /// "2017-07-03"。
    pub build_date: Option<String>,
    /// BIT 'i' @0x0F 的 8 字节编译日期串（GP104 实测 "10/07/16"；与
    /// PC 头发布日期不同源，原样携带）。
    pub internal_build_date: Option<String>,
    /// 字符串[0] 首行首空格前（"GV-N1070G1"）。
    pub message: Option<String>,
    /// 含 " Board" 的字符串（"GP104 Board"）。
    pub board_id: Option<String>,
    /// BIT 'i' bCertFlag（@0x44，token ≥ 0x44 时）。
    pub cert_flag: Option<u8>,
    /// BIT 'B' H264/HEVC 能力位 @0x0C：bit0 H264 解码禁、bit1 编码禁、
    /// bit2 HEVC 解码禁、bit3 编码禁、bit4-7 Region1-4 禁。
    pub h264_hevc_caps: Option<u32>,
    /// BIT 'B' PostMaxNumHeads @0x14（实测恒 2）。
    pub max_heads: Option<u8>,
    /// BIT 'S' 字符串表全量（原样）。
    pub strings: Vec<String>,
}

/// 解析 BIOS 身份（GPU-Z General 面板同源）。
pub fn find_bios_identity(data: &[u8]) -> Result<BiosIdentity, Error> {
    let img = find_image_base(data).ok_or_else(|| Error::from("identity: 55 AA base not found"))?;
    let r = Reader { data };
    let sig = find_bit_signature(data, img)
        .ok_or_else(|| Error::from("identity: BIT signature not found"))?;
    let entries = bit_entries(data, sig, img)?;
    let mut id = BiosIdentity::default();

    // 'B' BiosData：Version u32（BE 字节段 = BCD 对）+ OemVersion u8 +
    // H264/HEVC caps @0x0C + PostMaxNumHeads @0x14。
    if let Some(b) = entries.iter().find(|e| e.id == b'B') {
        let t = img + usize::from(b.offset);
        if t + 5 <= data.len() {
            let v = r.u32(t)?;
            let oem = r.u8(t + 4)?;
            id.version = Some(format!(
                "{:02x}.{:02x}.{:02x}.{:02x}.{oem:02X}",
                (v >> 24) & 0xFF,
                (v >> 16) & 0xFF,
                (v >> 8) & 0xFF,
                v & 0xFF
            ));
            id.oem_version = Some(oem);
        }
        if t + 0x10 <= data.len() {
            id.h264_hevc_caps = Some(r.u32(t + 0x0C)?);
        }
        if t + 0x15 <= data.len() {
            id.max_heads = Some(r.u8(t + 0x14)?);
        }
    }

    // 'i' InternalUse：编译日期 @0x0F、CertFlag @0x44。
    if let Some(i) = entries.iter().find(|e| e.id == b'i') {
        let t = img + usize::from(i.offset);
        let len = usize::from(i.len);
        if t + 0x17 <= data.len() {
            let d = &data[t + 0x0F..t + 0x17];
            id.internal_build_date = Some(String::from_utf8_lossy(d).trim().to_string());
        }
        if len >= 0x45 {
            id.cert_flag = Some(r.u8(t + 0x44)?);
        }
    }

    // 'S' 字符串表：3 字节条目 (u16 off, u8 len)。
    if let Some(s) = entries.iter().find(|e| e.id == b'S') {
        let t = img + usize::from(s.offset);
        let cnt = usize::from(s.len) / 3;
        for k in 0..cnt {
            let e = t + k * 3;
            if e + 3 > data.len() {
                break;
            }
            let off = usize::from(r.u16(e)?);
            let slen = usize::from(r.u8(e + 2)?);
            let Some(text) = data.get(img + off..img + off + slen) else {
                continue;
            };
            let text = String::from_utf8_lossy(text)
                .trim_end_matches('\0')
                .to_string();
            let first_line = text.lines().next().unwrap_or("").trim().to_string();
            if first_line.is_empty() {
                continue;
            }
            id.strings.push(text.trim_end().to_string());
            if id.message.is_none() {
                id.message = first_line.split(' ').next().map(str::to_string);
            }
            if id.board_id.is_none() && first_line.contains(" Board") {
                id.board_id = Some(first_line);
            }
        }
    }

    // x86 头 0x36 起："FT07/03/17"（2 字母 OEM 前缀 + MM/DD/YY）。
    if img + 0x40 <= data.len() {
        let raw = String::from_utf8_lossy(&data[img + 0x36..img + 0x40]).to_string();
        let rest = raw
            .trim_start_matches(|c: char| c.is_ascii_uppercase())
            .trim()
            .to_string();
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() == 3
            && parts
                .iter()
                .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_digit()))
        {
            id.build_date = Some(format!("20{}-{}-{}", parts[2], parts[0], parts[1]));
        }
    }
    Ok(id)
}

/// BIT 'i' InternalUse 全解（hexpat BitDataInternalUseV2 布局 + 13 卡实测
/// 校准）。字段偏移对 Maxwell 0x46/0x48、Pascal 0x5C、Turing 0x68、
/// Ampere/Ada 0x6E、Blackwell 0xA0 统一成立；长度不足的字段为 None，
/// 超出 0x60 的尾部（Blackwell 扩展区）原样携带。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InternalUseTable {
    pub table_offset: usize,
    pub token_len: u16,
    /// Version u32 的 BCD 段（与 BIT 'B' 同源："86.04.26.00"）。
    pub version: Option<String>,
    /// OemVersion u8（与 'B' Oem 合成完整版本尾）。
    pub oem_version: Option<u8>,
    /// Features u16 @0x05（编译特性位）。
    pub features: Option<u16>,
    /// P4MagicNumber u32 @0x07（Perforce checkin 号）。
    pub p4_magic: Option<u32>,
    /// BoardId u16 @0x0B。
    pub board_id: Option<u16>,
    /// bBuildDate[8] @0x0F（"MM/DD/YY"；与 x86 头发布日期不同源）。
    pub build_date: Option<String>,
    /// sNVChipSKU[3] @0x30 + mod @0x33。
    pub chip_sku: Option<String>,
    /// sNVProject[4] @0x34（实测 G600/G411/G152/3932…）。
    pub project: Option<String>,
    /// sNVProjectSKU[4] @0x38。
    pub project_sku: Option<String>,
    /// bNVBusinessCycle u8 @0x42。
    pub business_cycle: Option<u8>,
    /// bCertFlag u8 @0x44（消费卡 0x03、TITAN/P100 0x12 观测）。
    pub cert_flag: Option<u8>,
    /// alternateBoardId u16 @0x46（token ≥ 0x48 时）。
    pub alternate_board_id: Option<u16>,
    /// buildGuid 16B @0x48（Pascal 0x5C 表起有）。
    pub build_guid: Option<[u8; 16]>,
    /// minimumNetlistRev u16 @0x5C。
    pub min_netlist_rev: Option<u16>,
    /// revlock @0x5E/0x5F（minimum RM / current VBIOS）。
    pub revlock: Option<[u8; 2]>,
    /// ≥0x60 的扩展尾（Blackwell 0xA0 表：偏移量纲 u32 数组 + ASCII 片段，
    /// 语义未定案，原样携带）。
    pub tail_raw: Vec<u8>,
}

/// 定位并全解 BIT 'i' InternalUse 表（55AA 镜像基址自动探测）。
pub fn find_internal_use(data: &[u8]) -> Result<Option<InternalUseTable>, Error> {
    let img =
        find_image_base(data).ok_or_else(|| Error::from("internal-use: 55 AA base not found"))?;
    find_internal_use_at(data, img)
}

/// 同 [`find_internal_use`]，但镜像基址由调用方给出（Blackwell 容器的
/// 55AA 镜像链基址不是首个 55AA 候选）。
pub fn find_internal_use_at(data: &[u8], img: usize) -> Result<Option<InternalUseTable>, Error> {
    let sig = find_bit_signature(data, img)
        .ok_or_else(|| Error::from("internal-use: BIT signature not found"))?;
    let entries = bit_entries(data, sig, img)?;
    let Some(e) = entries.iter().find(|e| e.id == b'i') else {
        return Ok(None);
    };
    let t = img + usize::from(e.offset);
    let len = usize::from(e.len);
    if t + 4 > data.len() {
        return Ok(None);
    }
    let at = |a: usize| data.get(t + a).copied().unwrap_or(0);
    let rd16 = |a: usize| u16::from(at(a)) | u16::from(at(a + 1)) << 8;
    let rd32 = |a: usize| {
        u32::from(at(a))
            | u32::from(at(a + 1)) << 8
            | u32::from(at(a + 2)) << 16
            | u32::from(at(a + 3)) << 24
    };
    let ascii = |a: usize, n: usize| -> Option<String> {
        let s: String = (a..a + n)
            .map(|k| at(k) as char)
            .collect::<String>()
            .trim_end_matches(['\0', ' '])
            .to_string();
        let printable = s.chars().all(|c| c.is_ascii_graphic() || c == ' ');
        (!s.is_empty() && printable).then_some(s)
    };
    let mut out = InternalUseTable {
        table_offset: t,
        token_len: e.len,
        ..InternalUseTable::default()
    };
    // Version u32 @0：BCD 段（BE 字节序）。有效性校验：高字节应为 0x80-0x9F
    // 量级的芯片代（84/86/94/95/98 观测），排除全 0/GUID 式内容。
    let v = rd32(0);
    let hi = (v >> 24) & 0xFF;
    if (0x80..=0x9F).contains(&hi) {
        out.version = Some(format!(
            "{hi:02x}.{:02x}.{:02x}.{:02x}",
            (v >> 16) & 0xFF,
            (v >> 8) & 0xFF,
            v & 0xFF
        ));
        out.oem_version = Some(at(4));
    }
    if len >= 7 {
        out.features = Some(rd16(5));
    }
    if len >= 0x0B {
        out.p4_magic = Some(rd32(7));
    }
    if len >= 0x0D {
        out.board_id = Some(rd16(0x0B));
    }
    if len >= 0x17 {
        let d = &data[t + 0x0F..t + 0x17];
        let s = String::from_utf8_lossy(d).trim().to_string();
        if s.chars().all(|c| c.is_ascii_digit() || c == '/') {
            out.build_date = Some(s);
        }
    }
    if len >= 0x34 {
        out.chip_sku = ascii(0x30, 4);
    }
    if len >= 0x38 {
        out.project = ascii(0x34, 4);
    }
    if len >= 0x3C {
        out.project_sku = ascii(0x38, 4);
    }
    if len >= 0x43 {
        out.business_cycle = Some(at(0x42));
    }
    if len >= 0x45 {
        out.cert_flag = Some(at(0x44));
    }
    if len >= 0x48 {
        out.alternate_board_id = Some(rd16(0x46));
    }
    if len >= 0x58 {
        let mut g = [0u8; 16];
        for (k, b) in g.iter_mut().enumerate() {
            *b = at(0x48 + k);
        }
        out.build_guid = Some(g);
    }
    if len >= 0x5E {
        out.min_netlist_rev = Some(rd16(0x5C));
    }
    if len >= 0x60 {
        out.revlock = Some([at(0x5E), at(0x5F)]);
    }
    if len > 0x60 && t + len <= data.len() {
        out.tail_raw = data[t + 0x60..t + len].to_vec();
    }
    Ok(Some(out))
}

// ── 热/风扇策略表（PERF_PTRS modern 槽位；GPU-Z Thermal Limits 同源）─────

/// ThermalPolicy 条目（28B；温度 = u16 <<5 即 °C×32 定点）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThermalPolicyEntry {
    pub enabled: bool,
    /// +2 温度槽 A（°C）。GP104 enabled 条目实测 83 = GPU-Z "Rated"。
    pub temp_a_c: Option<u8>,
    /// +4 温度槽 B（°C）。GP104 实测 60。
    pub temp_b_c: Option<u8>,
    /// +6 温度槽 C（°C）。GP104 实测 92 = GPU-Z "Maximum"。
    pub temp_c_c: Option<u8>,
    /// +8 原始 u16（观测 200）。
    pub duty_like_raw: u16,
    /// +10 原始 u16（观测 0x4013 / 0xC013——低字节 0x13 恒定）。
    pub flags_raw: u16,
    /// +16 hysteresis 候选（观测 0x20=32）。
    pub hysteresis_raw: u16,
}

/// ThermalPolicy 表（PERF_PTRS +0x50；GP104 4 条目中 2 条 enabled）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThermalPolicyTable {
    pub table_offset: usize,
    pub entries: Vec<ThermalPolicyEntry>,
}

/// 定位并解析 ThermalPolicy 表（BIT 'P' +0x50，EFI 间隙重定位）。
pub fn find_thermal_policy(data: &[u8]) -> Result<Option<ThermalPolicyTable>, Error> {
    let Some(t) = perf_slot_target(data, 0x50)? else {
        return Ok(None);
    };
    if t + 6 > data.len() || data[t] != 0x10 {
        return Ok(None);
    }
    let r = Reader { data };
    let hdr = usize::from(r.u8(t + 1)?);
    let stride = usize::from(r.u8(t + 2)?);
    let cnt = usize::from(r.u8(t + 3)?);
    if stride != 28 {
        return Ok(None);
    }
    let mut out = Vec::new();
    for i in 0..cnt.min(16) {
        let e = t + hdr + i * stride;
        if e + stride > data.len() {
            break;
        }
        let temp = |a: usize| -> Option<u8> {
            let v = r.u16(e + a).ok()?;
            (v != 0 && v != 0xFFFF).then_some((v >> 5) as u8)
        };
        out.push(ThermalPolicyEntry {
            enabled: r.u8(e)? == 1,
            temp_a_c: temp(2),
            temp_b_c: temp(4),
            temp_c_c: temp(6),
            duty_like_raw: r.u16(e + 8)?,
            flags_raw: r.u16(e + 10)?,
            hysteresis_raw: r.u16(e + 16)?,
        });
    }
    Ok(Some(ThermalPolicyTable {
        table_offset: t,
        entries: out,
    }))
}

/// FanCooler 一个条目（26B；一个 fan cooler 控制器）。字节级定案（GP104/
/// GM200/1080Ti/P104 交叉验证）：+0x03=max duty%、**+0x0E=min RPM、
/// +0x10=max RPM**（0x10 处旧读法 = max，与 NVML Max RPM 同值；
/// TITAN X 1050/4800、GP104 0/4190）、+0x14=5000 常量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanCooler {
    /// +3 u8：max duty%（GP104 实测 100）。
    pub max_duty_percent: Option<u8>,
    /// +0x0E u16：min RPM（TITAN X 1050 / 1080Ti FE 1100 / GP104 0）。
    pub min_rpm: Option<u16>,
    /// +0x10 u16：max RPM（GP104 4190 / GM200 4800，与 NVML Max RPM 同值）。
    pub max_rpm: Option<u16>,
    /// +0x14 u16：观测常量 5000。
    pub const_5000: Option<u16>,
    pub raw: Vec<u8>,
}

/// FanCooler 表（PERF_PTRS +0x58）。**条目数 = fan cooler 控制器数候选**
/// （GP104 实测 1 条 → NVAPI get-fan-info Count: 1；多风扇卡预期多条，
/// 待实机样本验证）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanCoolerTable {
    pub table_offset: usize,
    pub coolers: Vec<FanCooler>,
}

/// 定位 FanCooler 表（BIT 'P' +0x58）。
pub fn find_fan_cooler(data: &[u8]) -> Result<Option<FanCoolerTable>, Error> {
    let Some(t) = perf_slot_target(data, 0x58)? else {
        return Ok(None);
    };
    if t + 4 > data.len() || data[t] != 0x10 {
        return Ok(None);
    }
    let r = Reader { data };
    let hdr = usize::from(r.u8(t + 1)?);
    let len = usize::from(r.u8(t + 2)?);
    let cnt = usize::from(r.u8(t + 3)?);
    if len < 20 || t + hdr + cnt * len > data.len() {
        return Ok(None);
    }
    let mut coolers = Vec::new();
    for i in 0..cnt.min(8) {
        let e = t + hdr + i * len;
        coolers.push(FanCooler {
            max_duty_percent: Some(r.u8(e + 3)?),
            min_rpm: Some(r.u16(e + 0x0E)?),
            max_rpm: Some(r.u16(e + 0x10)?),
            const_5000: Some(r.u16(e + 0x14)?),
            raw: data[e..e + len].to_vec(),
        });
    }
    Ok(Some(FanCoolerTable {
        table_offset: t,
        coolers,
    }))
}

/// FanCooler 表签名（表头 `10 04 1A 01` = ver/hdr4/len26/cnt1）。
const FAN_COOLER_SIG: [u8; 4] = [0x10, 0x04, 0x1A, 0x01];

/// 签名定位 FanCooler（跨代通用：Maxwell/Pascal 的 PERF 槽位图不同，
/// 但表头字节一致；GP104 @0x3080E、GM200 @0x9686 实测）。多条目卡预期
/// 逐条命中。Blackwell 的 P+0x58 槽（经 delta 重定位）同样指向此头。
pub fn find_fan_cooler_by_signature(data: &[u8]) -> FanCoolerTable {
    let mut coolers = Vec::new();
    let mut first_offset = None;
    let r = Reader { data };
    for t in 0..data.len().saturating_sub(4) {
        if data[t..t + 4] != FAN_COOLER_SIG {
            continue;
        }
        // 防同一条目重复计数：条目区 26B + 表头 4B，跳过本次命中覆盖区。
        if let Some(prev) = first_offset
            && t < prev + 30
        {
            continue;
        }
        let e = t + 4;
        if e + 0x16 > data.len() {
            continue;
        }
        let duty = r.u8(e + 3).unwrap_or(0);
        let min_rpm = r.u16(e + 0x0E).unwrap_or(0);
        let max_rpm = r.u16(e + 0x10).unwrap_or(0);
        // 合理性门：duty ≤ 100、RPM ≤ 8000、min ≤ max。
        if duty > 100 || max_rpm == 0 || max_rpm > 8000 || min_rpm > max_rpm {
            continue;
        }
        if first_offset.is_none() {
            first_offset = Some(t);
        }
        coolers.push(FanCooler {
            max_duty_percent: Some(duty),
            min_rpm: Some(min_rpm),
            max_rpm: Some(max_rpm),
            const_5000: Some(r.u16(e + 0x14).unwrap_or(0)),
            raw: data[e..e + 0x1A].to_vec(),
        });
        if coolers.len() >= 8 {
            break;
        }
    }
    FanCoolerTable {
        table_offset: first_offset.unwrap_or(0),
        coolers,
    }
}

/// FanPolicy / FanTest 等原始风扇表（PERF_PTRS +0x5C / +0x64）。字段布局
/// 未 RE，原样捕获备查（FanPolicy 53B 条目含曲线状数据，FanTest 5B）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFanTable {
    pub table_offset: usize,
    pub ver: u8,
    pub entry_len: u8,
    pub entry_count: u8,
    /// 每条目原始字节。
    pub entries: Vec<Vec<u8>>,
}

fn find_raw_fan_table(data: &[u8], slot: usize) -> Result<Option<RawFanTable>, Error> {
    let Some(t) = perf_slot_target(data, slot)? else {
        return Ok(None);
    };
    if t + 4 > data.len() || data[t] != 0x10 {
        return Ok(None);
    }
    let r = Reader { data };
    let hdr = usize::from(r.u8(t + 1)?);
    let entry_len = usize::from(r.u8(t + 2)?);
    let cnt = usize::from(r.u8(t + 3)?);
    if entry_len == 0 || t + hdr + cnt * entry_len > data.len() {
        return Ok(None);
    }
    let entries = (0..cnt.min(16))
        .map(|i| {
            let e = t + hdr + i * entry_len;
            data[e..e + entry_len].to_vec()
        })
        .collect();
    Ok(Some(RawFanTable {
        table_offset: t,
        ver: r.u8(t)?,
        entry_len: r.u8(t + 2)?,
        entry_count: r.u8(t + 3)?,
        entries,
    }))
}

/// FanPolicy 条目内的风扇曲线点：温度 = u16 定点 ÷32（°C，允许 0.5 粒度，
/// 如 GP104 启停边界 49.9°C），RPM 原值。六样本交叉验证（GP104 G1/GP104
/// FE/P100/GM200/Palit 980/Kepler 680）：
/// - G1 1070: (49.9, 0), (50, 800), (90, 4190) —— **风扇启停（0 dB）曲线**
/// - P100:    (42, 1400), (70, 2600), (82, 4800) —— 服务器常转
/// - Palit 980: (60, 0), (80, 1600), (90, 2200)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FanCurvePoint {
    pub temp_c: f64,
    pub rpm: u16,
}

/// FanPolicy 表（PERF_PTRS +0x5C；53B 条目）。曲线 = 条目尾部 3 个
/// (温度<<5, RPM) u16 对，通过"温度与 RPM 双严格递增 + 温度 20..120°C"
/// 窗口扫描锚定（头部标志字节可变长导致固定偏移不可用——Palit 与
/// GP104 相差 1 字节）。
#[derive(Debug, Clone, PartialEq)]
pub struct FanPolicyTable {
    pub table_offset: usize,
    pub entry_len: u8,
    /// 每条目一条曲线（观测 cnt=1）。
    pub curves: Vec<Vec<FanCurvePoint>>,
    /// 每条目曲线起始绝对偏移（诊断用）。
    pub curve_offsets: Vec<usize>,
    /// 每条目原始字节。
    pub raw_entries: Vec<Vec<u8>>,
}

/// 在条目字节内扫描第一个合法的 3 点 (温度<<5, RPM) 曲线窗口。
fn scan_fan_curve(entry: &[u8]) -> Option<(usize, Vec<FanCurvePoint>)> {
    if entry.len() < 12 {
        return None;
    }
    let u16at = |i: usize| u16::from(entry[i]) | u16::from(entry[i + 1]) << 8;
    // 曲线窗口不与条目起始 2 字节对齐（观测奇数偏移 21/17），逐字节扫。
    for start in 0..=entry.len() - 12 {
        let temps = [0, 4, 8].map(|o| u16at(start + o));
        let rpms = [0, 4, 8].map(|o| u16at(start + o + 2));
        let temp_ok = temps.iter().all(|t| (640..=3840).contains(t)) // 20..120 °C
            && temps[0] < temps[1]
            && temps[1] < temps[2];
        let rpm_ok = rpms[0] < rpms[1] && rpms[1] < rpms[2] && rpms[2] <= 0x2000;
        if temp_ok && rpm_ok {
            let points = (0..3)
                .map(|i| FanCurvePoint {
                    temp_c: f64::from(temps[i]) / 32.0,
                    rpm: rpms[i],
                })
                .collect();
            return Some((start, points));
        }
    }
    None
}

/// 定位并解析 FanPolicy 表（BIT 'P' +0x5C）。
pub fn find_fan_policy(data: &[u8]) -> Result<Option<FanPolicyTable>, Error> {
    let Some(t) = perf_slot_target(data, 0x5C)? else {
        return Ok(None);
    };
    if t + 4 > data.len() || data[t] != 0x10 {
        return Ok(None);
    }
    let r = Reader { data };
    let hdr = usize::from(r.u8(t + 1)?);
    let entry_len = usize::from(r.u8(t + 2)?);
    let cnt = usize::from(r.u8(t + 3)?);
    if entry_len < 12 || t + hdr + cnt * entry_len > data.len() {
        return Ok(None);
    }
    let mut curves = Vec::new();
    let mut curve_offsets = Vec::new();
    let mut raw_entries = Vec::new();
    for i in 0..cnt.min(8) {
        let e = t + hdr + i * entry_len;
        let entry = &data[e..e + entry_len];
        raw_entries.push(entry.to_vec());
        match scan_fan_curve(entry) {
            Some((start, points)) => {
                curve_offsets.push(e + start);
                curves.push(points);
            }
            None => {
                curve_offsets.push(0);
                curves.push(Vec::new());
            }
        }
    }
    Ok(Some(FanPolicyTable {
        table_offset: t,
        entry_len: r.u8(t + 2)?,
        curves,
        curve_offsets,
        raw_entries,
    }))
}

/// FanPolicy 表签名（表头 `10 05 35 01` = ver/hdr5/len53/cnt1）。
const FAN_POLICY_SIG: [u8; 4] = [0x10, 0x05, 0x35, 0x01];

/// 签名定位 FanPolicy（跨代通用：Maxwell PERF+0x5C 槽位在 Pascal 上是
/// 压缩数据、GM200 布局错位，但表头字节跨代一致——GP104 @0x3082E、
/// GM200 @0x96A4 实测；TITAN X stock 曲线 (61,1050)/(81.3,1900)/(91,4800)）。
/// 多条目/多表全部收集。Blackwell 的 P+0x5C 槽（经 delta 重定位）同头。
pub fn find_fan_policy_by_signature(data: &[u8]) -> FanPolicyTable {
    let mut curves = Vec::new();
    let mut curve_offsets = Vec::new();
    let mut raw_entries = Vec::new();
    let mut first_offset = None;
    for t in 0..data.len().saturating_sub(4) {
        if data[t..t + 4] != FAN_POLICY_SIG {
            continue;
        }
        if let Some(prev) = first_offset
            && t < prev + 57
        {
            continue;
        }
        // 表头 ver/hdr5/len0x35/cnt1；条目 = t+5 起 53B。
        if t + 5 + 53 > data.len() {
            continue;
        }
        let entry = &data[t + 5..t + 5 + 53];
        if first_offset.is_none() {
            first_offset = Some(t);
        }
        raw_entries.push(entry.to_vec());
        match scan_fan_curve(entry) {
            Some((start, points)) => {
                curve_offsets.push(t + 5 + start);
                curves.push(points);
            }
            None => {
                curve_offsets.push(0);
                curves.push(Vec::new());
            }
        }
    }
    FanPolicyTable {
        table_offset: first_offset.unwrap_or(0),
        entry_len: 0x35,
        curves,
        curve_offsets,
        raw_entries,
    }
}

/// 定位 FanTest 表（BIT 'P' +0x64；5B 条目）。
pub fn find_fan_test(data: &[u8]) -> Result<Option<RawFanTable>, Error> {
    find_raw_fan_table(data, 0x64)
}

// ── 功率表（CPR `get_power_table_list` 同款锚点扫描）─────────────────────

/// 功率表平台分类（按锚点前的"无用功率值"判别）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerPlatform {
    /// useless == 0：标准移动卡，target/limit 在锚后。
    Mobile,
    /// 0 < useless < 5000 mW：低功耗移动卡（P600m/MX150 一族），需二次跳表。
    MobileLowPower,
    /// useless ≥ 5000 mW：桌面卡，target/limit 在锚前。
    Desktop,
}

impl PowerPlatform {
    /// 显示名。
    pub fn name(self) -> &'static str {
        match self {
            Self::Mobile => "mobile",
            Self::MobileLowPower => "mobile-low-power",
            Self::Desktop => "desktop",
        }
    }
}

/// 功率滑条（enable/disable 标志字节组）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerSlider {
    /// true = 滑条启用（`0F 02 FF FF FF`），false = 禁用/未知变体。
    pub enabled: bool,
    /// 绝对偏移。
    pub offset: usize,
}

/// 一份功率表（双镜像 ROM 每镜像一份）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerTable {
    pub platform: PowerPlatform,
    /// `28 46 0F 00 28 46 0F` 锚点绝对偏移。
    pub anchor_offset: usize,
    /// 最小功率（mW；desktop 布局的 po-11 字段——CPR 仅用作平台判别的
    /// "无用功率值"，实为 min TDP：GP104 实测 90000 mW = GPU-Z Minimum
    /// 90.0 W；mobile 布局该值为 0 哨兵，True min 位置未知 → None）。
    pub min_mw: Option<u32>,
    /// target power（mW）。
    pub target_mw: u32,
    pub target_offset: usize,
    /// max power（mW）。
    pub limit_mw: u32,
    pub limit_offset: usize,
    pub sliders: Vec<PowerSlider>,
}

const POWER_ANCHOR: [u8; 7] = [0x28, 0x46, 0x0F, 0x00, 0x28, 0x46, 0x0F];
const POWER_00100: [u8; 3] = [0x00, 0x01, 0x00];

fn find_all(data: &[u8], needle: &[u8], start: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos < data.len() {
        let Some(found) = data[pos..]
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| pos + p)
        else {
            break;
        };
        out.push(found);
        pos = found + 1;
    }
    out
}

fn le32(data: &[u8], a: usize) -> u32 {
    u32::from(data.get(a).copied().unwrap_or(0))
        | u32::from(data.get(a + 1).copied().unwrap_or(0)) << 8
        | u32::from(data.get(a + 2).copied().unwrap_or(0)) << 16
        | u32::from(data.get(a + 3).copied().unwrap_or(0)) << 24
}

/// 扫描全部功率表。定位/判别逻辑与 CPR 逐字一致（三种平台布局）；
/// `00 01 00` 重复计数不足时跳过该锚（返回空 Vec = 未找到，非错误）。
pub fn find_power_tables(data: &[u8]) -> Vec<PowerTable> {
    let mut out = Vec::new();
    for anchor in find_all(data, &POWER_ANCHOR, 0) {
        // 滑条：锚点 ±200B 窗口内的 4 种标志字节组。
        let w0 = anchor.saturating_sub(200);
        let w1 = (anchor + 200).min(data.len());
        let mut sliders = Vec::new();
        for (pattern, enabled) in [
            ([0x0F, 0x02, 0xFF, 0xFF, 0xFF], true),
            ([0x0F, 0xFF, 0xFF, 0xFF, 0x02], false),
            ([0x0F, 0x02, 0xFF, 0xFF, 0x02], false),
            ([0x0F, 0xFF, 0xFF, 0xFF, 0x00], false),
        ] {
            for offset in find_all(data, &pattern, w0) {
                if offset + pattern.len() > w1 {
                    break;
                }
                sliders.push(PowerSlider { enabled, offset });
            }
        }

        // 功率值：锚后 200B 内第 3 个 `00 01 00`。
        let Some(po) = find_all(data, &POWER_00100, anchor)
            .into_iter()
            .take_while(|p| *p < anchor + 200)
            .nth(2)
        else {
            continue;
        };
        if po < 11 || po + 15 > data.len() {
            continue;
        }
        let useless = le32(data, po - 11);
        let (platform, target_offset, limit_offset) = if useless == 0 {
            (PowerPlatform::Mobile, po + 7, po + 11)
        } else if useless < 5000 {
            // 低功耗移动卡：po 后 800B 内第 11 个 `00 01 00` 再跳 20/24。
            let Some(lp) = find_all(data, &POWER_00100, po)
                .into_iter()
                .take_while(|p| *p < po + 800)
                .nth(10)
            else {
                continue;
            };
            (PowerPlatform::MobileLowPower, lp + 20, lp + 24)
        } else {
            (PowerPlatform::Desktop, po - 7, po - 3)
        };
        if limit_offset + 4 > data.len() {
            continue;
        }
        out.push(PowerTable {
            platform,
            anchor_offset: anchor,
            min_mw: (platform == PowerPlatform::Desktop).then_some(useless),
            target_mw: le32(data, target_offset),
            target_offset,
            limit_mw: le32(data, limit_offset),
            limit_offset,
            sliders,
        });
    }
    out
}

/// 功率前缀三元组一条：签名 `21 00`/`21 03` 后连续的
/// `[min u32][target u32][limit u32]`（mW）。13 卡实测（递增三元组 +
/// 文件名瓦数逐一吻合）：TITAN X 150/250/275、GM204 100/180/225、
/// 1080Ti 125/250/375、P104 90/180/217、GP104 90/180/200。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerTriple {
    /// `21 xx` 签名所在偏移。
    pub offset: usize,
    /// 签名字节（0x2100 桌面主流 / 0x2103 GM204 观测）。
    pub prefix: [u8; 2],
    pub min_mw: u32,
    pub target_mw: u32,
    pub limit_mw: u32,
}

/// 功率前缀三元组扫描（Maxwell/Pascal 专用补充——GM204/TITAN X 没有
/// CPR `28 46 0F` 双 u32 锚，是旧扫描 miss 的字节根因；Turing+ 上不命中，
/// 继续走 find_power_tables）。
pub fn find_power_triples(data: &[u8]) -> Vec<PowerTriple> {
    let mut out = Vec::new();
    for prefix in [[0x21u8, 0x00], [0x21, 0x03]] {
        for offset in find_all(data, &prefix, 0) {
            let base = offset + 2;
            if base + 12 > data.len() {
                continue;
            }
            let min = le32(data, base);
            let target = le32(data, base + 4);
            let limit = le32(data, base + 8);
            // 合理性门：严格递增 + mW 量级。
            if !(5_000..=2_000_000).contains(&min)
                || min >= target
                || target >= limit
                || limit > 2_000_000
            {
                continue;
            }
            out.push(PowerTriple {
                offset,
                prefix,
                min_mw: min,
                target_mw: target,
                limit_mw: limit,
            });
        }
    }
    out.sort_by_key(|t| t.offset);
    out
}

// ── 热/风扇表（nouveau nvkm/subdev/bios/therm.c 语义逐字移植）────────────

/// 风扇控制模式（nouveau NVBIOS_THERM_FAN_*；枚举序 = 覆写优先级，
/// Linear < Trip < Other，与 nouveau 的 "if mode > X" 比较一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FanMode {
    /// type 0x46 线性区间。
    Linear,
    /// type 0x24/0x25 trip 点曲线。
    Trip,
    /// 无风扇条目（Fermi+ 按 Linear 默认处理）。
    Other,
}

impl FanMode {
    /// 显示名。
    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Trip => "trip",
            Self::Other => "other",
        }
    }
}

/// 风扇 trip 点（阶梯曲线一档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanTrip {
    pub temp_c: u8,
    pub hysteresis: u8,
    /// 风扇占空比（%）。
    pub duty_percent: u8,
}

/// 温度阈值（(v & 0xff0) >> 4 / v & 0xf 打包解码）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TempThreshold {
    pub temp_c: u8,
    pub hysteresis: u8,
}

/// 热传感器标定（type 0x01/0x10-0x13）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ThermSensor {
    /// type 0x01：((s8)byte@+2) / 2。
    pub offset_constant: Option<i16>,
    pub offset_num: Option<u16>,
    pub offset_den: Option<u16>,
    pub slope_mult: Option<u16>,
    pub slope_div: Option<u16>,
}

/// 四个温度阈值（critical / down_clock / fan_boost / shutdown）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ThermThresholds {
    /// type 0x04。
    pub critical: Option<TempThreshold>,
    /// type 0x07。
    pub down_clock: Option<TempThreshold>,
    /// type 0x08。
    pub fan_boost: Option<TempThreshold>,
    /// type 0x32。
    pub shutdown: Option<TempThreshold>,
}

/// 未匹配 nouveau 已知 tag 的原始条目（校准与将来扩展用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThermalRawEntry {
    pub tag: u8,
    pub raw: u16,
    pub offset: usize,
}

/// BIT 'P' 热表解析结果（nouveau therm.c 语义）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VbiosThermal {
    pub table_offset: usize,
    pub ver: u8,
    pub entry_len: u8,
    pub entry_count: u8,
    pub fan_mode: Option<FanMode>,
    pub min_duty: Option<u8>,
    pub max_duty: Option<u8>,
    pub pwm_freq: Option<u16>,
    pub linear_min_temp: Option<u8>,
    pub linear_max_temp: Option<u8>,
    pub bump_period: Option<u16>,
    pub slow_down_period: Option<u16>,
    pub trips: Vec<FanTrip>,
    pub thresholds: ThermThresholds,
    pub sensor: ThermSensor,
    pub unknown_entries: Vec<ThermalRawEntry>,
}

const THERM_DUTY_LUT: [u8; 16] = [0, 0, 25, 0, 40, 0, 50, 0, 75, 0, 85, 0, 100, 0, 100, 0];

/// 定位 BIT 'P' 热表并按 nouveau 语义解析。
///
/// 指针位置：P v2 → P 表 +0x10，P v1 → +0x0C。**返回 Ok(None) = 指针为零**
/// ——GP104 Pascal 实测两块 ROM 均如此：Pascal 把热/风扇表撤出了 CPU 可见
/// BIT（PMU/FW 承载），Maxwell（GM200）有表但仅含传感器条目，无风扇曲线。
/// 未识别 tag 收进 [`VbiosThermal::unknown_entries`] 供将来校准。
pub fn find_thermal(data: &[u8]) -> Result<Option<VbiosThermal>, Error> {
    let img = find_image_base(data).ok_or_else(|| Error::from("thermal: 55 AA base not found"))?;
    let r = Reader { data };
    let sig = find_bit_signature(data, img)
        .ok_or_else(|| Error::from("thermal: BIT signature not found"))?;
    let entries = bit_entries(data, sig, img)?;
    let p = entries
        .iter()
        .find(|e| e.id == b'P')
        .ok_or_else(|| Error::from("thermal: BIT 'P' entry not found"))?;
    let ptab = img + usize::from(p.offset);
    // v1 → +0x0C，v2 → +0x10（nouveau therm_table）。
    let ptr_addr = ptab + if p.ver == 1 { 0x0C } else { 0x10 };
    let ptr = r.u32(ptr_addr)?;
    if ptr == 0 {
        return Ok(None);
    }
    let t = img + usize::try_from(ptr).map_err(|_| Error::from("thermal: bad pointer"))?;
    if t + 4 > data.len() {
        return Ok(None);
    }

    let mut th = VbiosThermal {
        table_offset: t,
        ver: r.u8(t)?,
        entry_len: r.u8(t + 2)?,
        entry_count: r.u8(t + 3)?,
        ..VbiosThermal::default()
    };
    let hdr = usize::from(r.u8(t + 1)?);
    let stride = usize::from(th.entry_len).max(3);
    // 双遍合一：type-tag 分派（sensor 遍门控 thrs_section/sensor_section）。
    let mut thrs_section: i16 = 0;
    let mut sensor_section: i16 = -1;
    for idx in 0..usize::from(th.entry_count) {
        let e = t + hdr + idx * stride;
        if e + 3 > data.len() {
            break;
        }
        let tag = r.u8(e)?;
        let value = r.u16(e + 1)?;
        match tag {
            // ── sensor 遍 ──
            0x00 => {
                thrs_section = value as i16;
                if value > 0 {
                    break; // ambient 段，nouveau 直接终止
                }
            }
            0x01 => {
                sensor_section += 1;
                if sensor_section == 0 {
                    th.sensor.offset_constant = Some(i16::from(r.u8(e + 2)? as i8) / 2);
                }
            }
            0x04 | 0x07 | 0x08 | 0x32 if thrs_section == 0 => {
                let thr = TempThreshold {
                    temp_c: ((value & 0x0FF0) >> 4) as u8,
                    hysteresis: (value & 0xF) as u8,
                };
                match tag {
                    0x04 => th.thresholds.critical = Some(thr),
                    0x07 => th.thresholds.down_clock = Some(thr),
                    0x08 => th.thresholds.fan_boost = Some(thr),
                    _ => th.thresholds.shutdown = Some(thr),
                }
            }
            0x10 if sensor_section == 0 => th.sensor.offset_num = Some(value),
            0x11 if sensor_section == 0 => th.sensor.offset_den = Some(value),
            0x12 if sensor_section == 0 => th.sensor.slope_mult = Some(value),
            0x13 if sensor_section == 0 => th.sensor.slope_div = Some(value),
            // ── fan 遍 ──
            0x22 => {
                th.min_duty = Some((value & 0xFF) as u8);
                th.max_duty = Some((value >> 8) as u8);
            }
            0x24 => {
                th.trips.push(FanTrip {
                    temp_c: ((value & 0x0FF0) >> 4) as u8,
                    hysteresis: (value & 0xF) as u8,
                    duty_percent: THERM_DUTY_LUT[usize::from(value >> 12)],
                });
                if th.fan_mode.is_none_or(|m| m > FanMode::Trip) {
                    th.fan_mode = Some(FanMode::Trip);
                }
            }
            0x25 => {
                if let Some(last) = th.trips.last_mut() {
                    last.duty_percent = value as u8;
                }
            }
            0x26 if th.pwm_freq.is_none() => th.pwm_freq = Some(value),
            0x3B => th.bump_period = Some(value),
            0x3C => th.slow_down_period = Some(value),
            0x46 => {
                if th.fan_mode.is_none_or(|m| m > FanMode::Linear) {
                    th.fan_mode = Some(FanMode::Linear);
                }
                th.linear_min_temp = Some(r.u8(e + 1)?);
                th.linear_max_temp = Some(r.u8(e + 2)?);
            }
            _ => th.unknown_entries.push(ThermalRawEntry {
                tag,
                raw: value,
                offset: e,
            }),
        }
    }
    // Fermi+ 默认线性（nouveau: card_type >= NV_C0 && mode == OTHER）。
    if th.fan_mode.is_none() {
        th.fan_mode = Some(FanMode::Linear);
    }
    Ok(Some(th))
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
        // entry_len 5, entry_cnt 8（形状照抄 GM200 实测；8 条目满足
        // parse_boost_ladder 的 record-shape 硬化下限，数值缩短）
        put(&mut d, 0x780, &[0x10, 0x09, 0x08, 0x01, 0x05, 0x08]);
        put(&mut d, 0x789, &u16le(0x01E0)); // P0 编码
        put(&mut d, 0x789 + 3, &[0x01]); // 边界在阶梯 idx 1
        for k in 0..8u16 {
            let e = 0x791 + k * 5;
            put(&mut d, usize::from(e), &u16le(0x04A6 + k * 25)); // 595.0 + 12.5k MHz
            put(&mut d, usize::from(e) + 4, &[0]); // vmap[0]
        }
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
        assert_eq!(ladder.entries.len(), 8);
        assert_eq!(ladder.entries[0].freq_mhz_x2, 0x04A6);
        assert_eq!(ladder.entries[7].freq_mhz_x2, 0x04A6 + 7 * 25);
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

    // ── VP 表 ───────────────────────────────────────────────────────────

    /// 构造最小合成 Pascal VP 表：2 profile（0x08、0x07）+ 头 `20 HLEN 01` +
    /// 3 阶梯点（324.0 idle / 1202.5 / 1911.0 max）+ mem = 0x86D8
    /// （旗标 0x8000 + 1752 MHz）。数值单位按模块文档：u32 = MHz×2^15
    /// （1911.0 MHz → 0x03BB_8000，与 CPR "+32768" 观测一致）。
    /// ladder 起点 = 头位置 + HLEN + 1，分母 0x0F 在其前一字节。
    fn synthetic_vp_h(hlen: u8) -> Vec<u8> {
        let u32le = |v: u32| v.to_le_bytes();
        let mut d = vec![0u8; 0x2000];
        let put = |d: &mut Vec<u8>, a: usize, b: &[u8]| d[a..a + b.len()].copy_from_slice(b);

        // 两条 57B profile 与头连续：ID 自低到高向头排列（0x07 文件序最前、
        // 离头最远；0x0F 紧贴头）——CPR 反向回走至 0x07 停止。
        let profile = |d: &mut Vec<u8>, a: usize, id: u8, l1: u16, l2: u16, l3: u16| {
            put(d, a, &[id]);
            put(d, a + 7, &l1.to_le_bytes());
            put(d, a + 13, &l2.to_le_bytes());
            put(d, a + 15, &0x0B_D8u16.to_le_bytes()); // mem_short（MHz 直存）
            put(d, a + 19, &0x86D8u16.to_le_bytes()); // mem_long：旗标+1752
            put(d, a + 25, &l3.to_le_bytes());
        };
        let header = 0x3A2;
        profile(&mut d, header - 2 * 57, 0x07, 660, 640, 620);
        profile(&mut d, header - 57, 0x08, 900, 880, 860);

        // 头 + 0x0F 分母 + 3 条 41B 阶梯（324.0 / 1202.5 / 1911.0）+ 0 终止
        put(&mut d, header, &[0x20, hlen, 0x01]);
        let ladder = header + usize::from(hlen) + 1;
        put(&mut d, ladder - 1, &[0x0F]);
        // mem clock：首条目 +8（旗标 0x8000 + 1752 MHz）
        put(&mut d, ladder + 8, &0x86D8u16.to_le_bytes());
        // raw 编码随世代：Pascal = MHz×2^15 定点；Turing+ = 低 14 位直存 MHz。
        let raws: [u32; 3] = if hlen == 0x10 || hlen == 0x12 {
            [
                (324.0f64 * 32768.0) as u32,
                (1202.5f64 * 32768.0) as u32,
                (1911.0f64 * 32768.0) as u32,
            ]
        } else {
            [324, 1202, 1911]
        };
        for (i, raw) in raws.iter().enumerate() {
            let off = ladder + i * 41;
            put(&mut d, off, &u32le(*raw));
            if i + 1 < 3 {
                put(&mut d, off + 41 - 1, &[0x0F]);
            }
        }
        d
    }

    fn synthetic_vp() -> Vec<u8> {
        synthetic_vp_h(0x10)
    }

    #[test]
    fn vp_synthetic_pascal() {
        let d = synthetic_vp();
        let tables = find_vp_tables(&d);
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert_eq!(t.generation, VpGeneration::Pascal);
        assert_eq!(t.header_offset, 0x3A2);
        assert_eq!(t.ladder_offset, t.header_offset + 0x11);

        // ladder：3 点；u32 = MHz×2^15（324.0 → 0x00A2_0000，高 u16 = 162 =
        // floor(MHz/2)，即 CPR 的 "clock_value/2"）
        assert_eq!(t.entries.len(), 3);
        assert_eq!(t.entries[0].raw, 324 * 32768);
        assert_eq!(t.entries[0].raw >> 16, 162);
        assert_eq!(t.entries[0].denominator, 0x0F);
        assert_eq!(t.entries[0].freq_mhz(), 324.0);
        assert_eq!(t.entries[1].freq_mhz(), 1202.5);
        assert_eq!(t.entries[2].freq_mhz(), 1911.0);
        // 奇数 MHz → 低 u16 = 0x8000（CPR 的 "flags" 之谜）
        assert_eq!(t.entries[2].raw & 0xFFFF, 0x8000);

        // profiles：回走至 0x07，反转为文件序 [0x07, 0x08]；limit 半 MHz ×2
        assert_eq!(t.profiles.len(), 2);
        assert_eq!(t.profiles[0].id, 0x07);
        assert_eq!(t.profiles[1].id, 0x08);
        assert_eq!(t.profiles[1].limit_raw, [900, 880, 860]);
        assert_eq!(t.profiles[1].limit_mhz(), [1800.0, 1760.0, 1720.0]);
        assert!(!t.profiles[0].is_empty());
        assert_eq!(t.non_empty_profiles(), 2);

        // mem：&0x3FFF 剥旗标
        assert_eq!(t.mem_clock_raw, 0x86D8);
        assert_eq!(t.mem_clock_mhz(), 1752);
    }

    #[test]
    fn vp_generation_dispatch() {
        // Turing+ 头（0x13）→ 65B profile；校准数据到位前仅断言布局分派。
        let d = synthetic_vp_h(0x13);
        let tables = find_vp_tables(&d);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].generation, VpGeneration::TuringPlus);
        assert_eq!(tables[0].generation.profile_len(), 65);

        // 头长字节落在编码外 → 不识别
        let mut d2 = synthetic_vp();
        d2[0x3A2 + 1] = 0x11;
        assert!(find_vp_tables(&d2).is_empty());

        // 分母不是 0x0F → 拒收
        let mut d3 = synthetic_vp();
        d3[0x3A2 + 0x11 - 1] = 0x0E;
        assert!(find_vp_tables(&d3).is_empty());
    }

    #[test]
    fn vp_rejects_random_bytes() {
        // 全 0xFF / 全 0x00 / 随机小文件均不伪命中
        assert!(find_vp_tables(&[0xFF; 0x1000]).is_empty());
        assert!(find_vp_tables(&[0x00; 0x1000]).is_empty());
        assert!(find_vp_tables(&[0x20, 0x10, 0x01]).is_empty());
    }

    /// 机会性真文件测试：reverse/ 下若有 Pascal ROM（编程器 dump）则校验
    /// VP 解析不变量。数值级逐位校准待第一份实机 dump 落库后补入。
    #[test]
    fn vp_real_roms_when_present() {
        for path in [
            "../reverse/P4000.rom",
            "../reverse/pascal.rom",
            "../reverse/P100.rom",
        ] {
            let Ok(d) = std::fs::read(path) else {
                eprintln!("skip: {path} not present");
                continue;
            };
            let tables = find_vp_tables(&d);
            let Some(t) = tables.first() else {
                panic!("{path}: no VP table found (Pascal dump expected)");
            };
            assert_eq!(t.generation, VpGeneration::Pascal, "{path}");
            assert!(!t.entries.is_empty(), "{path} ladder empty");
            for e in &t.entries {
                let f = e.freq_mhz();
                assert!((100.0..2500.0).contains(&f), "{path} freq {f} out of range");
                assert_eq!(e.denominator, 0x0F, "{path} denominator");
            }
            // 阶梯单调不减（idle → max）
            for w in t.entries.windows(2) {
                assert!(
                    w[1].freq_mhz() >= w[0].freq_mhz(),
                    "{path} ladder not monotonic"
                );
            }
            let last = t.profiles.first().expect("{path} no profiles");
            assert_eq!(last.id, 0x07, "{path} first profile should be 0x07");
            assert_eq!(
                t.profiles.last().expect("{path} no profiles").id,
                0x0F,
                "{path} last profile should be boost 0x0F"
            );
            let mem = t.mem_clock_mhz();
            assert!((500..8000).contains(&mem), "{path} mem {mem} out of range");
        }
    }

    /// GP100 服务器卡变体（P100 dump 逐位实测，e84d2a98…，264KB ×2 份
    /// 一致）：VP 表**仅 ladder、无 profile 数组**（回走 10 步无 0x07）；
    /// 2 频点 × 2 副本，u32/32768 精确命中官方规格（base 名义 1190、boost
    /// 名义 1328）；HBM2 无消费级 mem 字段（raw 0）。服务器卡无 GPU Boost
    /// 阶梯，固定频点，与 4 条形态自洽。
    #[test]
    fn vp_p100_server_variant_when_present() {
        let Ok(d) = std::fs::read("../reverse/p100-vbios.rom") else {
            eprintln!("skip: ../reverse/p100-vbios.rom not present");
            return;
        };
        let tables = find_vp_tables(&d);
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert_eq!(t.generation, VpGeneration::Pascal);
        assert_eq!(t.header_offset, 0xB160);
        assert_eq!(t.ladder_offset, 0xB171);
        // 仅阶梯变体
        assert!(t.profiles.is_empty(), "GP100 has no profile array");

        assert_eq!(t.entries.len(), 4);
        assert_eq!(t.entries[0].raw, 0x0252_C94B);
        assert_eq!(t.entries[1].raw, 0x0252_C94B);
        assert_eq!(t.entries[2].raw, 0x0298_4A61);
        assert_eq!(t.entries[3].raw, 0x0298_4A61);
        let base = t.entries[0].freq_mhz();
        let boost = t.entries[2].freq_mhz();
        assert!((1189.0..=1190.0).contains(&base), "base {base}");
        assert!((1328.0..=1329.0).contains(&boost), "boost {boost}");
        for e in &t.entries {
            assert_eq!(e.denominator, 0x0F);
        }
        // HBM2：无消费级 mem 字段
        assert_eq!(t.mem_clock_raw, 0);
    }

    /// Quadro Kepler（K4000/GK106）的 GPU Boost 阶梯：P+0x34 头
    /// `10 06 04 04 05 3f`——mark **4B**×4（Maxwell/780Ti 为 8B，旧门禁
    /// 因此拒收 = "读不到 VFTable" 的根因）+ 5B×63 点。16 个非零点
    /// 324.0..810.5 MHz 与 KeplerBiosTweaker "Boost Clocks" 网格逐值一致；
    /// 电压经 vmap（P+0x20，ver 0x20/34B 条目）解析，顶点 875..975mV
    /// 与 KBT Voltage Table 页一致。魔改镜像（活卡 dump）阶梯顶被抬到
    /// 1045.5 MHz / 1312.5mV。
    #[test]
    fn kepler_quadro_boost_ladder_when_present() {
        const K4000: &str = "../reverse/K4000_original.rom";
        let Ok(d) = std::fs::read(K4000) else {
            eprintln!("skip: {K4000} not present");
            return;
        };
        let vb = parse(&d).expect("legacy parse");
        let ladder = vb.boost_ladder.as_ref().expect("K4000 boost ladder");
        assert_eq!(ladder.ver, 0x10);
        assert_eq!(ladder.entries.len(), 63);
        let nonzero: Vec<u16> = ladder
            .entries
            .iter()
            .map(|e| e.freq_mhz_x2)
            .filter(|f| *f != 0)
            .collect();
        assert_eq!(nonzero.len(), 16);
        assert_eq!(nonzero[0], 648, "324.0 MHz x2");
        assert_eq!(*nonzero.last().expect("nonempty"), 1621, "810.5 MHz x2");
        // 顶点电压 = vmap[26]（875..975 mV，与 KBT Voltage Table 一致）。
        let top = &ladder.entries[15];
        assert_eq!(top.freq_mhz_x2, 1621);
        let (vmin, vmax) = vb.ladder_voltage_uv(top.vmap_index).expect("vmap");
        assert_eq!((vmin, vmax), (875_000, 975_000));

        // 活卡魔改 dump：阶梯顶 1045.5 MHz、电压上限 1312.5 mV。
        const MODDED: &str = "../reverse/K4000_modded.rom";
        let Ok(m) = std::fs::read(MODDED) else {
            eprintln!("skip: {MODDED} not present");
            return;
        };
        let mvb = parse(&m).expect("legacy parse");
        let mladder = mvb.boost_ladder.as_ref().expect("modded ladder");
        assert_eq!(mladder.entries[15].freq_mhz_x2, 2091, "1045.5 MHz x2");
        let (_, mmax) = mvb
            .ladder_voltage_uv(mladder.entries[15].vmap_index)
            .expect("vmap");
        assert_eq!(mmax, 1_312_500);
    }

    /// Clock States（perf v0x40 per-Pstate 9 域）+ Boost States（P+0x30
    /// 三元组流）：K4000 与 KeplerBiosTweaker 两页逐值一致；域值 = u32
    /// & 0xFFF（P0 高位旗标 0x4000 剥离后 GPC=1098/DDR=2808）。
    #[test]
    fn kepler_clock_and_boost_states_when_present() {
        const K4000: &str = "../reverse/K4000_original.rom";
        let Ok(d) = std::fs::read(K4000) else {
            eprintln!("skip: {K4000} not present");
            return;
        };
        let states = find_perf_states(&d).expect("ok").expect("perf states");
        assert_eq!(states.len(), 3);
        let codes: Vec<u8> = states.iter().map(|s| s.pstate_code).collect();
        assert_eq!(codes, vec![0x07, 0x0A, 0x0F]); // P8, P5, P0
        assert_eq!(states.len() * 9, states.len() * PERF_DOMAIN_NAMES.len());
        // P0：域旗标剥离
        let p0 = &states[2];
        assert_eq!(p0.vmap_index, 0x04);
        assert_eq!(
            p0.domains_mhz,
            vec![1098, 1152, 1098, 2808, 1229, 1080, 540, 324, 540]
        );
        // P8：整卡怠速域
        assert_eq!(
            states[0].domains_mhz,
            vec![648, 648, 648, 324, 648, 648, 405, 324, 540]
        );

        let groups = find_boost_states(&d).expect("ok").expect("boost states");
        assert_eq!(groups.len(), 3);
        let p0 = &groups[2];
        assert_eq!(p0.pstate_code, 0x0F);
        let by_name = |name: &str| {
            p0.ranges
                .iter()
                .find(|r| r.domain == name)
                .copied()
                .expect("domain")
        };
        assert_eq!(
            (by_name("GPC").min_mhz_x2, by_name("GPC").max_mhz_x2),
            (1098, 1621)
        );
        assert_eq!(
            (by_name("SYS").min_mhz_x2, by_name("SYS").max_mhz_x2),
            (1229, 1815)
        );
        assert_eq!(
            (by_name("L2C").min_mhz_x2, by_name("L2C").max_mhz_x2),
            (1152, 1702)
        );
    }

    /// Turing+ 阶梯解码 = 低 14 位直存 MHz（&0x3FFF）：2070 点值
    /// 1410/1410/1620 与活卡 get-private-vftable base/boost 精确一致；
    /// "/32768" 旧读法在 Turing+ 上得一半频率（705/810）。同时验证
    /// VBIOS_sample TU106 1MB full dump 的 NVGI v3 + RFFS/RFRD 容器层
    /// （本仓库第一份 Turing+ 完整样本，hexpat 布局实测定案）。
    #[test]
    fn vp_turing_plus_ladder_and_flash_dir_when_present() {
        let Ok(d) = std::fs::read("../reverse/2070.rom") else {
            eprintln!("skip: ../reverse/2070.rom not present");
            return;
        };
        let tables = find_vp_tables(&d);
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert_eq!(t.generation, VpGeneration::TuringPlus);
        let mhz: Vec<u32> = t.entries.iter().map(|e| e.freq_mhz() as u32).collect();
        assert_eq!(mhz, vec![1410, 1410, 1620]);

        // TU106 full dump（NVGI 容器）：version@5=3、头长 0x24；RFFS v1 +
        // RFRD v2 + pci_option_rom_offset 处 55AA + InfoROM "JFFS"。
        const TU106: &str =
            "../reverse/VBIOS_sample/2070_TU106_90061800EE_219W_10DE_1F07_1043_8671.rom";
        let Ok(full) = std::fs::read(TU106) else {
            eprintln!("skip: {TU106} not present");
            return;
        };
        let n = parse_nvgi(&full).expect("NVGI");
        assert_eq!(n.version, 0x03);
        assert_eq!(n.header_len, 0x24);
        assert_eq!(n.xve_sub_vendor, 0x1043);
        assert_eq!(n.xve_subsystem_id, 0x8671);
        let fd = find_flash_directory(&full).expect("flash directory");
        assert_eq!(fd.rffs.version, 1);
        assert_eq!(fd.rffs.entry_size, 32);
        assert_eq!(fd.rom_dir.version, 2);
        assert!(
            fd.pci_rom_magic_ok,
            "RFRD.pci_option_rom_offset must be 55AA"
        );
        assert_eq!(fd.rom_dir.inforom_offset, 1_024_000);
        // Pascal（GP104 NVGI dump）无 RFFS/RFRD。
        let Ok(pascal) = std::fs::read("../reverse/GP104.rom") else {
            return;
        };
        assert!(find_flash_directory(&pascal).is_none());
    }

    /// BIT 'i' InternalUse 全解：TITAN X（Maxwell 0x48 表）版本/项目/日期
    /// 逐字段命中（G600、07/21/15、版本 84.00.45.00 + Oem 03 = 文件名
    /// 8400450003）；GP104（Pascal 0x5C 表）带 GUID。功率前缀三元组
    /// （GM204/TITAN X 无 CPR 锚的 miss 修复）+ 风扇签名定位。
    #[test]
    fn internal_use_power_triple_fan_signature_when_present() {
        const TITANX: &str =
            "../reverse/VBIOS_sample/GTX_TITANX_8400450003_GM200_275W_10DE_17C2_10DE_1132.rom";
        let Ok(d) = std::fs::read(TITANX) else {
            eprintln!("skip: {TITANX} not present");
            return;
        };
        let iu = find_internal_use(&d)
            .expect("bit ok")
            .expect("internal use");
        assert_eq!(iu.version.as_deref(), Some("84.00.45.00"));
        assert_eq!(iu.oem_version, Some(0x03));
        assert_eq!(iu.build_date.as_deref(), Some("07/21/15"));
        assert_eq!(iu.project.as_deref(), Some("G600"));
        assert_eq!(iu.build_guid, None, "Maxwell 0x48 表无 GUID");

        // 功率三元组：TITAN X 150/250/275（`21 03` 前缀）。
        let triples = find_power_triples(&d);
        assert_eq!(triples.len(), 1, "{triples:?}");
        assert_eq!(triples[0].prefix, [0x21, 0x03]);
        assert_eq!(
            (triples[0].min_mw, triples[0].target_mw, triples[0].limit_mw),
            (150_000, 250_000, 275_000)
        );

        // 风扇签名：cooler min/max RPM = 1050/4800；曲线 3 点
        // (61,1050)/(81.4,1900)/(91,4800)。
        let fc = find_fan_cooler_by_signature(&d);
        assert_eq!(fc.coolers.len(), 1);
        assert_eq!(fc.coolers[0].min_rpm, Some(1050));
        assert_eq!(fc.coolers[0].max_rpm, Some(4800));
        let fp = find_fan_policy_by_signature(&d);
        assert_eq!(fp.curves.len(), 1);
        let pts: Vec<(u32, u16)> = fp.curves[0]
            .iter()
            .map(|p| (p.temp_c as u32, p.rpm))
            .collect();
        assert_eq!(pts, vec![(61, 1050), (81, 1900), (90, 4800)]);

        // GM204 功率三元组（旧锚点扫描在此 miss）：100/180/225。
        const GM204: &str = "../reverse/Palit.GTX980.4096.141009.rom";
        let Ok(g) = std::fs::read(GM204) else {
            eprintln!("skip: {GM204} not present");
            return;
        };
        let t = find_power_triples(&g);
        assert_eq!(
            (t[0].min_mw, t[0].target_mw, t[0].limit_mw),
            (100_000, 180_000, 225_000)
        );

        // Pascal 0x5C 'i' 表：GUID 存在（gp104-1070.rom）。
        const GP104: &str = "../reverse/gp104-1070.rom";
        let Ok(gp) = std::fs::read(GP104) else {
            eprintln!("skip: {GP104} not present");
            return;
        };
        let p = find_internal_use(&gp)
            .expect("bit ok")
            .expect("internal use");
        assert!(p.build_guid.is_some(), "Pascal 0x5C 表应带 build GUID");
    }

    /// Blackwell（50 系）容器：4C 头识别、BIT 基址、delta 重定位、功率
    /// 记录、ThermalPolicy slowdown、'i' 扩展尾。
    #[test]
    fn blackwell_container_when_present() {
        const TI5060: &str = "../reverse/VBIOS_sample/5060Ti_98061F00C3.rom";
        let Ok(d) = std::fs::read(TI5060) else {
            eprintln!("skip: {TI5060} not present");
            return;
        };
        assert!(is_blackwell_container(&d));
        let info = find_blackwell(&d).expect("blackwell info");
        assert_eq!(info.image_base, 0x35400);
        assert_eq!(info.bit_offset, 0x361F0);
        let perf = info.perf.as_ref().expect("perf ptrs");
        let delta = perf.delta.expect("delta");
        assert_eq!(delta, 0x4D400);
        assert_eq!(delta % 0x400, 0);
        // 功率记录：5060Ti 桌面 250W 恒定。
        let p = info.power.expect("power record");
        assert_eq!((p.rated_mw, p.max_mw), (250_000, 250_000));
        // ThermalPolicy slowdown 87°C ×3。
        assert_eq!(info.slowdown_c, vec![87, 87, 87]);

        // 移动端功率 Rated/Max 与文件名吻合。
        const AISTONE: &str = "../reverse/VBIOS_sample/Aistone_GN22-X9_5080_Rated_80W_Max_175W_non_DDS_98032E0059_PD.rom";
        let Ok(m) = std::fs::read(AISTONE) else {
            eprintln!("skip: {AISTONE} not present");
            return;
        };
        let mi = find_blackwell(&m).expect("blackwell info");
        let mp = mi.power.expect("power record");
        assert_eq!((mp.rated_mw, mp.max_mw), (80_000, 175_000));

        // 非 Blackwell（NVGI/55AA 镜像）不误判。
        assert!(!is_blackwell_container(&d[0x1000..]));
    }

    /// 无 profile 变体（GP100 布局）：回走踩空 → profiles 空，阶梯保留。
    #[test]
    fn vp_no_profile_variant_yields_empty_profiles() {
        let mut d = synthetic_vp();
        // 抹掉两条 profile 的 ID（0x330、0x369），使回走 10 步无 0x07
        d[0x330] = 0x55;
        d[0x369] = 0x55;
        let tables = find_vp_tables(&d);
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert!(t.profiles.is_empty());
        assert_eq!(t.entries.len(), 3);
        assert_eq!(t.entries[2].freq_mhz(), 1911.0);
    }

    /// 合成 footers：两条 footer + marker，校验解码语义（mem_half ×2 =
    /// DRAM；mem_x2 &0x3FFF = DDR 显示值）与文件序（0x07 离 ladder 最近，
    /// 与实卡观测一致）。
    #[test]
    fn vp_footers_synthetic() {
        let mut d = synthetic_vp();
        // ladder 尾后放两条 footer + marker。ladder = 0x3A2+0x11+1 = 0x3B4；
        // 3 条目后尾 = 0x3B4 + 3*41 = 0x42F。
        let put = |d: &mut Vec<u8>, a: usize, b: &[u8]| d[a..a + b.len()].copy_from_slice(b);
        let footer7 = 0x431;
        let footerf = 0x431 + 41;
        put(&mut d, footer7, &[0x07, 0x00]);
        put(&mut d, footer7 + 9, &405u16.to_le_bytes()); // DDR 405
        put(&mut d, footer7 + 11, &202u16.to_le_bytes()); // ×2 = 404 DRAM
        put(&mut d, footerf, &[0x0F, 0x00]);
        put(&mut d, footerf + 9, &0x0FA4u16.to_le_bytes()); // DDR 4004
        put(&mut d, footerf + 11, &1001u16.to_le_bytes()); // ×2 = 2002 DRAM
        put(&mut d, footerf + 39, &[0x00, 0x00, 0x10, 0x0E]); // marker @+39
        // 回走终止符（实 ROM 由真实数据充当；i≤3 时仅挡 append 不停走）。
        // i=3 槽位 = 0x408；i=4 槽位 b1@0x3E0 置非零 → 停走。
        put(&mut d, 0x408, &[0x55, 0x55]);
        put(&mut d, 0x3E0, &[0x55]);

        let t = &find_vp_tables(&d)[0];
        assert_eq!(t.footers.len(), 2);
        assert_eq!(t.footers[0].id, 0x07);
        assert_eq!(t.footers[0].mem_clock_mhz(), 404);
        assert_eq!(t.footers[0].mem_clock_ddr_mhz(), 405);
        assert_eq!(t.footers[1].id, 0x0F);
        assert_eq!(t.footers[1].mem_clock_mhz(), 2002);
        assert_eq!(t.footers[1].mem_clock_ddr_mhz(), 4004);
        assert!(!t.footers[1].is_empty());
    }

    /// 合成功率表（desktop 布局）：锚点 + 3×`00 01 00` + useless=90000 +
    /// target/limit + 滑条。
    #[test]
    fn power_table_synthetic_desktop() {
        let mut d = vec![0u8; 0x400];
        let put = |d: &mut Vec<u8>, a: usize, b: &[u8]| d[a..a + b.len()].copy_from_slice(b);
        let anchor = 0x200;
        put(&mut d, anchor, &[0x28, 0x46, 0x0F, 0x00, 0x28, 0x46, 0x0F]);
        // 滑条（enabled）在窗口内
        put(&mut d, anchor - 50, &[0x0F, 0x02, 0xFF, 0xFF, 0xFF]);
        // 3 个 `00 01 00`（锚后 200B 内）
        let p1 = anchor + 9;
        let p2 = anchor + 0x22;
        let p3 = anchor + 0x35;
        put(&mut d, p1, &[0x00, 0x01, 0x00]);
        put(&mut d, p2, &[0x00, 0x01, 0x00]);
        put(&mut d, p3, &[0x00, 0x01, 0x00]);
        // useless @ p3-11 = 90000 → desktop；target @ p3-7、limit @ p3-3
        put(&mut d, p3 - 11, &900_000u32.to_le_bytes());
        put(&mut d, p3 - 7, &180_000u32.to_le_bytes());
        put(&mut d, p3 - 3, &200_000u32.to_le_bytes());

        let tables = find_power_tables(&d);
        assert_eq!(tables.len(), 1);
        let p = &tables[0];
        assert_eq!(p.platform, PowerPlatform::Desktop);
        assert_eq!(p.target_mw, 180_000);
        assert_eq!(p.limit_mw, 200_000);
        assert_eq!(p.sliders.len(), 1);
        assert!(p.sliders[0].enabled);
    }

    /// 合成热表（BIT P v2 +0x10 → tag 结构）：duty/pwm/trips/linear 全字段。
    #[test]
    fn thermal_synthetic_full() {
        let mut d = vec![0u8; 0x800];
        let put = |d: &mut Vec<u8>, a: usize, b: &[u8]| d[a..a + b.len()].copy_from_slice(b);
        put(&mut d, 0, &[0x55, 0xAA]);
        put(&mut d, 0x1D0, &BIT_SIG);
        // P v2 @ 0x2d9（len 0x68），热表指针 @ +0x10 → 0x400
        put(&mut d, 0x1DC, &[b'P', 2]);
        put(&mut d, 0x1DE, &0x68u16.to_le_bytes());
        put(&mut d, 0x1E0, &0x2D9u16.to_le_bytes());
        put(&mut d, 0x2D9 + 0x10, &0x400u32.to_le_bytes());
        // 热表：ver 0x15, hdr 4, len 4, cnt 5
        put(&mut d, 0x400, &[0x15, 0x04, 0x04, 0x05]);
        // [0] 0x22 duty: min=30 max=100
        put(&mut d, 0x404, &[0x22, 30, 100, 0]);
        // [1] 0x26 pwm_freq = 0x3000
        put(&mut d, 0x408, &[0x26, 0x00, 0x30, 0]);
        // [2] 0x24 trip: temp 60, hyst 3, duty nibble 2 → 25%
        //     value = duty<<12 | temp<<4 | hyst = 0x2000 | 0x3C0 | 0x3
        put(&mut d, 0x40C, &[0x24, 0xC3, 0x23, 0]);
        // [3] 0x25 trip duty override = 40
        put(&mut d, 0x410, &[0x25, 40, 0, 0]);
        // [4] 0x46 linear: min 30 max 90
        put(&mut d, 0x414, &[0x46, 30, 90, 0]);

        let th = find_thermal(&d).expect("bit ok").expect("thermal table");
        assert_eq!(th.table_offset, 0x400);
        assert_eq!(th.ver, 0x15);
        assert_eq!(th.min_duty, Some(30));
        assert_eq!(th.max_duty, Some(100));
        assert_eq!(th.pwm_freq, Some(0x3000));
        assert_eq!(th.trips.len(), 1);
        assert_eq!(th.trips[0].temp_c, 60);
        assert_eq!(th.trips[0].hysteresis, 3);
        // 0x25 覆写最后一条 trip 的 duty（nibble 2 → 25%，被 40 覆盖）
        assert_eq!(th.trips[0].duty_percent, 40);
        // 0x46 在 0x24 之后 → 线性模式（nouveau 比较序）
        assert_eq!(th.fan_mode, Some(FanMode::Linear));
        assert_eq!(th.linear_min_temp, Some(30));
        assert_eq!(th.linear_max_temp, Some(90));
    }

    /// 合成 BIT 但热指针为零 → Ok(None)（GP104 Pascal 实测形态）。
    #[test]
    fn thermal_zero_pointer_is_none() {
        let d = synthetic(); // P v2 @0x2d9，+0x10 未填 = 0
        assert!(matches!(find_thermal(&d), Ok(None)));
    }

    /// PERF_PTR 槽位图（合成）：P v2 @0x2d9 填两个槽——+0x10 指向合法表
    /// 头（0x500）、+0x38 指向"乱字节"（模拟 Pascal 压缩区目标）。
    #[test]
    fn perf_ptr_map_synthetic() {
        let mut d = vec![0u8; 0x800];
        let put = |d: &mut Vec<u8>, a: usize, b: &[u8]| d[a..a + b.len()].copy_from_slice(b);
        put(&mut d, 0, &[0x55, 0xAA]);
        put(&mut d, 0x1D0, &BIT_SIG);
        put(&mut d, 0x1DC, &[b'P', 2]);
        put(&mut d, 0x1DE, &0x9Cu16.to_le_bytes()); // len 156 → 扩展槽在位
        put(&mut d, 0x1E0, &0x2D9u16.to_le_bytes());
        // +0x10 → 0x500：ver 0x24 hdr 4 len 3 cnt 14（GM200 热表形态）
        put(&mut d, 0x2D9 + 0x10, &0x500u32.to_le_bytes());
        put(&mut d, 0x500, &[0x24, 0x04, 0x03, 0x0E]);
        // +0x38 → 0x600：ver 0xAE（压缩区形态，plausible=false）
        put(&mut d, 0x2D9 + 0x38, &0x600u32.to_le_bytes());
        put(&mut d, 0x600, &[0xAE, 0x74, 0x57, 0x65]);

        let m = find_perf_ptr_map(&d).expect("map");
        assert_eq!(m.ptab_offset, 0x2D9);
        assert_eq!(m.p_len, 156);
        assert_eq!(m.slots.len(), 2);
        assert_eq!(m.slots[0].slot_offset, 0x10);
        assert_eq!(m.slots[0].name, "ThermalControl");
        assert_eq!(m.slots[0].abs_offset, 0x500);
        assert_eq!(m.slots[0].header, Some([0x24, 0x04, 0x03, 0x0E]));
        assert!(m.slots[0].plausible);
        assert_eq!(m.slots[1].slot_offset, 0x38);
        assert_eq!(m.slots[1].name, "VirtualPState");
        assert!(!m.slots[1].plausible);
    }

    /// PERF_PTR 实卡三 ROM 断言：GP104 仅 Voltage 三槽 plausible（其余指向
    /// 压缩区）；GM200 旧布局 —— +0x10=热表(0x24)、+0x20=vmap(0x20)、
    /// +0x30=boost(0x11)、+0x34=ladder，且 Fan 三槽明文在位。
    #[test]
    fn perf_ptr_map_real_roms_when_present() {
        if let Ok(d) = std::fs::read("../reverse/gp104-1070.rom") {
            let m = find_perf_ptr_map(&d).expect("map");
            assert_eq!(m.p_len, 156);
            // EFI 间隙重定位后全部非零槽位落明文（21 槽 OK）
            assert_eq!(
                m.slots.iter().filter(|s| s.plausible).count(),
                m.slots.len()
            );
            // VirtualPState 重定位后 = VP 扫描命中位（交叉验证）
            let vp = m
                .slots
                .iter()
                .find(|s| s.name == "VirtualPState")
                .expect("VP slot");
            assert_eq!(vp.abs_offset, 0x2D643);
            assert!(vp.plausible);
            let vd = m
                .slots
                .iter()
                .find(|s| s.name == "VoltageDevice")
                .expect("VoltageDevice");
            assert_eq!(vd.header, Some([0x10, 0x04, 24, 8]));
            // Fan 三槽明文在位（Pascal 风扇表实际存在！）
            let fc = m
                .slots
                .iter()
                .find(|s| s.name == "FanCooler")
                .expect("FanCooler");
            assert_eq!(fc.abs_offset, 0x30810);
            assert_eq!(fc.header, Some([0x10, 0x04, 26, 1]));
        }
        if let Ok(d) = std::fs::read("../reverse/GM200.bin") {
            let m = find_perf_ptr_map(&d).expect("map");
            assert_eq!(m.p_len, 104);
            let by_off = |o: usize| m.slots.iter().find(|s| s.slot_offset == o).unwrap();
            // 旧布局语义（槽名是 modern 名，目标表 ver 才是真语义）
            assert_eq!(by_off(0x10).abs_offset, 0x8D93); // nouveau 热表
            assert_eq!(by_off(0x10).header, Some([0x24, 0x04, 0x03, 14]));
            assert_eq!(by_off(0x20).header, Some([0x20, 0x0F, 34, 101])); // vmap
            assert_eq!(by_off(0x30).header, Some([0x11, 0x06, 6, 6])); // boost
            assert_eq!(by_off(0x34).header, Some([0x10, 0x09, 8, 6])); // ladder 头 4B
            // Fan 三槽明文在位（内容布局未 RE，仅表头）
            assert_eq!(by_off(0x58).header, Some([0x10, 0x04, 26, 1]));
            assert_eq!(by_off(0x5C).header, Some([0x10, 0x05, 53, 1]));
            assert_eq!(by_off(0x64).header, Some([0x10, 0x04, 5, 1]));
        }
    }

    /// 实卡校准（gp104-1070.rom，GP104 Pascal）：footers/power/DCB 逐位
    /// 断言 + 热表缺席（P+0x10 = 0）。数值来自 Python 探针逐位解码。
    #[test]
    fn vp_full_real_rom_when_present() {
        let Ok(d) = std::fs::read("../reverse/gp104-1070.rom") else {
            eprintln!("skip: gp104-1070.rom not present");
            return;
        };
        // footers：12 槽、6 非空，文件序 [7,a,a,d,d,f]；值与 CPR GUI 一致
        let t = &find_vp_tables(&d)[0];
        assert_eq!(t.footers.len(), 12);
        let non_empty: Vec<(u8, u16, u16)> = t
            .footers
            .iter()
            .filter(|f| !f.is_empty())
            .map(|f| (f.id, f.mem_clock_mhz(), f.mem_clock_ddr_mhz()))
            .collect();
        assert_eq!(
            non_empty,
            vec![
                (0x07, 202, 405),
                (0x0A, 404, 810),
                (0x0A, 404, 810),
                (0x0D, 1900, 3802),
                (0x0D, 1900, 3802),
                (0x0F, 2002, 4004),
            ]
        );

        // power：desktop 180/200 W + slider enabled（锚点 0x30184）
        let power = find_power_tables(&d);
        assert_eq!(power.len(), 1);
        let p = &power[0];
        assert_eq!(p.platform, PowerPlatform::Desktop);
        assert_eq!(p.anchor_offset, 0x30184);
        assert_eq!(p.target_mw, 180_000);
        assert_eq!(p.limit_mw, 200_000);
        assert!(p.sliders.iter().any(|s| s.enabled));

        // thermal：指针为零 → None（Pascal 把风扇控制撤出可见 BIT）
        assert!(matches!(find_thermal(&d), Ok(None)));

        // falcon：EFI 间隙重定位后命中 @0x1FAE4，PMU 固件清单三条
        // （Python 探针逐位对照，Python probe bit-verified）。
        let falcon = find_falcon_table(&d)
            .expect("bit ok")
            .expect("falcon table");
        assert_eq!(falcon.table_offset, 0x1FAE4);
        assert_eq!(falcon.raw_ptr, 0xF0E4);
        assert_eq!(falcon.entries.len(), 5);
        let active: Vec<(&str, Option<&str>, u32)> = falcon
            .entries
            .iter()
            .filter(|e| e.desc_ptr != 0)
            .map(|e| {
                (
                    e.application.expect("named app"),
                    e.target,
                    e.desc.as_ref().expect("desc").stored_size,
                )
            })
            .collect();
        assert_eq!(
            active,
            vec![
                ("PRE_OS", Some("PMU"), 0x42B8),
                ("PRIMARY_DEVINIT_ENGINE", Some("PMU"), 0x3E50),
                ("FIRMWARE_SEC_LIC", None, 0x7390),
            ]
        );
        // FW_SEC_LIC 带 version/crypt 头（ver=2，安全许可证 ucode）
        let lic = &falcon.entries[4].desc.as_ref().expect("lic desc");
        assert!(lic.has_version_crypt);
        assert_eq!(lic.version, 2);
        let preos = &falcon.entries[0].desc.as_ref().expect("preos desc");
        assert!(!preos.has_version_crypt);
        assert_eq!(preos.stored_size, preos.uncompressed_size);

        // NVGI（NvAPI 镜像无 NVGI → None）
        assert!(parse_nvgi(&d).is_none());
    }

    /// NVGI 头（程序员 dump）：GP104.rom 开头实测逐位校准
    /// （sub_vendor=0x10DE NVIDIA、subsystem=0x119E GP104M；版本字节在
    /// +5 = 0x02 Pascal，头长 +6 = 0x10）。
    #[test]
    fn nvgi_real_rom_when_present() {
        let Ok(d) = std::fs::read("../reverse/GP104.rom") else {
            eprintln!("skip: GP104.rom not present");
            return;
        };
        let n = parse_nvgi(&d).expect("NVGI header");
        assert_eq!(n.version, 0x02);
        assert_eq!(n.header_len, 0x10);
        assert_eq!(n.total_data_size, 0x87C);
        assert_eq!(n.flags, 0x80);
        assert_eq!(n.xve_sub_vendor, 0x10DE);
        assert_eq!(n.xve_subsystem_id, 0x119E);
        // bare PC ROM（NvAPI 镜像）→ None
        let Ok(d2) = std::fs::read("../reverse/gp104-1070.rom") else {
            return;
        };
        assert!(parse_nvgi(&d2).is_none());
    }

    /// 显存三表实机校准（gp104-1070.rom，Python 探针逐位对照）：
    /// MemClock 11 频段×10 strap（首段 0..540）、MemTweak 64 组×68B、
    /// GPU-Z General/Power Limit/Thermal Limits 面板对齐（gp104-1070.rom
    /// 实卡逐字段对照，用户提供的 GPU-Z 2.69 截图）。
    #[test]
    fn gpuz_aligned_real_rom_when_present() {
        let Ok(d) = std::fs::read("../reverse/gp104-1070.rom") else {
            eprintln!("skip: gp104-1070.rom not present");
            return;
        };
        // General
        let id = find_bios_identity(&d).expect("identity");
        assert_eq!(id.version.as_deref(), Some("86.04.50.40.4A"));
        assert_eq!(id.oem_version, Some(0x4A));
        assert_eq!(id.build_date.as_deref(), Some("2017-07-03"));
        assert_eq!(id.message.as_deref(), Some("GV-N1070G1"));
        assert_eq!(id.board_id.as_deref(), Some("GP104 Board"));
        assert_eq!(id.internal_build_date.as_deref(), Some("10/07/16"));
        assert_eq!(id.cert_flag, Some(0x03));

        // Power Limit: Minimum 90.0 W / Default 180 / Maximum 200
        let power = find_power_tables(&d);
        let p = &power[0];
        assert_eq!(p.min_mw, Some(90_000));
        assert_eq!(p.target_mw, 180_000);
        assert_eq!(p.limit_mw, 200_000);
        // Adjustment Range -50% to +11% 派生自 min/def/max
        let lo = (p.min_mw.unwrap() as f64 - p.target_mw as f64) / p.target_mw as f64 * 100.0;
        let hi = (p.limit_mw as f64 - p.target_mw as f64) / p.target_mw as f64 * 100.0;
        assert!((lo + 50.0).abs() < 0.1 && (hi - 11.0).abs() < 0.2);

        // Thermal Limits: Rated 83°C / Maximum 92°C
        let tp = find_thermal_policy(&d)
            .expect("bit ok")
            .expect("thermal policy");
        let enabled: Vec<_> = tp.entries.iter().filter(|e| e.enabled).collect();
        assert_eq!(enabled.len(), 2);
        // 条目[2]：83(rated) / 60 / 92(maximum)
        assert_eq!(tp.entries[2].temp_a_c, Some(83));
        assert_eq!(tp.entries[2].temp_b_c, Some(60));
        assert_eq!(tp.entries[2].temp_c_c, Some(92));
        // 条目[0]：94°C 三连（NVIDIA 官方 Maximum GPU Temperature）
        assert_eq!(tp.entries[0].temp_a_c, Some(94));
        assert_eq!(tp.entries[0].temp_c_c, Some(94));

        // FanCooler：1 cooler（= get-fan-info Count）、max duty 100%、
        // max RPM 4190（= get-fan-info Max）；FanPolicy/FanTest 明文在位
        let fc = find_fan_cooler(&d).expect("bit ok").expect("fan cooler");
        assert_eq!(fc.coolers.len(), 1);
        assert_eq!(fc.coolers[0].max_duty_percent, Some(100));
        assert_eq!(fc.coolers[0].max_rpm, Some(4190));
        let fp = find_fan_policy(&d).expect("bit ok").expect("fan policy");
        assert_eq!(fp.entry_len, 53);
        // 风扇启停曲线（GPU-Z/Afterburner 同源）：0 RPM 以下不转（0 dB）
        assert_eq!(fp.curves[0].len(), 3);
        assert_eq!(fp.curves[0][0].temp_c, 1597.0 / 32.0);
        assert_eq!(fp.curves[0][0].rpm, 0);
        assert_eq!(fp.curves[0][1].temp_c, 50.0);
        assert_eq!(fp.curves[0][1].rpm, 800);
        assert_eq!(fp.curves[0][2].temp_c, 90.0);
        assert_eq!(fp.curves[0][2].rpm, 4190);
        let ft = find_fan_test(&d).expect("bit ok").expect("fan test");
        assert_eq!(ft.entry_len, 5);
        assert_eq!(ft.entry_count, 1);
    }

    /// MemInfo strap→变体解码 + ≥22 扩展（tj_max/热策略/功率修正）。
    #[test]
    fn memory_tables_real_rom_when_present() {
        let Ok(d) = std::fs::read("../reverse/gp104-1070.rom") else {
            eprintln!("skip: gp104-1070.rom not present");
            return;
        };
        let mc = find_memory_clock_table(&d)
            .expect("bit ok")
            .expect("memory clock table");
        assert_eq!(mc.table_offset, 0x2D8C8);
        assert_eq!(mc.ver, 0x11);
        assert!(mc.cmd_script_list_ptr.is_some());
        assert_eq!(mc.entries.len(), 6);
        // 首频段 0..540（GDDR5 内存时钟 MHz 量级）
        assert_eq!(mc.entries[0].freq_min_raw, 0);
        assert_eq!(mc.entries[0].freq_max_raw, 540);
        assert_eq!(mc.entries[0].straps.len(), 10);
        // tweak_timings_index 0xFF = None 哨兵在 strap 副本 +0
        assert!(
            mc.entries
                .iter()
                .any(|e| e.straps.iter().any(|s| s[0] == 0xFF))
        );

        let mt = find_memory_tweak_table(&d)
            .expect("bit ok")
            .expect("memory tweak table");
        assert_eq!(mt.table_offset, 0x2DC0A);
        assert_eq!(mt.ver, 0x20);
        assert_eq!(mt.base_entry_size, 68);
        assert_eq!(mt.extended_entry_size, 12);
        assert_eq!(mt.extended_entry_count, 0);
        assert_eq!(mt.entries.len(), 64);
        assert_eq!(mt.entries[0].len(), 68);

        let mi = find_memory_info(&d).expect("bit ok").expect("memory info");
        assert_eq!(mi.strap_count, 10);
        assert_eq!(mi.strap_translation.len(), 10);
        // GP104：elen=8（<22，无热/功耗扩展），16 变体，GDDR5 Samsung/Micron
        assert_eq!(mi.entries.len(), 16);
        let e0 = &mi.entries[0];
        assert_eq!(memory_type_name(e0.variant.mem_type), Some("GDDR5"));
        assert_eq!(memory_vendor_name(e0.variant.vendor_id), Some("Samsung"));
        assert_eq!(memory_density_name(e0.variant.density), Some("8Gb"));
        assert!(
            mi.entries
                .iter()
                .any(|e| memory_vendor_name(e.variant.vendor_id) == Some("Micron"))
        );
        assert!(mi.entries.iter().all(|e| e.pwr_adjustment_slope.is_none()));
    }

    /// Falcon 实机对照：GP104.rom（NVGI 头 dump，重定位量不同）与 P100
    /// （GP100 服务器卡）同构命中。
    #[test]
    fn falcon_more_real_roms_when_present() {
        if let Ok(d) = std::fs::read("../reverse/GP104.rom") {
            let f = find_falcon_table(&d).expect("bit ok").expect("falcon");
            assert_eq!(f.table_offset, 0x20CE4);
            assert_eq!(f.raw_ptr, 0xF2E4);
            assert_eq!(f.entries.len(), 5);
            assert_eq!(f.entries[0].application, Some("PRE_OS"));
            assert_eq!(f.entries[0].target, Some("PMU"));
            assert_eq!(
                f.entries[0].desc.as_ref().expect("desc").stored_size,
                0x1094
            );
        }
        if let Ok(d) = std::fs::read("../reverse/p100-vbios.rom") {
            let f = find_falcon_table(&d).expect("bit ok").expect("falcon");
            assert_eq!(f.table_offset, 0x1FCE4);
            let active: Vec<&str> = f
                .entries
                .iter()
                .filter(|e| e.desc_ptr != 0)
                .filter_map(|e| e.application)
                .collect();
            assert_eq!(
                active,
                vec!["PRE_OS", "PRIMARY_DEVINIT_ENGINE", "FIRMWARE_SEC_LIC"]
            );
        }
    }

    /// GM200（Maxwell）对照：P+0x10 指向 ver 0x24 表（传感器条目 +
    /// nouveau 未识别 tag；无风扇曲线——明文区没有 tag 风扇表，实证）。
    #[test]
    fn thermal_maxwell_real_rom_when_present() {
        let Ok(d) = std::fs::read("../reverse/GM200.bin") else {
            eprintln!("skip: GM200.bin not present");
            return;
        };
        let th = find_thermal(&d).expect("bit ok").expect("maxwell thermal");
        assert_eq!(th.table_offset, 0x8D93);
        assert_eq!(th.ver, 0x24);
        assert_eq!(th.entry_len, 3);
        assert_eq!(th.entry_count, 14);
        // GM200 表内无风扇字段，Fermi+ 默认线性
        assert_eq!(th.fan_mode, Some(FanMode::Linear));
        assert!(th.trips.is_empty());
        assert!(th.min_duty.is_none());
        // 未识别 tag（0x49/0x47）留档
        let tags: Vec<u8> = th.unknown_entries.iter().map(|u| u.tag).collect();
        assert!(tags.contains(&0x49) && tags.contains(&0x47), "{tags:?}");
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

    /// Pascal 及以后：P+0x34 无指针（GP104/P100 实测 = 0）→ 无 boost-ladder；
    /// 这些世代的 vBIOS 曲线走独立的 Virtual P-State ladder 解码器
    /// (`--enable-detail-parser`)，绝不能被本解析器误产垃圾阶梯。
    #[test]
    fn pascal_plus_has_no_boost_ladder() {
        for path in [
            "../reverse/GP104.rom",
            "../reverse/p100_vbios.rom",
            "../reverse/p100-vbios.rom",
        ] {
            let Ok(d) = std::fs::read(path) else {
                eprintln!("skip: {path} not present");
                continue;
            };
            let vb = parse(&d).expect("parse pascal rom");
            assert!(
                vb.boost_ladder.is_none(),
                "{path} must NOT yield a boost-ladder"
            );
            assert!(
                vb.warnings
                    .iter()
                    .any(|w| w.contains("boost-ladder: no pointer")),
                "{path} should warn 'no pointer'"
            );
        }
    }
}
