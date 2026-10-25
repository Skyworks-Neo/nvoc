//! VFE（Voltage/Frequency Equation）输出方程表解码——Turing/Ampere/Ada 与
//! GB20x 共用（vix `rom/structure.py::vfe_range` + `tables.py` 方程分类的
//! Rust 移植，read-only）。
//!
//! 方程布局（25B/Ada+GB20x 链接 u16、24B/Ampere+Turing 链接 u8）：
//! `+0` kind（1=二次输出、2=min/max、3=compare、4=定点、7=GB20x 多项式）、
//! `+1` 输入变量、`+2`(+`+24` u16 高位) 下一链接、`+3/+7` f32 输出钳位
//! min/max、`+11..+22` f32×3 系数、`+23` 输出旗标（低 4 位 = 单位码，
//! 2 = 电压 µV）。min/max 与 compare 方程的子链接随世代宽度不同。
//!
//! [`equation_span`] 沿链接递归求方程列表的 (最低, 最高) µV：每个方程先钳
//! 到自身 clamp 再累加下一链接（RTX 2000 Ada VMIN：clamp 560 mV、驱动
//! 650 mV，其差即链上浮点项之和）。二次项斜率为浮点残差（|c| < 1e-6）时
//! 方程取常数值；其余以 clamp 为界。

use std::collections::HashSet;

use crate::error::Error;
use crate::vbios::image_chain::f32_at;

/// 残差阈值：斜率绝对值低于它视为 0（语料最大 5e-9，真实斜率 ≥35）。
const VFE_RESIDUE: f64 = 1e-6;

/// 链接 none 哨兵（u8 链接 → 0xFF、u16 链接 → 0xFFFF）。
pub fn none_sentinel(layout: VfeLayout) -> u16 {
    match layout.link_width {
        1 => 0xFF,
        _ => 0xFFFF,
    }
}

/// 方程几何：尺寸 + 链接字节宽。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VfeLayout {
    pub equation_size: usize,
    /// 1 = u8 链接（Ampere/Turing），2 = u16 链接（Ada/GB20x）。
    pub link_width: usize,
}

impl VfeLayout {
    pub const ADA_GB20X: VfeLayout = VfeLayout {
        equation_size: 25,
        link_width: 2,
    };
    pub const AMPERE: VfeLayout = VfeLayout {
        equation_size: 24,
        link_width: 1,
    };

    fn none_sentinel(&self) -> u16 {
        match self.link_width {
            1 => 0xFF,
            _ => 0xFFFF,
        }
    }

    /// 读一个链接。u16 宽度时高位字节：next 链接在 low+22（2→24），
    /// 子链接在 low+4（11→15、12→16、15→19、16→20）。
    fn read_link(&self, data: &[u8], at: usize, off: usize) -> Result<u16, Error> {
        self.read_link_at(data, at, off, 4)
    }

    /// next 链接（low@+2、high@+24）。
    fn read_next(&self, data: &[u8], at: usize) -> Result<u16, Error> {
        self.read_link_at(data, at, 2, 22)
    }

    fn read_link_at(
        &self,
        data: &[u8],
        at: usize,
        off: usize,
        high_delta: usize,
    ) -> Result<u16, Error> {
        let b = data[at + off];
        Ok(match self.link_width {
            1 => u16::from(b),
            _ => u16::from(b) | (u16::from(data[at + off + high_delta]) << 8),
        })
    }
}

/// 方程 kind 原始码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EquationKind {
    Quadratic,
    MinMax,
    Compare,
    FixedPoint,
    /// GB20x REL 多项式（项引用变量而非方程）。
    Polynomial,
    Unknown(u8),
}

impl EquationKind {
    fn from_raw(raw: u8) -> EquationKind {
        match raw {
            1 => EquationKind::Quadratic,
            2 => EquationKind::MinMax,
            3 => EquationKind::Compare,
            4 => EquationKind::FixedPoint,
            7 => EquationKind::Polynomial,
            other => EquationKind::Unknown(other),
        }
    }
}

