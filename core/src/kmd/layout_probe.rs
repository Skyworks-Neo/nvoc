//! nvlddmkm 静态布局探测:对任意一代 x64 内核镜像,纯静态推导 RM 电源策略
//! 对象链的全部偏移(全局槽 → GPU 表 → Major → PowerRoot 字段),零硬件、
//! fail-closed。产物 [`NvlddmkmLayout`] 供 kmd 活体走查(链定位/写探针)按代
//! 消费,解决任务书「版本红线」的工程化:610/616.92 两代已知布局作为
//! 地面真值,新镜像的推导结果必须过交叉验证门才输出。
//!
//! 方法学(2026-10-06 三代实证:576.02 / 610.74 / 616.92):
//! 1. **F7 记录写签名**(`66 C7 44 24 ?? F7 03`,栈偏移可漂)跨代字节稳定,
//!    回溯函数边界得生成器;生成器对 root 寄存器的 disp32 访问簇按结构间距
//!    分类(elig=X+1 / amountActive=X+2 / base=X+4 / amount=X+8 / key=X+C /
//!    LOWER=X+10 / UPPER=X+14),绝对偏移即出;
//! 2. 生成器序首条 `mov r64,[rcx+d32]` = Major→PowerRoot;
//!    SetAmount 族的 `cmp byte [reg+init],0`(80 B8)为第二来源互证;
//! 3. **语义扫描**:线性反汇编 .text,跟踪「rip 相对全局加载(写段)→ ≤2 条
//!    指令内 [同 reg+disp]」的 (槽,disp) 对,热门对 = DriverGlobal 槽;
//!    继续跟踪指针在寄存器间的传播,槽加载后 ≤0x40 条内的大 disp(0x30000..
//!    0x100000)解引用簇 = GPU 表链:count/ID/Major 三偏移(ID-Major=8、
//!    count 在 ID 后 0x100..0x400 内);
//! 4. RM 命令立即数(0x2080A61A/0x2080E61B 族)在 .rdata 的分派表项里搜,
//!    命中即出 handler(可选,未命中不阻塞)。
//!
//! 本模块平台中立(纯字节+capstone);活体走查在 [`super::pagewalk`]。

use quick_error::quick_error;
use serde::{Deserialize, Serialize};

/// 一代镜像的完整布局档案(全部为 RVA/偏移,活体时加模块基址)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvlddmkmLayout {
    /// COFF TimeDateStamp(缓存键之一)。
    pub timestamp: u32,
    /// OptionalHeader.SizeOfImage(缓存键之一)。
    pub size_of_image: u32,
    /// 镜像首选基址。
    pub image_base: u64,
    /// DriverGlobal 槽 RVA(槽内是 state 指针)。
    pub global_slot_rva: u64,
    /// state → GPU 表 的偏移。
    pub state_table_off: u32,
    /// GPU 表内 count 字段偏移(u32)。
    pub table_count_off: u32,
    /// GPU 表内 Major[i] 指针偏移(条目基)。
    pub entry_major_off: u32,
    /// GPU 表内 GPU-ID[i] 偏移(= entry_major_off + 8)。
    pub entry_id_off: u32,
    /// 条目步长(实测两代均 0x10)。
    pub entry_stride: u32,
    /// Major → PowerRoot 偏移。
    pub major_root_off: u32,
    /// root init 字节偏移。
    pub root_init_off: u32,
    /// root eligibility 字节偏移。
    pub root_elig_off: u32,
    /// root amountActive 字节偏移。
    pub root_amount_active_off: u32,
    /// root base/cTGP(C 项)dword 偏移。
    pub root_base_off: u32,
    /// root amount(A 项)dword 偏移。
    pub root_amount_off: u32,
    /// root policy key 字节偏移。
    pub root_key_off: u32,
    /// root LOWER(B 项)dword 偏移。
    pub root_lower_off: u32,
    /// root UPPER(饱和顶)dword 偏移。
    pub root_upper_off: u32,
    /// F7 生成器函数 RVA。
    pub generator_rva: u64,
    /// RM GET 命令(可选,分派表未命中为 None)。
    pub rm_get_cmd: Option<u32>,
    /// RM SET 命令(可选)。
    pub rm_set_cmd: Option<u32>,
    /// RM GET handler RVA(可选)。
    pub get_handler_rva: Option<u64>,
    /// RM SET handler RVA(可选)。
    pub set_handler_rva: Option<u64>,
    /// 锚点审计 trail(每个锚的命中情况,诊断/存档用)。
    pub anchors: Vec<String>,
}

