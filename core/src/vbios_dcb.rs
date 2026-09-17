//! vBIOS DCB（Display Configuration Block）解析——CPR_DCB.py 的 Rust 移植
//! （JadeRover Nvidia-vBIOS-Clock-Power-Tweaker ssj92 的 DCB 工作）。
//!
//! 模式扫描定位 DCB 4.x/5 头（版本 0x40-0x42/0x50）+ 紧随其后的 connector
//! 子表，双校验（形状 + connector 行合理性）过滤伪命中。覆盖 Fermi~Pascal
//! 移动 ROM 到 Ada/Blackwell 移动 dump 的观测格式。**只读显示配置**，不涉
//! 及刷写安全。

/// connector 类型字节的用户可读名（CPR 的宽松命名，精确子类型看原始字节）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectorType {
    Unused,
    DviAnalog,
    DviDigital,
    HdmiStyle,
    HdmiTmds,
    LvdsPanel,
    ExternalDp,
    InternalEdp,
    DpUsbC,
    DpUsbCSpecial,
    Disabled,
    Unknown(u8),
}

impl ConnectorType {
    fn from_code(code: u8) -> Self {
        match code {
            0x00 => Self::Unused,
            0x30 => Self::DviAnalog,
            0x31 => Self::DviDigital,
            0x38 => Self::HdmiStyle,
            0x3F => Self::HdmiTmds,
            0x40 => Self::LvdsPanel,
            0x46 => Self::ExternalDp,
            0x47 => Self::InternalEdp,
            0x60 => Self::DpUsbC,
            // 0x61：GPU-Z 实证桌面 Pascal（GV-N1070G1）为 HDMI；CPR 的
            // "DP/USB-C" 标注来自移动卡观察，0x73 保留承接。
            0x61 => Self::HdmiTmds,
            0x73 => Self::DpUsbCSpecial,
            0xFF => Self::Disabled,
            other => Self::Unknown(other),
        }
    }

    /// 短角色名（GPU-Z General 连接器对照精化）。
    pub fn short_role(self) -> &'static str {
        match self {
            Self::InternalEdp => "eDP",
            Self::LvdsPanel => "LVDS",
            Self::ExternalDp => "DP",
            Self::DpUsbC | Self::DpUsbCSpecial => "DP/USB-C",
            Self::DviAnalog => "DVI-I",
            Self::DviDigital => "DVI-D",
            Self::HdmiStyle | Self::HdmiTmds => "HDMI",
            Self::Disabled => "Disabled",
            Self::Unused => "Unused",
            Self::Unknown(_) => "Unknown",
        }
    }
}

/// connector 类型字节集合（CPR 合理性检查用）。
fn known_connector_code(code: u8) -> bool {
    matches!(
        code,
        0x30 | 0x31 | 0x38 | 0x3F | 0x40 | 0x46 | 0x47 | 0x60 | 0x61 | 0x73
    )
}

/// connector 索引 → DP pad 名（DP_A..；0x10/0x20 前缀风格 best-effort）。
pub fn pad_name(index: u8) -> Option<String> {
    let n = match index {
        0..=15 => index,
        0x10..=0x17 | 0x20..=0x27 => index & 0x0F,
        _ => return None,
    };
    let letter = char::from(b'A' + n);
    Some(format!("DP_{letter}"))
}

/// DCB output 条目（8B 输出路径表，heuristic 解码）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DcbOutput {
    pub index: usize,
    pub offset: usize,
    pub raw: [u8; 8],
    /// 低 4 位角色：0x2=HDMI、0x6=DP。
    pub role: Option<&'static str>,
    pub connector_number: u8,
}

/// connector 表一条。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DcbConnector {
    pub index: usize,
    pub offset: usize,
    pub raw: [u8; 4],
    pub connector_type: ConnectorType,
    pub type_code: u8,
    pub connector_index: u8,
}

impl DcbConnector {
    pub fn is_internal_edp(&self) -> bool {
        self.type_code == 0x47
    }

    pub fn is_internal_panel(&self) -> bool {
        self.type_code == 0x40 || self.type_code == 0x47
    }

    pub fn is_disabled(&self) -> bool {
        self.type_code == 0xFF
    }
}

/// 一个 DCB 块（双镜像 ROM 可有两份，取第一份为主）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DcbBlock {
    pub offset: usize,
    pub version: u8,
    pub header_length: u8,
    pub entry_count: u8,
    pub entry_length: u8,
    pub connector_offset: usize,
    pub connectors: Vec<DcbConnector>,
    /// connector 表前 0x147B 的启发式输出路径表（best-effort 角色补充）。
    pub outputs: Vec<DcbOutput>,
}