/// 二次电压输出的进一步归类（vix `_ada_equation_kind` 语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    /// 温度输入上的二次式，温度降额 REL——唯一可安全抬 clamp 上限的形状。
    TemperatureDerated,
    /// 零斜率（常数输出）。
    Constant,
    /// 被其它方程引用或被多个 role 共选。
    Shared,
    /// 带下一链接（值会再叠加后续方程）。
    Chained,
    /// 输出单位不是电压（µV）。
    NonVoltage,
    /// 输入变量不是温度通道。
    NonTemperature,
    /// 钳位/系数非有限或 min > max。
    Invalid,
    MinMax,
    Compare,
    FixedPoint,
    Polynomial,
    Unknown,
}

/// 单条方程。
#[derive(Debug, Clone, PartialEq)]
pub struct VfeEquation {
    pub index: u16,
    pub offset: usize,
    pub kind_raw: u8,
    pub kind: EquationKind,
    /// 输入变量索引。
    pub variable: u8,
    /// 下一链接（none 哨兵 → None）。
    pub next: Option<u16>,
    /// f32 输出钳位 (min, max)。
    pub clamp: Option<(f32, f32)>,
    /// f32 系数 [c0, c1, c2]。
    pub coefficients: Option<[f32; 3]>,
    /// `+23` 输出旗标字节。
    pub output_flags: u8,
    /// 输出单位码（低 4 位；2 = 电压 µV）。
    pub output_unit: u8,
    /// min/max 或 compare 子链接。
    pub children: Vec<u16>,
}

/// VFE 表整体。
#[derive(Debug, Clone, PartialEq)]
pub struct VfeTable {
    pub offset: usize,
    pub header_size: u8,
    /// 变量行大小。
    pub row_size: u8,
    /// 变量数。
    pub variable_count: u8,
    /// 变量行计数（Ada 头 +10 的 u16 高位参与方程数）。
    pub equation_size: usize,
    pub link_width: usize,
    /// 方程区起始绝对偏移。
    pub equations_base: usize,
    /// 方程条数。
    pub equation_count: u16,
    pub equations: Vec<VfeEquation>,
    /// 被其它方程引用的索引集合；出现未知 kind 时为 None。
    pub references: Option<HashSet<u16>>,
}

impl VfeTable {
    pub fn layout(&self) -> VfeLayout {
        VfeLayout {
            equation_size: self.equation_size,
            link_width: self.link_width,
        }
    }

    pub fn equation(&self, index: u16) -> Option<&VfeEquation> {
        self.equations.iter().find(|e| e.index == index)
    }
}