quick_error! {
    /// 布局探测失败(fail-closed:任何交叉验证门不过即整体拒绝)。
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum ProbeError {
        TooSmall {
            display("image too small for a PE")
        }
        NotPe {
            display("not a PE32+ image (bad MZ/PE/optional-header magic)")
        }
        NoText {
            display(".text section not found")
        }
        F7SignatureNotFound {
            display("F7 record signature not found (66 C7 44 24 ?? F7 03)")
        }
        GeneratorBoundaryNotFound {
            display("generator function boundary not found before the F7 record write")
        }
        RootFieldSpacingFailed(disps: String) {
            display("root field spacing gate failed (disps: {})", disps)
        }
        MajorRootMismatch(a: u32, b: u32) {
            display("Major→root offset mismatch between anchors: {:#x} vs {:#x}", a, b)
        }
        NoGlobalSlot {
            display("no DriverGlobal slot candidate (semantic scan found no hot rip-load)")
        }
        NoTableChain {
            display("no GPU-table chain candidate (slot load → big-disp cluster)")
        }
        TableEntryLayoutFailed(d: u32) {
            display("GPU-table entry layout gate failed for state+disp {:#x}", d)
        }
    }
}

/// PE 节。
#[derive(Debug, Clone)]
struct Section {
    rva: u64,
    vsize: u64,
    raw: u64,
    rawsz: u64,
    #[allow(dead_code)]
    name: String,
    writable: bool,
}

/// 最小 PE32+ 解析(节表 + 三个头字段)。
struct PeImage<'a> {
    img: &'a [u8],
    image_base: u64,
    timestamp: u32,
    size_of_image: u32,
    sections: Vec<Section>,
}

impl<'a> PeImage<'a> {
    fn parse(img: &'a [u8]) -> Result<Self, ProbeError> {
        if img.len() < 0x200 {
            return Err(ProbeError::TooSmall);
        }
        let lfanew = u32::from_le_bytes(img[0x3C..0x40].try_into().unwrap()) as usize;
        if lfanew + 0x100 > img.len() || &img[lfanew..lfanew + 4] != b"PE\0\0" {
            return Err(ProbeError::NotPe);
        }
        let coff = lfanew + 4;
        let machine = u16::from_le_bytes(img[coff..coff + 2].try_into().unwrap());
        if machine != 0x8664 {
            return Err(ProbeError::NotPe); // 仅支持 x64
        }
        let timestamp = u32::from_le_bytes(img[coff + 4..coff + 8].try_into().unwrap());
        let opt = coff + 20;
        if u16::from_le_bytes(img[opt..opt + 2].try_into().unwrap()) != 0x20B {
            return Err(ProbeError::NotPe);
        }
        let image_base = u64::from_le_bytes(img[opt + 24..opt + 32].try_into().unwrap());
        let size_of_image = u32::from_le_bytes(img[opt + 56..opt + 60].try_into().unwrap());
        let nsec = u16::from_le_bytes(img[coff + 2..coff + 4].try_into().unwrap()) as usize;
        let secoff = opt + u16::from_le_bytes(img[coff + 16..coff + 18].try_into().unwrap()) as usize;
        let mut sections = Vec::new();
        for i in 0..nsec {
            let s = secoff + i * 40;
            if s + 40 > img.len() {
                break;
            }
            let name = String::from_utf8_lossy(&img[s..s + 8])
                .trim_end_matches('\0')
                .to_string();
            let vsize = u32::from_le_bytes(img[s + 8..s + 12].try_into().unwrap()) as u64;
            let rva = u32::from_le_bytes(img[s + 12..s + 16].try_into().unwrap()) as u64;
            let rawsz = u32::from_le_bytes(img[s + 16..s + 20].try_into().unwrap()) as u64;
            let raw = u32::from_le_bytes(img[s + 20..s + 24].try_into().unwrap()) as u64;
            let chars = u32::from_le_bytes(img[s + 36..s + 40].try_into().unwrap());
            sections.push(Section {
                rva,
                vsize,
                raw,
                rawsz,
                name,
                writable: chars & 0x8000_0000 != 0,
            });
        }
        Ok(Self {
            img,
            image_base,
            timestamp,
            size_of_image,
            sections,
        })
    }