/// 一个 pad（DP_x）的聚合角色视图。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DisplayPad {
    pub name: String,
    pub roles: Vec<String>,
    pub connectors: Vec<DcbConnector>,
    pub is_internal_edp: bool,
    pub is_internal_panel: bool,
}

const DCB_VERSIONS: [u8; 4] = [0x40, 0x41, 0x42, 0x50];

/// 扫描全部 DCB 块。判定（CPR 同款）：版本字节、头长 5..0x30、条目数
/// 1..0x40、条目长 4/8，connector 子表紧跟其后且同版本族；connector 行
/// 需 ≥2 行合理（已知类型 + 合法索引）或含内面板行。
pub fn find_dcb_blocks(data: &[u8]) -> Vec<DcbBlock> {
    let mut blocks = Vec::new();
    if data.len() < 16 {
        return blocks;
    }
    for off in 0..data.len() - 16 {
        let (ver, hdr_len, entry_count, entry_len) =
            (data[off], data[off + 1], data[off + 2], data[off + 3]);
        if !DCB_VERSIONS.contains(&ver) || !(5..=0x30).contains(&hdr_len) {
            continue;
        }
        if !(1..=0x40).contains(&entry_count) || !matches!(entry_len, 4 | 8) {
            continue;
        }
        let conn_off =
            off + usize::from(hdr_len) + usize::from(entry_count) * usize::from(entry_len);
        if conn_off + 5 >= data.len() {
            continue;
        }
        let (cv, ch, cc, ce) = (
            data[conn_off],
            data[conn_off + 1],
            data[conn_off + 2],
            data[conn_off + 3],
        );
        if !DCB_VERSIONS.contains(&cv)
            || !(5..=0x30).contains(&ch)
            || !(1..=0x40).contains(&cc)
            || !matches!(ce, 4 | 8)
        {
            continue;
        }
        // connector 行合理性：至少 2 行已知类型 + 合法 pad 索引，或含内面板。
        let blob_end = conn_off + 4 + (usize::from(cc).min(16)) * usize::from(ce);
        let blob = &data[conn_off + 4..blob_end.min(data.len())];
        let mut plausible_rows = 0usize;
        let mut has_internal = false;
        for row in blob.chunks_exact(usize::from(ce)) {
            if row.len() < 4 {
                continue;
            }
            let (ctype, cidx) = (row[1], row[2]);
            let idx_ok =
                cidx <= 7 || (0x10..=0x17).contains(&cidx) || (0x20..=0x27).contains(&cidx);
            if known_connector_code(ctype) && idx_ok {
                plausible_rows += 1;
                if ctype == 0x40 || ctype == 0x47 {
                    has_internal = true;
                }
            }
        }
        if plausible_rows < 2 && !has_internal {
            continue;
        }
        blocks.push(parse_block(data, off));
    }
    blocks
}

fn parse_block(data: &[u8], off: usize) -> DcbBlock {
    let (ver, hdr_len, entry_count, entry_len) =
        (data[off], data[off + 1], data[off + 2], data[off + 3]);
    let conn_off = off + usize::from(hdr_len) + usize::from(entry_count) * usize::from(entry_len);
    let (cv, _ch, cc, ce) = (
        data[conn_off],
        data[conn_off + 1],
        data[conn_off + 2],
        data[conn_off + 3],
    );
    let mut connectors = Vec::new();
    for i in 0..usize::from(cc) {
        let coff = conn_off + 4 + i * usize::from(ce);
        let Some(raw) = data.get(coff..coff + 4) else {
            break;
        };
        connectors.push(DcbConnector {
            index: i,
            offset: coff,
            raw: [raw[0], raw[1], raw[2], raw[3]],
            connector_type: ConnectorType::from_code(raw[1]),
            type_code: raw[1],
            connector_index: raw[2],
        });
    }
    let outputs = parse_outputs_near(data, conn_off);
    let _ = cv;
    DcbBlock {
        offset: off,
        version: ver,
        header_length: hdr_len,
        entry_count,
        entry_length: entry_len,
        connector_offset: conn_off,
        connectors,
        outputs,
    }
}