/// 解码 VFE 表。头 5B（+0 版本、+1 头长、+2 变量行长、+3 变量数、+4 标志
/// 类字节）；方程数读法随世代：Ada/GB20x = u16@+5 与 +10 拼、
/// Ampere/Turing = u8@+5。方程区基址 = `offset + header_size + row_size ×
/// variable_count`。
pub fn decode(
    data: &[u8],
    offset: usize,
    layout: VfeLayout,
    count_wide: bool,
) -> Result<VfeTable, Error> {
    if offset + 5 > data.len() {
        return Err(Error::from("vbios: VFE header out of bounds"));
    }
    let header_size = data[offset + 1];
    let row_size = data[offset + 2];
    let variable_count = data[offset + 3];
    let equation_count = if count_wide {
        u16::from(data[offset + 5]) | (u16::from(data[offset + 10]) << 8)
    } else {
        u16::from(data[offset + 5])
    };
    let equations_base =
        offset + usize::from(header_size) + usize::from(row_size) * usize::from(variable_count);
    if equation_count == 0 {
        return Err(Error::from("vbios: VFE table has no equations"));
    }
    let end = equations_base + usize::from(equation_count) * layout.equation_size;
    if end > data.len() {
        return Err(Error::from("vbios: VFE equations out of bounds"));
    }
    let none = layout.none_sentinel();
    let mut equations = Vec::with_capacity(usize::from(equation_count));
    for i in 0..equation_count {
        let at = equations_base + usize::from(i) * layout.equation_size;
        let kind_raw = data[at];
        let next = layout.read_next(data, at)?;
        let clamp = match (f32_at(data, at + 3), f32_at(data, at + 7)) {
            (Ok(lo), Ok(hi)) => Some((lo, hi)),
            _ => None,
        };
        let coefficients = match (
            f32_at(data, at + 11),
            f32_at(data, at + 15),
            f32_at(data, at + 19),
        ) {
            (Ok(a), Ok(b), Ok(c)) => Some([a, b, c]),
            _ => None,
        };
        let children = match EquationKind::from_raw(kind_raw) {
            EquationKind::MinMax => vec![
                layout.read_link(data, at, 11)?,
                layout.read_link(data, at, 12)?,
            ],
            EquationKind::Compare => vec![
                layout.read_link(data, at, 15)?,
                layout.read_link(data, at, 16)?,
            ],
            _ => Vec::new(),
        }
        .into_iter()
        .filter(|&c| c != none)
        .collect();
        equations.push(VfeEquation {
            index: i,
            offset: at,
            kind_raw,
            kind: EquationKind::from_raw(kind_raw),
            variable: data[at + 1],
            next: (next != none).then_some(next),
            clamp,
            coefficients,
            output_flags: data[at + 23.min(layout.equation_size - 1)],
            output_unit: data[at + 23.min(layout.equation_size - 1)] & 0x0F,
            children,
        });
    }
    let references = references_of(
        data,
        equations_base,
        layout,
        usize::from(equation_count),
        true,
    );
    Ok(VfeTable {
        offset,
        header_size,
        row_size,
        variable_count,
        equation_size: layout.equation_size,
        link_width: layout.link_width,
        equations_base,
        equation_count,
        equations,
        references,
    })
}

/// 全部方程引用的索引集合。`polynomial_allowed`：GB20x 语料含 kind 7。
/// 遇到集合外 kind 返回 None（调用方按"未知引用"降级只读）。
pub fn references_of(
    data: &[u8],
    equations_base: usize,
    layout: VfeLayout,
    count: usize,
    polynomial_allowed: bool,
) -> Option<HashSet<u16>> {
    let none = layout.none_sentinel();
    let mut refs = HashSet::new();
    let size = layout.equation_size;
    for i in 0..count {
        let at = equations_base + i * size;
        let e = data.get(at..at + size)?;
        let kind = match e[0] {
            0 => continue,
            1..=4 => e[0],
            7 if polynomial_allowed => 7,
            _ => return None,
        };
        let mut links = vec![match layout.link_width {
            1 => u16::from(e[2]),
            _ => u16::from(e[2]) | (u16::from(e[24]) << 8),
        }];
        if kind == 3 {
            links.push(match layout.link_width {
                1 => u16::from(e[15]),
                _ => u16::from(e[15]) | (u16::from(e[19]) << 8),
            });
            links.push(match layout.link_width {
                1 => u16::from(e[16]),
                _ => u16::from(e[16]) | (u16::from(e[20]) << 8),
            });
        } else if kind == 2 {
            links.push(match layout.link_width {
                1 => u16::from(e[11]),
                _ => u16::from(e[11]) | (u16::from(e[15]) << 8),
            });
            links.push(match layout.link_width {
                1 => u16::from(e[12]),
                _ => u16::from(e[12]) | (u16::from(e[16]) << 8),
            });
        }
        refs.extend(links.into_iter().filter(|&l| l != none));
    }
    Some(refs)
}