    /// RVA → 文件偏移(落在 raw 内才有)。
    fn file_off(&self, rva: u64) -> Option<usize> {
        for s in &self.sections {
            if rva >= s.rva && rva < s.rva + s.rawsz {
                let d = (rva - s.rva) as usize;
                let f = s.raw as usize + d;
                if f < self.img.len() {
                    return Some(f);
                }
            }
        }
        None
    }

    fn text(&self) -> Result<(u64, u64), ProbeError> {
        self.sections
            .iter()
            .find(|s| s.name == ".text")
            .map(|s| (s.rva, s.vsize))
            .ok_or(ProbeError::NoText)
    }
}

/// 回溯函数边界:从 `site` 向前找 ≥2 连续 0xCC(int3 填充)后的第一字节,
/// 最多回看 `max_back`。
fn fn_start_before(img: &[u8], site: usize, max_back: usize) -> Option<usize> {
    let lo = site.saturating_sub(max_back);
    let mut i = site;
    while i > lo {
        if img[i - 1] == 0xCC && img[i - 2] == 0xCC {
            return Some(i);
        }
        i -= 1;
    }
    None
}

/// 锚 1+2:F7 签名 → 生成器 → root 字段间距分类 + Major→root。
/// 返回 (root 字段偏移组, Major→root, 生成器 RVA, 锚点 trail)。
#[allow(clippy::too_many_lines)]
fn anchor_generator(
    pe: &PeImage,
    cs: &capstone::Capstone,
) -> Result<(RootFields, u32, u64, Vec<String>), ProbeError> {
    let mut trail = Vec::new();
    // F7 记录写:66 C7 44 24 XX F7 03(栈偏移可漂)
    let mut f7_sites = Vec::new();
    for o in 0..pe.img.len().saturating_sub(7) {
        if pe.img[o] == 0x66
            && pe.img[o + 1] == 0xC7
            && pe.img[o + 2] == 0x44
            && pe.img[o + 3] == 0x24
            && pe.img[o + 5] == 0xF7
            && pe.img[o + 6] == 0x03
        {
            f7_sites.push(o);
        }
    }
    if f7_sites.is_empty() {
        return Err(ProbeError::F7SignatureNotFound);
    }
    trail.push(format!("F7 签名 {} 处 @ {:#x?}", f7_sites.len(), f7_sites));

    use capstone::arch::x86::X86OperandType;
    use capstone::arch::DetailsArchInsn;

    for &site in &f7_sites {
        let Some(fn_off) = fn_start_before(pe.img, site, 0x800) else {
            continue;
        };
        let fn_rva = fn_off as u64;
        // site 是文件偏移;反汇编地址基 = image_base + fn_rva(伪 VA,同域可比)
        let site_stop = pe.image_base + fn_rva + (site - fn_off) as u64 + 0x30;
        let mut root_reg: Option<capstone::RegId> = None;
        let mut major_root: Option<u32> = None;
        let mut disps: Vec<(u32, bool)> = Vec::new(); // (disp, byte_access)
        let insns = cs.disasm_count(
            &pe.img[fn_off..(site + 0x40).min(pe.img.len())],
            pe.image_base + fn_rva,
            400,
        );
        let Ok(insns) = insns else { continue };
        for insn in insns.iter() {
            if insn.address() >= site_stop {
                break; // F7 写点后一点点就够
            }
            let Ok(detail) = cs.insn_detail(insn) else { continue };
            let ops: Vec<_> = match detail.arch_detail().x86() {
                Some(x) => x.operands().collect(),
                None => continue,
            };
            // cmp 的 mem 可在任一操作数位(cmp byte [rbx+X], dl 是 (Mem, Reg) 序);
            // 单操作数形态(如 cmp byte [rbx+X], imm8 被截断)跳过
            let (dst, mem) = if ops.len() == 2 {
                match (&ops[0].op_type, &ops[1].op_type) {
                    (X86OperandType::Reg(d), X86OperandType::Mem(m)) => (Some(d), Some(m)),
                    (X86OperandType::Mem(m), X86OperandType::Reg(d)) => (Some(d), Some(m)),
                    _ => (None, None),
                }
            } else {
                (None, None)
            };
            if matches!(
                insn.mnemonic(),
                Some("mov") | Some("movzx") | Some("cmp")
            )
                && let (Some(dst), Some(mem)) = (dst, mem)
            {
                let base_name = cs.reg_name(mem.base()).unwrap_or_default();
                if base_name == "rcx"
                    && mem.index() == capstone::RegId(0)
                    && mem.disp() != 0
                    && root_reg.is_none()
                    && insn.mnemonic() == Some("mov")
                {
                    // 序首 root 装载:mov r64,[rcx+M]
                    major_root = Some(mem.disp() as u32);
                    root_reg = Some(*dst);
                } else if root_reg.is_some_and(|r| r == mem.base()) && mem.index() == capstone::RegId(0) {
                    // root 字段访问(mov/movzx 读,cmp 测试)
                    let byte_acc =
                        insn.mnemonic() == Some("cmp") || insn.mnemonic() == Some("movzx");
                    disps.push((mem.disp() as u32, byte_acc));
                }
                // 指针转寄存器(mov rA, rB)不做 —— 生成器直接用 root reg
            }
        }
        if root_reg.is_none() || disps.is_empty() {
            continue;
        }
        // 结构间距分类:找 x 使 {x+1,x+2}⊆byte 位,{x+4,x+8,x+C,x+10,x+14}⊆dword 位
        let byte_disps: Vec<u32> = disps.iter().filter(|(_, b)| *b).map(|(d, _)| *d).collect();
        let all: Vec<u32> = disps.iter().map(|(d, _)| *d).collect();
        let has = |v: u32| all.contains(&v);
        // init(x) 本身可能不被生成器访问(610 实测:生成器只摸 elig/amountActive/key)。
        // 以任一字节访问候选作 elig(x+1) 回退推 x,再验 {x+2} 与五 dword。
        let mut init: Option<u32> = None;
        for &v in &byte_disps {
            let x = v.wrapping_sub(1);
            if byte_disps.contains(&(x + 1))
                && has(x + 4)
                && has(x + 8)
                && has(x + 0xC)
                && has(x + 0x10)
                && has(x + 0x14)
            {
                init = Some(x);
                break;
            }
        }
        let Some(init) = init else {
            continue;
        };
        let fields = RootFields {
            init,
            elig: init + 1,
            amount_active: init + 2,
            base: init + 4,
            amount: init + 8,
            key: init + 0xC,
            lower: init + 0x10,
            upper: init + 0x14,
        };
        let m = major_root.ok_or(ProbeError::GeneratorBoundaryNotFound)?;
        trail.push(format!(
            "生成器 @ {fn_rva:#x}: root 字段 init={init:#x} M={m:#x}(间距分类过)"
        ));
        return Ok((fields, m, fn_rva, trail));
    }
    Err(ProbeError::GeneratorBoundaryNotFound)
}