/// 启发式 8B 输出路径表（CPR：connector 表 -0x147 处，Pascal~Blackwell
/// 移动 dump 观测）。只用于给 pad 补充 HDMI/DP 备选角色。
fn parse_outputs_near(data: &[u8], conn_off: usize) -> Vec<DcbOutput> {
    let Some(start) = conn_off.checked_sub(0x147) else {
        return Vec::new();
    };
    if start + 8 > data.len() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..24usize {
        let Some(raw) = data.get(start + i * 8..start + i * 8 + 8) else {
            break;
        };
        let low = raw[0] & 0x0F;
        if low == 0x0F {
            break; // EOL
        }
        if low == 0x0E {
            continue;
        }
        let role = match low {
            0x2 => Some("HDMI"),
            0x6 => Some("DP"),
            _ => None,
        };
        let conn_num = raw[2] & 0x7F;
        if let Some(role) = role {
            out.push(DcbOutput {
                index: i,
                offset: start + i * 8,
                raw: raw.try_into().unwrap_or([0; 8]),
                role: Some(role),
                connector_number: conn_num,
            });
        }
    }
    out
}

impl DcbBlock {
    /// 主块选择：双镜像取第一份（CPR primary_block 语义）。
    pub fn primary(data: &[u8]) -> Option<DcbBlock> {
        find_dcb_blocks(data).into_iter().next()
    }

    /// pad 聚合角色图（CPR build_display_map：connector 表为准，outputs
    /// 补充非内面板 pad 的 HDMI/DP 备选角色）。
    pub fn display_map(&self) -> [DisplayPad; 16] {
        let mut map: [DisplayPad; 16] = std::array::from_fn(|i| DisplayPad {
            name: format!("DP_{}", char::from(b'A' + i as u8)),
            ..DisplayPad::default()
        });
        for conn in &self.connectors {
            let Some(pad_idx) = pad_index(conn.connector_index) else {
                continue;
            };
            let role = conn.connector_type.short_role();
            let pad = &mut map[pad_idx];
            if role == "Disabled" || role == "Unused" {
                continue;
            }
            if conn.is_internal_edp() {
                pad.roles.clear();
                pad.roles.push("eDP".to_string());
                pad.is_internal_edp = true;
                pad.is_internal_panel = true;
            } else if role == "LVDS" && !pad.is_internal_edp {
                pad.roles.clear();
                pad.roles.push("LVDS".to_string());
                pad.is_internal_panel = true;
            } else if !pad.roles.iter().any(|r| r == role) {
                pad.roles.push(role.to_string());
            }
            pad.connectors.push(conn.clone());
        }
        for out in &self.outputs {
            let Some(pad) = pad_name(out.connector_number) else {
                continue;
            };
            let Some(pad_idx) = map.iter().position(|p| p.name == pad) else {
                continue;
            };
            let pad = &mut map[pad_idx];
            if pad.is_internal_panel {
                continue;
            }
            let Some(role) = out.role else {
                continue;
            };
            if !pad.roles.iter().any(|r| r == role) {
                pad.roles.push(role.to_string());
            }
        }
        map
    }
}

fn pad_index(index: u8) -> Option<usize> {
    match index {
        0..=15 => Some(usize::from(index)),
        0x10..=0x17 | 0x20..=0x27 => Some(usize::from(index & 0x0F)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 实卡校准（gp104-1070.rom）：DCB @0x55C2、connector @0x5603，
    /// 5 个非空 connector（DVI/HDMI + 3×DP + DP/USB-C），无内面板。
    #[test]
    fn dcb_real_rom_when_present() {
        let Ok(data) = std::fs::read("../reverse/gp104-1070.rom") else {
            eprintln!("skip: gp104-1070.rom not present");
            return;
        };
        let block = DcbBlock::primary(&data).expect("DCB block");
        assert_eq!(block.offset, 0x55C2);
        assert_eq!(block.connector_offset, 0x5603);
        assert_eq!(block.connectors.len(), 16);
        let non_empty: Vec<_> = block
            .connectors
            .iter()
            .filter(|c| !c.is_disabled())
            .collect();
        assert_eq!(non_empty.len(), 5);
        assert_eq!(non_empty[0].raw, [0x00, 0x31, 0x10, 0x00]);
        assert_eq!(non_empty[0].connector_type.short_role(), "DVI-D");
        assert_eq!(non_empty[1].raw, [0x00, 0x46, 0x01, 0x00]);
        assert_eq!(non_empty[1].connector_type.short_role(), "DP");
        assert_eq!(non_empty[3].connector_type.short_role(), "HDMI");

        let map = block.display_map();
        let active: Vec<_> = map
            .iter()
            .filter(|p| !p.roles.is_empty())
            .map(|p| (p.name.as_str(), p.roles.join("+")))
            .collect();
        assert_eq!(
            active,
            vec![
                ("DP_A", "DVI-D".to_string()),
                ("DP_B", "DP".to_string()),
                ("DP_C", "DP".to_string()),
                ("DP_D", "HDMI".to_string()),
                ("DP_E", "DP".to_string()),
            ]
        );
        assert!(!map.iter().any(|p| p.is_internal_panel));
    }
}