/// 沿 next 链接求方程列表 (最低, 最高) µV。循环/越界/未知形状/无界钳位
/// 返回 None。`index` 为 none 哨兵时返回 (0, 0)（空列表）。
pub fn equation_span(
    data: &[u8],
    base: usize,
    layout: VfeLayout,
    count: u16,
    index: u16,
) -> Option<(f64, f64)> {
    equation_span_seen(data, base, layout, count, index, &mut HashSet::new())
}

fn equation_span_seen(
    data: &[u8],
    base: usize,
    layout: VfeLayout,
    count: u16,
    index: u16,
    seen: &mut HashSet<u16>,
) -> Option<(f64, f64)> {
    let none = layout.none_sentinel();
    let size = layout.equation_size;
    let mut low = 0.0f64;
    let mut high = 0.0f64;
    let mut index = index;
    while index != none {
        if index >= count || seen.contains(&index) {
            return None;
        }
        seen.insert(index);
        let e =
            data.get(base + size * usize::from(index)..base + size * usize::from(index) + size)?;
        let (lo, hi, c0, c1, c2) = (
            f32::from_le_bytes(e[3..7].try_into().ok()?),
            f32::from_le_bytes(e[7..11].try_into().ok()?),
            f32::from_le_bytes(e[11..15].try_into().ok()?),
            f32::from_le_bytes(e[15..19].try_into().ok()?),
            f32::from_le_bytes(e[19..23].try_into().ok()?),
        );
        if e[23] & 0x0F != 2
            || lo
                .partial_cmp(&hi)
                .is_none_or(|o| o == std::cmp::Ordering::Greater)
        {
            return None;
        }
        let span = match e[0] {
            1 | 7 => {
                let constant = e[0] == 1
                    && f64::from(c1).abs() < VFE_RESIDUE
                    && f64::from(c2).abs() < VFE_RESIDUE;
                if constant {
                    (f64::from(c0), f64::from(c0))
                } else {
                    (f64::NEG_INFINITY, f64::INFINITY)
                }
            }
            2 | 3 => {
                let (a, b) = if e[0] == 2 { (11, 12) } else { (15, 16) };
                let mut child = |k: usize| -> Option<(f64, f64)> {
                    let idx = match layout.link_width {
                        1 => u16::from(e[k]),
                        _ => u16::from(e[k]) | (u16::from(e[k + 4]) << 8),
                    };
                    equation_span_seen(data, base, layout, count, idx, seen)
                };
                let ca = child(a)?;
                let cb = child(b)?;
                (ca.0.min(cb.0), ca.1.max(cb.1))
            }
            _ => return None,
        };
        let value = (
            span.0.clamp(lo as f64, hi as f64),
            span.1.min(hi as f64).max(lo as f64),
        );
        if !value.0.abs().lt(&1e30) || !value.1.abs().lt(&1e30) {
            // ±FLT_MAX 钳位等于不设界。
            return None;
        }
        low += value.0;
        high += value.1;
        index = match layout.link_width {
            1 => u16::from(e[2]),
            _ => u16::from(e[2]) | (u16::from(e[24]) << 8),
        };
    }
    Some((low, high))
}

/// 常数电压方程的 (值 µV)：kind 1、单位 2、无链接、有限且
/// min = max = c0、c1 = c2 = 0。
pub fn constant_equation_uv(equation: &VfeEquation) -> Option<f32> {
    let (lo, hi) = equation.clamp?;
    let [c0, c1, c2] = equation.coefficients?;
    if equation.kind_raw != 1
        || equation.output_unit != 2
        || equation.next.is_some()
        || !lo.is_finite()
        || lo != hi
        || hi != c0
        || c1 != 0.0
        || c2 != 0.0
    {
        return None;
    }
    Some(lo)
}