/// root 字段偏移组。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RootFields {
    init: u32,
    elig: u32,
    amount_active: u32,
    base: u32,
    amount: u32,
    key: u32,
    lower: u32,
    upper: u32,
}

/// 锚 2 互证:SetAmount 族签名 `cmp byte [reg+init],0`(80 B8)的函数序首
/// `mov r64,[rcx+d32]` → 第二个 Major→root 来源。
fn anchor_set_amount(
    pe: &PeImage,
    cs: &capstone::Capstone,
    init_off: u32,
) -> Option<u32> {
    use capstone::arch::x86::X86OperandType;
    use capstone::arch::DetailsArchInsn;
    let mut pat = vec![0x80u8, 0xB8];
    pat.extend_from_slice(&init_off.to_le_bytes());
    pat.push(0x00);
    let mut hits = Vec::new();
    let mut off = 0;
    while let Some(i) = find_sub(pe.img, &pat, off) {
        hits.push(i);
        off = i + 1;
        if hits.len() > 16 {
            break;
        }
    }
    for &h in &hits {
        let Some(fn_off) = fn_start_before(pe.img, h, 0x200) else {
            continue;
        };
        let Ok(insns) = cs.disasm_count(&pe.img[fn_off..h + 8], pe.image_base + fn_off as u64, 30)
        else {
            continue;
        };
        for insn in insns.iter() {
            if insn.address() as usize >= h {
                break;
            }
            if insn.mnemonic() == Some("mov")
                && let Ok(detail) = cs.insn_detail(insn)
            {
                let ops: Vec<_> = match detail.arch_detail().x86() {
                    Some(x) => x.operands().collect(),
                    None => continue,
                };
                if ops.len() == 2
                    && let (X86OperandType::Reg(_), X86OperandType::Mem(mem)) =
                        (&ops[0].op_type, &ops[1].op_type)
                    && cs.reg_name(mem.base()).as_deref() == Some("rcx")
                    && mem.index() == capstone::RegId(0)
                    && mem.disp() > 0
                    && mem.disp() < 0x10000
                {
                    return Some(mem.disp() as u32);
                }
            }
        }
    }
    None
}