/// 二次电压输出的归类（跨世代）。`variable_is_temperature`：变量表中该
/// 变量是否为温度通道（type 3）。`roles_picking`：选中本方程的 role 计数。
pub fn classify_quadratic(
    equation: &VfeEquation,
    references: Option<&HashSet<u16>>,
    variable_count: u8,
    variable_is_temperature: bool,
    roles_picking: usize,
) -> OutputKind {
    if equation.kind_raw != 1 {
        return match equation.kind {
            EquationKind::MinMax => OutputKind::MinMax,
            EquationKind::Compare => OutputKind::Compare,
            EquationKind::FixedPoint => OutputKind::FixedPoint,
            EquationKind::Polynomial => OutputKind::Polynomial,
            _ => OutputKind::Unknown,
        };
    }
    if equation.output_unit != 2 {
        return OutputKind::NonVoltage;
    }
    let (lo, _) = match equation.clamp {
        Some(c) if c.0.is_finite() && c.1.is_finite() && c.0 <= c.1 => c,
        _ => return OutputKind::Invalid,
    };
    if lo < 0.0 {
        return OutputKind::Invalid;
    }
    if equation.next.is_some() {
        return OutputKind::Chained;
    }
    let referenced = references
        .map(|r| r.contains(&equation.index))
        .unwrap_or(true);
    if referenced || roles_picking > 1 {
        return OutputKind::Shared;
    }
    let Some([_, c1, c2]) = equation.coefficients else {
        return OutputKind::Unknown;
    };
    if c1 == 0.0 && c2 == 0.0 {
        return OutputKind::Constant;
    }
    if equation.variable >= variable_count || !variable_is_temperature {
        return OutputKind::NonTemperature;
    }
    OutputKind::TemperatureDerated
}

/// 变量表访问：第 `index` 个变量的 type 字节（vix：`b[vfe + b[vfe+1] +
/// variable * b[vfe+2]]`）。
pub fn variable_type(data: &[u8], table: &VfeTable, index: u8) -> Option<u8> {
    let at = table.offset
        + usize::from(table.header_size)
        + usize::from(index) * usize::from(table.row_size);
    data.get(at).copied()
}

/// 变量是否为温度通道（type 3）。
pub fn variable_is_temperature(data: &[u8], table: &VfeTable, index: u8) -> bool {
    variable_type(data, table, index) == Some(3)
}

/// 变量条目绝对偏移。
pub fn variable_offset(table: &VfeTable, index: u8) -> usize {
    table.offset + usize::from(table.header_size) + usize::from(index) * usize::from(table.row_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eq_bytes(kind: u8, unit: u8, next: u16, lo: f32, hi: f32, coeffs: [f32; 3]) -> Vec<u8> {
        let mut e = vec![0u8; 25];
        e[0] = kind;
        e[2] = (next & 0xFF) as u8;
        e[24] = (next >> 8) as u8;
        e[3..7].copy_from_slice(&lo.to_le_bytes());
        e[7..11].copy_from_slice(&hi.to_le_bytes());
        e[11..15].copy_from_slice(&coeffs[0].to_le_bytes());
        e[15..19].copy_from_slice(&coeffs[1].to_le_bytes());
        e[19..23].copy_from_slice(&coeffs[2].to_le_bytes());
        e[23] = unit;
        e
    }

    fn table_with(equations: &[Vec<u8>]) -> (Vec<u8>, VfeTable) {
        // 头：version 0x30, header_size 14, row_size 0, variable_count 0
        //（测试不带变量表），flag 25；+5/+10 = count（u16）。
        let mut data = vec![0x30, 14, 0, 0, 25, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        data[5] = (equations.len() & 0xFF) as u8;
        data[10] = (equations.len() >> 8) as u8;
        let base = data.len();
        for e in equations {
            data.extend_from_slice(e);
        }
        let table = decode(&data, 0, VfeLayout::ADA_GB20X, true).unwrap();
        assert_eq!(table.equations_base, base);
        (data, table)
    }

    #[test]
    fn constant_equation_and_span() {
        let (data, t) = table_with(&[eq_bytes(
            1,
            2,
            0xFFFF,
            800_000.0,
            800_000.0,
            [800_000.0, 0.0, 0.0],
        )]);
        let e = t.equation(0).unwrap();
        assert_eq!(constant_equation_uv(e), Some(800_000.0));
        let span = equation_span(&data, t.equations_base, t.layout(), t.equation_count, 0).unwrap();
        assert!((span.0 - 800_000.0).abs() < 1e-3);
        assert!(t.references.unwrap().is_empty());
    }

    #[test]
    fn chained_span_adds_next_link() {
        // 方程 0：斜率非零 → span = clamp [0, 200]；链接方程 1：常数 150。
        // 链和 = (0+150, 200+150)。
        let eq0 = eq_bytes(1, 2, 1, 0.0, 200.0, [0.0, 35.0, 0.0]);
        let eq1 = eq_bytes(1, 2, 0xFFFF, 150.0, 150.0, [150.0, 0.0, 0.0]);
        let (data, t) = table_with(&[eq0, eq1]);
        let span = equation_span(&data, t.equations_base, t.layout(), t.equation_count, 0).unwrap();
        assert!((span.0 - 150.0).abs() < 1e-3, "{span:?}");
        assert!((span.1 - 350.0).abs() < 1e-3, "{span:?}");
        assert_eq!(t.references.unwrap(), [1].into_iter().collect());
    }

    #[test]
    fn minmax_tree_spans_children() {
        // kind 2 min/max：子 = 0 和 1。
        let mut head = eq_bytes(2, 2, 0xFFFF, 0.0, 1e9, [0.0, 0.0, 0.0]);
        head[11] = 0;
        head[12] = 1;
        let eq0 = eq_bytes(1, 2, 0xFFFF, 600_000.0, 600_000.0, [600_000.0, 0.0, 0.0]);
        let eq1 = eq_bytes(1, 2, 0xFFFF, 655_000.0, 655_000.0, [655_000.0, 0.0, 0.0]);
        let (data, t) = table_with(&[eq0, eq1, head]);
        let span = equation_span(&data, t.equations_base, t.layout(), t.equation_count, 2).unwrap();
        assert!((span.0 - 600_000.0).abs() < 1e-3);
        assert!((span.1 - 655_000.0).abs() < 1e-3);
    }

    #[test]
    fn classify_temperature_derated_vs_constant() {
        let rel = eq_bytes(
            1,
            2,
            0xFFFF,
            915_000.0,
            1_200_000.0,
            [900_000.0, 35.0, 0.01],
        );
        let (_data, t) = table_with(&[rel]);
        let e = t.equation(0).unwrap();
        let kind = classify_quadratic(e, t.references.as_ref(), 8, true, 1);
        assert_eq!(kind, OutputKind::TemperatureDerated);
        let (_data, t) = table_with(&[eq_bytes(
            1,
            2,
            0xFFFF,
            1_200_000.0,
            1_200_000.0,
            [1_200_000.0, 0.0, 0.0],
        )]);
        let e = t.equation(0).unwrap();
        assert_eq!(
            classify_quadratic(e, t.references.as_ref(), 8, false, 1),
            OutputKind::Constant
        );
    }

    #[test]
    fn span_cycle_and_unknown_kind_fail_closed() {
        let eq0 = eq_bytes(1, 2, 0, 0.0, 1e9, [0.0, 0.0, 0.0]); // 自链 = 环
        let (data, t) = table_with(&[eq0]);
        assert!(equation_span(&data, t.equations_base, t.layout(), t.equation_count, 0).is_none());
        // kind 5：未知 → references=None、span=None。
        let bad = eq_bytes(5, 2, 0xFFFF, 0.0, 1e9, [0.0, 0.0, 0.0]);
        let (data, t) = table_with(&[bad]);
        assert!(t.references.is_none());
        assert!(equation_span(&data, t.equations_base, t.layout(), t.equation_count, 0).is_none());
    }
}