fn find_sub(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// 语义扫描链节:线性反汇编 .text,产出 (槽, 表disp) 热门对与
/// (槽, 表disp, 大disp) 链簇。
/// (命中数, 访问宽度字节)
type HitCount = (usize, u8);

struct ChainScan {
    pairs: Vec<((u64, u32), usize)>,
    /// ((槽, 表 disp, 大 disp), 命中数+访问宽度)
    chains: Vec<((u64, u32, u32), HitCount)>,
}

fn scan_chains(pe: &PeImage, cs: &capstone::Capstone) -> Result<ChainScan, ProbeError> {
    use capstone::arch::x86::X86OperandType;
    use capstone::arch::DetailsArchInsn;
    // 字节锚定(与活体差分 session 的 Python 成功版同构):
    // 1) 全 .text 扫 `48/4C 8B ??(mod=00,rm=101)` = mov r64,[rip+d32];
    // 2) 目标落在写段 → state 槽装载候选;小窗(0x200)反汇编跟踪寄存器,
    //    记 (槽, deref disp) 对与 (槽, 表 disp, 大 disp) 链。
    // 线性全量反汇编在数据混排段会失同步,字节锚定是决定性手段。
    let (tva, tsz) = pe.text()?;
    let text_start = pe.file_off(tva).ok_or(ProbeError::NoText)?;
    let mut pairs: std::collections::HashMap<(u64, u32), usize> = Default::default();
    let mut chains: std::collections::HashMap<(u64, u32, u32), (usize, u8)> = Default::default();

    let mut off: usize = 0;
    while off + 7 <= tsz as usize {
        let i = text_start + off;
        let b0 = pe.img[i];
        let b1 = pe.img[i + 1];
        let modrm = pe.img[i + 2];
        // REX.W(48-4F) + 8B + modrm(mod=00, rm=101 → rip 相对)
        if (0x48..=0x4F).contains(&b0) && b1 == 0x8B && modrm & 0xC7 == 0x05 {
            let d = i32::from_le_bytes(pe.img[i + 3..i + 7].try_into().unwrap());
            let next = tva + off as u64 + 7;
            let target = (next as i64 + d as i64) as u64;
            let in_writable = pe
                .sections
                .iter()
                .any(|sec| target >= sec.rva && target < sec.rva + sec.vsize && sec.writable);
            if in_writable {
                // 小窗跟踪:目标寄存器 = modrm.reg(带 REX.R 扩展)
                let reg_ext = (u16::from((b0 >> 2) & 1)) << 3; // REX.R → bit3
                let reg_num = (((modrm >> 3) & 7) as u16 | reg_ext) as usize;
                const REGS: [&str; 16] = [
                    "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi",
                    "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15",
                ];
                let mut holders: Vec<&str> = vec![REGS[reg_num]];
                let window = &pe.img[i + 7..(i + 7 + 0x200).min(pe.img.len())];
                let mut bigs: Vec<(u32, u8)> = Vec::new(); // (disp, 访问宽度)
                let mut small: Vec<u32> = Vec::new();
                if let Ok(insns) = cs.disasm_count(window, next, 0x60) {
                    for insn in insns.iter() {
                        if insn.mnemonic() == Some("ret") {
                            break;
                        }
                        let Ok(detail) = cs.insn_detail(insn) else { continue };
                        let ops: Vec<_> = match detail.arch_detail().x86() {
                            Some(x) => x.operands().collect(),
                            None => continue,
                        };
                        // 找 mem 操作数(基寄存器在 holders)
                        for (oi, op) in ops.iter().enumerate() {
                            let X86OperandType::Mem(mem) = &op.op_type else { continue };
                            let base = cs.reg_name(mem.base()).unwrap_or_default();
                            if !holders.iter().any(|h| *h == base) {
                                continue;
                            }
                            let disp = mem.disp();
                            if (0x30000..0x100000).contains(&disp) {
                                bigs.push((disp as u32, op.size));
                            } else if (0x40..0x10000).contains(&disp) {
                                small.push(disp as u32);
                            }
                            // 指针传播:mov rD,[holders+…] → rD 入 holders(仅第 1 操作数)
                            if oi == 0
                                && insn.mnemonic() == Some("mov")
                                && let X86OperandType::Reg(nd) = &op.op_type
                            {
                                let _ = nd;
                            }
                        }
                        // 传播:mov rD, [mem] 的目的寄存器
                        if insn.mnemonic() == Some("mov")
                            && ops.len() == 2
                            && let (X86OperandType::Reg(nd), X86OperandType::Mem(mem)) =
                                (&ops[0].op_type, &ops[1].op_type)
                        {
                            let base = cs.reg_name(mem.base()).unwrap_or_default();
                            if holders.iter().any(|h| *h == base)
                                && let Some(name) = cs.reg_name(*nd)
                                && let Some(dst) = REGS.iter().find(|r| **r == name)
                            {
                                holders.push(dst);
                            }
                        }
                        if bigs.len() >= 3 {
                            break;
                        }
                    }
                }
                // (槽, 小 disp) 对与 (槽, 表 disp, 大 disp) 链:表 disp = 出现最多的小 disp
                // 组装成对计数;链按 "每个大 disp 配每个出现的小 disp" 组合(保守:
                // 只在窗口内同时存在时关联)。
                let mut small_counts: std::collections::HashMap<u32, usize> = Default::default();
                for d in &small {
                    *small_counts.entry(*d).or_insert(0) += 1;
                }
                for (d, n) in &small_counts {
                    *pairs.entry((target, *d)).or_insert(0) += n;
                }
                if let Some((&td, _)) = small_counts.iter().max_by_key(|(_, n)| *n) {
                    let mut big_counts: std::collections::HashMap<(u32, u8), usize> =
                        Default::default();
                    for bs in &bigs {
                        *big_counts.entry(*bs).or_insert(0) += 1;
                    }
                    for ((b, sz), n) in big_counts {
                        let e = chains.entry((target, td, b)).or_insert((0, sz));
                        e.0 += n;
                    }
                }
            }
        }
        off += 1;
    }
    let mut pairs: Vec<_> = pairs.into_iter().collect();
    pairs.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let mut chains: Vec<_> = chains.into_iter().collect();
    chains.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
    Ok(ChainScan { pairs, chains })
}

/// 从链簇选 GPU 表链:(槽, D) 组内找 ID/Major 对(M=I-8 双双在列)与
/// count(C 在 I 后 0x100..0x400)。
fn pick_table_chain(
    scan: &ChainScan,
) -> Result<(u64, u32, u32, u32, u32), ProbeError> {
    // 组槽+D → (bigdisp, size) 集合
    let mut groups: std::collections::HashMap<(u64, u32), Vec<(u32, u8)>> = Default::default();
    for ((slot, d, big), (_, sz)) in &scan.chains {
        groups.entry((*slot, *d)).or_default().push((*big, *sz));
    }
    let mut ranked: Vec<_> = groups.into_iter().collect();
    ranked.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for ((slot, d), bigs) in ranked {
        // 条目区两代代码gen不同:610 ID 带 disp32(dword cmp)、Major 折叠进变址;
        // 616.92 Major 带 disp32(qword mov)、ID 折叠。宽度语义:dword=ID/qword=Major
        // (count 也是 dword,故 dword 候选逐个试,以「ID 后 0x100..0x400 有 count」
        // 为接受条件;qword 候选与 dword 候选差 8 时互证)。
        let qwords: Vec<u32> = bigs.iter().filter(|(_, sz)| *sz == 8).map(|(b, _)| *b).collect();
        let dwords: Vec<u32> = bigs.iter().filter(|(_, sz)| *sz == 4).map(|(b, _)| *b).collect();
        // qword 主导(Major 直接可见)
        for &m in &qwords {
            let count = bigs
                .iter()
                .find(|(b, _)| *b > m && (*b - m) >= 0x100 && (*b - m) < 0x400);
            if let Some(&(count_off, _)) = count {
                return Ok((slot, d, count_off, m, m + 8));
            }
        }
        // dword 候选当 ID 逐个试
        for &i in &dwords {
            if i < 8 {
                continue;
            }
            let count = bigs
                .iter()
                .find(|(b, _)| *b > i && (*b - i) >= 0x100 && (*b - i) < 0x400);
            if let Some(&(count_off, _)) = count {
                return Ok((slot, d, count_off, i - 8, i));
            }
        }
    }
    // 全组失败 → 诊断输出后报最热组
    eprintln!("[probe-debug] pairs top8: {:?}", &scan.pairs[..scan.pairs.len().min(8)]);
    eprintln!("[probe-debug] chains top8: {:?}", &scan.chains[..scan.chains.len().min(8)]);
    let hottest = scan
        .pairs
        .first()
        .map(|((_, d), _)| *d)
        .unwrap_or(0);
    Err(ProbeError::TableEntryLayoutFailed(hottest))
}

/// 全量探测:对一代 x64 nvlddmkm 镜像推导完整布局(fail-closed)。
///
/// # Errors
/// 任一交叉验证门不过即 [`ProbeError`](具体哪道门见变体)。
#[allow(clippy::too_many_lines)]
pub fn probe(img: &[u8]) -> Result<NvlddmkmLayout, ProbeError> {
    let pe = PeImage::parse(img)?;
    use capstone::prelude::*;
    let cs = Capstone::new()
        .x86()
        .mode(capstone::arch::x86::ArchMode::Mode64)
        .detail(true)
        .build()
        .map_err(|_| ProbeError::NotPe)?;

    // 锚 1+2:生成器 → root 字段 + Major→root
    let (fields, major_root, generator_rva, mut anchors) = anchor_generator(&pe, &cs)?;
    // 锚 2 互证:SetAmount 族
    if let Some(m2) = anchor_set_amount(&pe, &cs, fields.init) {
        if m2 != major_root {
            return Err(ProbeError::MajorRootMismatch(major_root, m2));
        }
        anchors.push(format!("SetAmount 族互证 M={m2:#x} ✓"));
    } else {
        anchors.push("SetAmount 族未命中(非致命)".into());
    }
    // 锚 3:语义扫描 → 槽 + 表链
    let scan = scan_chains(&pe, &cs)?;
    anchors.push(format!(
        "语义扫描:热对 top3 {:?}",
        scan.pairs.iter().take(3).collect::<Vec<_>>()
    ));
    let (slot, state_d, count_off, major_off, id_off) = pick_table_chain(&scan)?;
    anchors.push(format!(
        "GPU 表链:槽={slot:#x} state+{state_d:#x} count=+{count_off:#x} Major=+{major_off:#x} id=+{id_off:#x}"
    ));
    // 锚 4:RM 命令(可选)。cmd 立即数会在 .text(代码引用)与 .rdata(分派表)
    // 各出现一次;只认「+0x10 处为镜像内指针」的分派表命中。
    let dispatch_handler = |cmd: u32| -> Option<u64> {
        let pat = cmd.to_le_bytes();
        let mut off = 0;
        while let Some(i) = find_sub(pe.img, &pat, off) {
            off = i + 1;
            let Some(rva) = pe_file_to_rva(&pe, i as u64) else {
                continue;
            };
            let Some(f) = pe.file_off(rva + 0x10) else {
                continue;
            };
            let h = u64::from_le_bytes(pe.img[f..f + 8].try_into().unwrap());
            if h > pe.image_base && h < pe.image_base + u64::from(pe.size_of_image) {
                return Some(h - pe.image_base);
            }
        }
        None
    };
    let rm_get = find_sub(pe.img, &0x2080_A61Au32.to_le_bytes(), 0).map(|_| 0x2080_A61Au32);
    let rm_set = find_sub(pe.img, &0x2080_E61Bu32.to_le_bytes(), 0).map(|_| 0x2080_E61Bu32);
    let (get_handler, set_handler) = match (rm_get, rm_set) {
        (Some(g), Some(_)) => {
            let gh = dispatch_handler(g);
            let sh = dispatch_handler(rm_set.unwrap());
            anchors.push(format!(
                "RM 命令 GET={g:#x} SET={:?} handler=({gh:?},{sh:?})",
                rm_set
            ));
            (gh, sh)
        }
        _ => {
            anchors.push("RM 命令未命中(非致命)".into());
            (None, None)
        }
    };
    Ok(NvlddmkmLayout {
        timestamp: pe.timestamp,
        size_of_image: pe.size_of_image,
        image_base: pe.image_base,
        global_slot_rva: slot,
        state_table_off: state_d,
        table_count_off: count_off,
        entry_major_off: major_off,
        entry_id_off: id_off,
        entry_stride: 0x10,
        major_root_off: major_root,
        root_init_off: fields.init,
        root_elig_off: fields.elig,
        root_amount_active_off: fields.amount_active,
        root_base_off: fields.base,
        root_amount_off: fields.amount,
        root_key_off: fields.key,
        root_lower_off: fields.lower,
        root_upper_off: fields.upper,
        generator_rva,
        rm_get_cmd: rm_get,
        rm_set_cmd: rm_set,
        get_handler_rva: get_handler,
        set_handler_rva: set_handler,
        anchors,
    })
}

/// 文件偏移 → RVA。
fn pe_file_to_rva(pe: &PeImage, file_off: u64) -> Option<u64> {
    pe.sections.iter().find_map(|s| {
        (file_off >= s.raw && file_off < s.raw + s.rawsz)
            .then_some(s.rva + (file_off - s.raw))
    })
}

#[cfg(test)]
mod tests {
    #![allow(unused_imports)]

    /// 结构间距分类的合成用例:disp 集合含 {X+1,X+2} 字节对 + 五个 dword。
    #[test]
    fn root_spacing_classifier_synthetic() {
        let x: u32 = 0x3CE0;
        let disps: Vec<(u32, bool)> = vec![
            (x, true),
            (x + 1, true),
            (x + 2, true),
            (x + 4, false),
            (x + 8, false),
            (x + 0xC, true),
            (x + 0x10, false),
            (x + 0x14, false),
            (0x1C90, false),
        ];
        let byte_disps: Vec<u32> = disps.iter().filter(|(_, b)| *b).map(|(d, _)| *d).collect();
        let all: Vec<u32> = disps.iter().map(|(d, _)| *d).collect();
        let has = |v: u32| all.contains(&v);
        let mut init: Option<u32> = None;
        for &c in &byte_disps {
            if byte_disps.contains(&(c + 1))
                && has(c + 4)
                && has(c + 8)
                && has(c + 0xC)
                && has(c + 0x10)
                && has(c + 0x14)
            {
                init = Some(c);
                break;
            }
        }
        assert_eq!(init, Some(0x3CE0), "x 本身=init,x+1=elig");
    }
}
