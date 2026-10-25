//! DCB v3/v4 头指针子表族——envytools nvbios（dcb.c/gpio.c/i2c.c/extdev.c/
//! conn.c/mux.c）的 Rust 移植。
//!
//! DCB v3.0+ 头是固定布局的指针数组：+0x04 I2C、+0x06 签名 0x4EDCBDCB、
//! +0x0A GPIO、+0x0C INPUTDEV、+0x0E CINEMA、+0x10 SPREADSPECTRUM、
//! +0x12 EXTDEV、+0x14 CONN、hlen≥25 → +0x17 HDTVTT、hlen≥27 → +0x19 MUX。
//! 本模块解析其中五张有完整条目布局的表（I2C/GPIO/EXTDEV/CONN/MUX），
//! 其余指针只报告位置。TU106（v4.1）+ GP102（v4.1）实测校准：
//! EXTDEV 唯一非空条目 = INA3221@0x80（1080 Ti 功率计）。
//!
//! I2C v4.1+ 条目是 PMGR 语义（nouveau nvkm/subdev/bios/i2c.c 一手定案，
//! envytools 枚举停在 Turing 前不含 0x10/0x30 观测值）：u32 低 10 位 =
//! drive 线（0x1F=无）+ auxch 线（0x1F=无）。

use crate::vbios::dcb::DcbBlock;

/// 主 DCB 块的指针子表族全集（[`parse_all`] 产物）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DcbExtTables {
    pub dcb_offset: usize,
    pub header_ptrs: DcbHeaderPtrs,
    pub i2c: Option<I2cTable>,
    pub gpio: Option<GpioTable>,
    pub extdev: Option<ExtdevTable>,
    pub conn: Option<ConnTable>,
    pub mux: Option<MuxTable>,
}

/// 便捷入口：主 DCB（0x36 锚定）+ 全部指针子表。DCB 头内的 u16 指针为
/// legacy 镜像相对，经 [`legacy::find_image_base`] 折算为文件偏移。
pub fn parse_all(data: &[u8]) -> Option<DcbExtTables> {
    let block = DcbBlock::primary(data)?;
    let img = crate::vbios::legacy::find_image_base(data)?;
    let ptrs = header_ptrs(data, &block)?;
    let abs = |rel: u16| -> Option<usize> {
        if rel == 0 {
            None
        } else {
            let a = img + usize::from(rel);
            (a < data.len()).then_some(a)
        }
    };
    Some(DcbExtTables {
        dcb_offset: block.offset,
        header_ptrs: ptrs,
        i2c: abs(ptrs.i2c).and_then(|a| parse_i2c(data, a)),
        gpio: abs(ptrs.gpio).and_then(|a| parse_gpio(data, a)),
        extdev: abs(ptrs.extdev).and_then(|a| parse_extdev(data, a)),
        conn: abs(ptrs.conn).and_then(|a| parse_conn(data, a)),
        mux: abs(ptrs.mux).and_then(|a| parse_mux(data, a)),
    })
}

/// DCB v3/v4 头里的全部子表指针（u16，DCB 所在镜像相对）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DcbHeaderPtrs {
    pub i2c: u16,
    pub gpio: u16,
    pub inputdev: u16,
    pub cinema: u16,
    pub spreadspectrum: u16,
    pub extdev: u16,
    pub conn: u16,
    pub hdtvtt: u16,
    pub mux: u16,
}

/// 从 DCB 块读头指针。签名 0x4EDCBDCB 不符（或块过短）→ None。
/// 0x42/0x50 等 envytools 未收录版本仍按同布局尝试，靠签名把关。
pub fn header_ptrs(data: &[u8], block: &DcbBlock) -> Option<DcbHeaderPtrs> {
    let o = block.offset;
    let sig = u32::from(data.get(o + 6).copied()?)
        | u32::from(data.get(o + 7).copied()?) << 8
        | u32::from(data.get(o + 8).copied()?) << 16
        | u32::from(data.get(o + 9).copied()?) << 24;
    if sig != 0x4EDCBDCB {
        return None;
    }
    let r = |a: usize| -> u16 {
        u16::from(data.get(a).copied().unwrap_or(0))
            | u16::from(data.get(a + 1).copied().unwrap_or(0)) << 8
    };
    Some(DcbHeaderPtrs {
        i2c: r(o + 0x04),
        gpio: r(o + 0x0A),
        inputdev: r(o + 0x0C),
        cinema: r(o + 0x0E),
        spreadspectrum: r(o + 0x10),
        extdev: r(o + 0x12),
        conn: r(o + 0x14),
        hdtvtt: if block.header_length >= 25 {
            r(o + 0x17)
        } else {
            0
        },
        mux: if block.header_length >= 27 {
            r(o + 0x19)
        } else {
            0
        },
    })
}

// ── I2C 端口表（DCB +0x04）───────────────────────────────────────────────

/// I2C 端口类型（envious 0x30/0x40 世代 + nouveau PMGR 4.1+ 语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum I2cPortKind {
    /// 0：VGA CRTC 寄存器对（wr/rd 索引）。
    VgaCr,
    /// 1：NV4E 位口（drive 索引）。
    Nv4eBit,
    /// 4：PCRTC（loc + 地址）。
    Pcrtc,
    /// 5：PNVIO 位口（loc + 共享组）。
    NvioBit,
    /// 6：AUXCH（loc + 共享组）。
    AuxCh,
    /// 4.1+：PMGR 复合（drive 线 + auxch 线，各 5 位、0x1F=缺席）。
    Pmgr {
        drive: Option<u8>,
        auxch: Option<u8>,
    },
    Unused,
    Unknown(u8),
}

impl I2cPortKind {
    /// 端口类型名（JSON/人读输出用）。
    pub fn name(self) -> &'static str {
        match self {
            Self::VgaCr => "VGACR",
            Self::Nv4eBit => "NV4E_BIT",
            Self::Pcrtc => "PCRTC",
            Self::NvioBit => "PNVIO",
            Self::AuxCh => "AUXCH",
            Self::Pmgr { .. } => "PMGR",
            Self::Unused => "UNUSED",
            Self::Unknown(_) => "UNKNOWN",
        }
    }
}

/// 一条 I2C 端口。`write_index`/`read_index` = VGACR wr/rd CRTC 索引；
/// `loc`/`address` = PCRTC；`loc`/`shared_id` = PNVIO/AUXCH。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2cPort {
    pub index: usize,
    pub kind: I2cPortKind,
    pub write_index: u8,
    pub read_index: u8,
    pub loc: u8,
    pub address: u8,
    pub shared: bool,
    pub shared_id: u8,
}

/// I2C 端口表。`default_ports` = 头 +4 字节的两个默认端口索引。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I2cTable {
    pub offset: usize,
    pub version: u8,
    pub default_ports: [u8; 2],
    pub ports: Vec<I2cPort>,
}

/// 解析 I2C 表（仅 DCB v3.0+ 自带头格式；0x12-0x22 旧格式无头不在此列）。
pub fn parse_i2c(data: &[u8], offset: usize) -> Option<I2cTable> {
    let (version, hlen, cnt, rlen) = (
        *data.get(offset)?,
        *data.get(offset + 1)?,
        usize::from(*data.get(offset + 2)?),
        usize::from(*data.get(offset + 3)?),
    );
    if !matches!(version, 0x30 | 0x40 | 0x41) || rlen < 4 || hlen < 5 {
        return None;
    }
    let defs = *data.get(offset + 4)?;
    let mut ports = Vec::new();
    for i in 0..cnt.min(64) {
        let e = offset + usize::from(hlen) + i * rlen;
        let b = [
            *data.get(e)?,
            *data.get(e + 1)?,
            *data.get(e + 2)?,
            *data.get(e + 3)?,
        ];
        let kind = if version >= 0x41 {
            // PMGR：u32 低 10 位 = drive[4:0] + auxch[9:5]；两者全 1 = 空。
            let v = u32::from(b[0])
                | u32::from(b[1]) << 8
                | u32::from(b[2]) << 16
                | u32::from(b[3]) << 24;
            let drive = v & 0x1F;
            let auxch = (v >> 5) & 0x1F;
            if drive == 0x1F && auxch == 0x1F {
                I2cPortKind::Unused
            } else {
                I2cPortKind::Pmgr {
                    drive: (drive != 0x1F).then_some(drive as u8),
                    auxch: (auxch != 0x1F).then_some(auxch as u8),
                }
            }
        } else {
            match b[3] {
                0x00 => I2cPortKind::VgaCr,
                0x01 => I2cPortKind::Nv4eBit,
                0x04 => I2cPortKind::Pcrtc,
                0x05 => I2cPortKind::NvioBit,
                0x06 => I2cPortKind::AuxCh,
                0xFF => I2cPortKind::Unused,
                other => I2cPortKind::Unknown(other),
            }
        };
        ports.push(I2cPort {
            index: i,
            kind,
            write_index: b[0],
            read_index: b[1],
            loc: b[0] & 0x0F,
            address: b[1],
            shared: b[1] & 1 != 0,
            shared_id: (b[1] >> 1) & 0x0F,
        });
    }
    Some(I2cTable {
        offset,
        version,
        default_ports: [defs & 0x0F, defs >> 4],
        ports,
    })
}

// ── GPIO 表（DCB +0x0A）─────────────────────────────────────────────────

/// GPIO 行（按版本解位段；老版本 line/tag 合用一个 u16）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpioLine {
    pub index: usize,
    /// GPIO 线号。
    pub line: u8,
    /// 功能 tag（0xFF=未用）。
    pub tag: u8,
    pub tag_name: &'static str,
    /// 两个逻辑值极性（0/1/2/3）。
    pub log: [u8; 2],
    /// 附加参数位。
    pub param: bool,
    /// v0x40+：默认电平。
    pub default_val: Option<bool>,
    /// v0x40+：模式（1=SPEC NVIO、2=SPEC SOR）。
    pub mode: Option<u8>,
    /// v0x41：io 位（1 = lockpin 复用）。
    pub io: Option<bool>,
    /// v0x41：SPEC_OUT 多路值。
    pub spec_out: Option<u8>,
    /// v0x41：SPEC_IN 多路值。
    pub spec_in: Option<u8>,
    /// v0x41：lockpin 位段。
    pub lockpin: Option<u8>,
    /// v0x40 附加字节。
    pub unk40_0: Option<u8>,
}

/// GPIO 表。`xpio_tables` = v0x31+ XPIODIR 指向的外部 GPIO 扩展器表。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpioTable {
    pub offset: usize,
    pub version: u8,
    pub entries: Vec<GpioLine>,
    pub xpio_tables: Vec<XpioTable>,
}

/// 外部 GPIO 扩展器（XPIO）表头。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XpioTable {
    pub offset: usize,
    pub version: u8,
    /// 扩展器芯片类型（0x02=ADT7473、0x05=PCA9536…）。
    pub extender_type: u8,
    pub address: u8,
    pub bus: u8,
    pub entries: usize,
}

/// GPIO 功能 tag 名（envytools bios.h L282-534 全表；LCD1-7 六连族按
/// 0x8A + 12×n 规律生成，与手抄逐值核对）。
pub fn gpio_tag_name(tag: u8) -> &'static str {
    match tag {
        0x00 => "PANEL_BACKLIGHT_ON",
        0x01 => "PANEL_POWER",
        0x02 => "PANEL_POWER_STATUS",
        0x03 => "VSYNC",
        0x04 => "VID_0",
        0x05 => "VID_1",
        0x06 => "VID_2",
        0x07 => "HPD_0",
        0x08 => "HPD_1",
        0x09 => "FAN_PWM",
        0x0C => "TVDAC_0",
        0x0D => "TVDAC_0_ALT_LOAD_DETECT",
        0x0E => "STEREO_DAC_SELECT",
        0x0F => "STEREO_TOGGLE",
        0x10 => "ATX_POWER_BAD",
        0x11 => "THERM_EVENT_DETECT",
        0x12 => "VTG_RST",
        0x13 => "SUS_STAT",
        0x14 => "SPREAD_0",
        0x15 => "SPREAD_1",
        0x16 => "VDS_FRAMEID_0",
        0x17 => "VDS_FRAMEID_1",
        0x18 => "MEM_VOLTAGE",
        0x19 => "CUSTOMER",
        0x1A => "VID_3",
        0x1B => "VID_DEFAULT",
        0x1C => "TUNER",
        0x1D => "CURRENT_SHARE",
        0x1E => "CURRENT_SHARE_ENABLE",
        0x1F => "PANEL_SELF_TEST",
        0x20 => "PANEL_LAMP_STATUS",
        0x21 => "PANEL_BACKLIGHT_LEVEL",
        0x22 => "REQUIRED_POWER_SENSE",
        0x23 => "THERM_SHUTDOWN",
        0x24 => "HDTV_SELECT",
        0x25 => "HDTV_ALT_DETECT",
        0x27 => "OPTIONAL_POWER_SENSE",
        0x29 => "FRAME_LOCK_DAUGHTER_INTRPT",
        0x2A => "SW_PERF_LEVEL_SLOWDOWN",
        0x2B => "HW_SLOWDOWN_ENABLE",
        0x2C => "DISABLE_POWER_SENSE",
        0x2D => "TVDAC_1",
        0x2E => "MEM_VREF",
        0x30 => "GENERIC_INITIALIZED",
        0x31 => "HD_OVER_SDTV_BOOT_PREF",
        0x32 => "DIGITAL_ENC_INTRPT_ENABLE",
        0x33 => "I2C_OR_DDC",
        0x34 => "THERM_ALERT",
        0x35 => "THERM_CRITICAL",
        0x3C => "SCART_SELECT",
        0x3D => "FAN_TACH",
        0x3F => "EXT_SYNC_0",
        0x40 => "SLI_SENSE_0",
        0x41 => "SLI_SENSE_1",
        0x42 => "SWAP_RDY_SYNC_A",
        0x43 => "SWAP_RDY_OUT",
        0x44 => "SLI_SENSE_1_ALT",
        0x45 => "SCART_0",
        0x46 => "SCART_1",
        0x47 => "HD_DONGLE_STRAP_0",
        0x48 => "HD_DONGLE_STRAP_1",
        0x49 => "THERM_ALERT_OUT",
        0x4A => "DP_EXT_0",
        0x4B => "DP_EXT_1",
        0x4C => "ATX_POWER_BAD_ALT",
        0x4D => "TVDAC_0_LOAD_DETECT",
        0x4E => "ANALOGIX_ENC_EXT_RESET",
        0x4F => "I2C_SCL_KEEPER_CIRCUIT_ENABLE",
        0x50 => "DVI_DAC_SWITCH",
        0x51 => "HPD_2",
        0x52 => "HPD_3",
        0x53 => "DP_EXT_2",
        0x54 => "DP_EXT_3",
        0x55 => "MAXIM_EXT_RESET",
        0x56 => "SLI_LED_ACTIVE_DISPLAY",
        0x57 => "SPDIF_INPUT",
        0x58 => "TOSLINK_INPUT",
        0x59 => "AUDIO_SELECT",
        0x5A => "DPAUX_I2C_0",
        0x5B => "DPAUX_I2C_1",
        0x5C => "DPAUX_I2C_2",
        0x5D => "DPAUX_I2C_3",
        0x5E => "HPD_4",
        0x5F => "HPD_5",
        0x60 => "HPD_6",
        0x63 => "EXT_DEV_0_INTRPT",
        0x6A => "SWITCHED_OUTPUTS",
        0x6B => "CUSTOMER_RW",
        0x6C => "MXM_3_0_PIN_26",
        0x6D => "MXM_3_0_PIN_28",
        0x6E => "MXM_3_0_PIN_30",
        0x6F => "HW_PWR_SLOWDOWN",
        0x70 => "SWAP_RDY_SYNC_B",
        0x71 => "TRIGGER_PMU",
        0x72 => "SWAP_RDY_OUT_B",
        0x73 => "VID_4",
        0x74 => "VID_5",
        0x75 => "VID_6",
        0x76 => "VID_7",
        0x77 => "LVDS_FAST_SWITCH_MUX",
        0x78 => "FAN_FAILSAFE_PWM",
        0x79 => "ATX_POWER_LOW",
        0x7A => "ATX_FORCE_LOW_PWR",
        0x7B => "FAN_OVERTEMP",
        0x7C => "POST_GPU_LED",
        0x80 => "SMPBI_EVENT",
        0x81 => "VID_PWM0",
        0x82 => "VID_PWM1",
        0x83 => "SLI_LED_PWM",
        0x84 => "LOGO_LED_PWM",
        0x85 => "PSR_FRAME_LOCK_A",
        0x86 => "FB_CLAMP_EC_JT_MEM_SRFSH",
        0x87 => "FB_CLAMP_TOGGLE_REQ",
        0x8A..=0xB3 => {
            const LCD_NAMES: [&str; 6] = [
                "BACKLIGHT_ON",
                "POWER",
                "POWER_STATUS",
                "SELF_TEST",
                "LAMP_STATUS",
                "BACKLIGHT_LEVEL",
            ];
            let n = (usize::from(tag) - 0x8A) / 12 + 1;
            let f = (usize::from(tag) - 0x8A) % 12;
            if f < 6 {
                // 名字里带 LCD 编号需返回 String；静态表只保留功能名，
                // 编号由调用方按 tag 段还原——这里退化为功能名。
                let _ = n;
                LCD_NAMES[f]
            } else {
                "LCD_UNKNOWN"
            }
        }
        0xFF => "UNUSED",
        _ => "UNKNOWN",
    }
}

/// 解析 GPIO 表（v0x10/0x30/0x31/0x40/0x41）。
pub fn parse_gpio(data: &[u8], offset: usize) -> Option<GpioTable> {
    let version = *data.get(offset)?;
    let (hlen, cnt, rlen) = if version == 0x10 {
        (
            3usize,
            usize::from(*data.get(offset + 2)?),
            usize::from(*data.get(offset + 1)?),
        )
    } else {
        (
            usize::from(*data.get(offset + 1)?),
            usize::from(*data.get(offset + 2)?),
            usize::from(*data.get(offset + 3)?),
        )
    };
    if !matches!(version, 0x10 | 0x30 | 0x31 | 0x40 | 0x41) || cnt == 0 || cnt > 128 {
        return None;
    }
    let mut xpio_tables = Vec::new();
    if version >= 0x31 {
        let xpiodir = u16::from(data.get(offset + 4).copied().unwrap_or(0))
            | u16::from(data.get(offset + 5).copied().unwrap_or(0)) << 8;
        if xpiodir != 0 {
            xpio_tables = parse_xpiodir(data, usize::from(xpiodir));
        }
    }
    let mut entries = Vec::new();
    for i in 0..cnt {
        let e = offset + hlen + i * rlen;
        let b0 = *data.get(e)?;
        let b1 = *data.get(e + 1)?;
        let (line, tag, log0, log1, param);
        let mut default_val = None;
        let mut mode = None;
        let mut io = None;
        let mut spec_out = None;
        let mut spec_in = None;
        let mut lockpin = None;
        let mut unk40_0 = None;
        match version {
            0x10 | 0x30 | 0x31 => {
                let v = u16::from(b0) | u16::from(b1) << 8;
                line = (v & 0x1F) as u8;
                // 0x3F 段内保留值映射到 0xFF（UNUSED）。
                let raw_tag = ((v >> 5) & 0x3F) as u8;
                tag = if raw_tag == 0x3F { 0xFF } else { raw_tag };
                log0 = ((v >> 11) & 3) as u8;
                log1 = ((v >> 13) & 3) as u8;
                param = v >> 15 & 1 != 0;
            }
            0x40 => {
                let b2 = *data.get(e + 2)?;
                let b3 = *data.get(e + 3)?;
                line = b0 & 0x1F;
                unk40_0 = Some(b0 >> 5);
                tag = b1;
                log0 = (b3 >> 3) & 3;
                log1 = (b3 >> 5) & 3;
                param = b3 >> 7 & 1 != 0;
                default_val = Some(b3 & 1 != 0);
                mode = Some((b3 >> 1) & 3);
                let _ = b2;
            }
            0x41 => {
                let b3 = *data.get(e + 3)?;
                let b4 = *data.get(e + 4)?;
                line = b0 & 0x3F;
                io = Some(b0 >> 6 & 1 != 0);
                default_val = Some(b0 >> 7 & 1 != 0);
                tag = b1;
                spec_out = Some(*data.get(e + 2)?);
                spec_in = Some(b3 & 0x1F);
                param = b3 >> 7 & 1 != 0;
                lockpin = Some(b4 & 0x0F);
                log0 = (b4 >> 4) & 3;
                log1 = (b4 >> 6) & 3;
            }
            _ => return None,
        }
        entries.push(GpioLine {
            index: i,
            line,
            tag,
            tag_name: gpio_tag_name(tag),
            log: [log0, log1],
            param,
            default_val,
            mode,
            io,
            spec_out,
            spec_in,
            lockpin,
            unk40_0,
        });
    }
    Some(GpioTable {
        offset,
        version,
        entries,
        xpio_tables,
    })
}

/// XPIODIR 表（v0x30/0x40，hlen 4 / rlen 2，条目 = u16 XPIO 表指针）。
fn parse_xpiodir(data: &[u8], offset: usize) -> Vec<XpioTable> {
    let Some(version) = data.get(offset).copied() else {
        return Vec::new();
    };
    let (Some(hlen), Some(cnt), Some(rlen)) = (
        data.get(offset + 1).copied(),
        data.get(offset + 2).copied(),
        data.get(offset + 3).copied(),
    ) else {
        return Vec::new();
    };
    if !matches!(version, 0x30 | 0x40) || rlen < 2 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..usize::from(cnt).min(16) {
        let e = offset + usize::from(hlen) + i * usize::from(rlen);
        let ptr = u16::from(data.get(e).copied().unwrap_or(0))
            | u16::from(data.get(e + 1).copied().unwrap_or(0)) << 8;
        if ptr == 0 {
            continue;
        }
        let t = usize::from(ptr);
        // XPIO 表头：ver/hlen/entries/rlen + 扩展器类型@4、addr@5、bus@6
        // （envytools gpio.c XPIO 头注释；条目布局与 GPIO 同版式）。
        if let (Some(xv), Some(xc)) = (data.get(t).copied(), data.get(t + 2).copied())
            && matches!(xv, 0x10 | 0x30 | 0x31 | 0x40 | 0x41)
        {
            out.push(XpioTable {
                offset: t,
                version: xv,
                extender_type: data.get(t + 4).copied().unwrap_or(0),
                address: data.get(t + 5).copied().unwrap_or(0),
                bus: data.get(t + 6).copied().unwrap_or(0),
                entries: usize::from(xc),
            });
        }
    }
    out
}

// ── EXTDEV 表（DCB +0x12）───────────────────────────────────────────────

/// 外部 I2C 设备类型（envytools bios.h L643-699 全表）。
pub fn extdev_type_name(t: u8) -> &'static str {
    match t {
        0x01 => "ADM1032",
        0x02 => "MAX6649",
        0x03 => "LM99",
        0x06 => "MAX1617",
        0x07 => "LM64",
        0x0B => "LM89",
        0x0C => "TMP411",
        0x30 => "ADS1112",
        0x40 => "VT1103",
        0x41 => "PX3540",
        0x42 => "VT1165",
        0x43 => "CHIL_I2C_8203/8212/8213/8214",
        0x48 => "CHIL_SMBUS_8112A/B/8225/8228",
        0x49 => "CHIL_SMBUS_8266/8316",
        0x4C => "INA219",
        0x4D => "INA209",
        0x4E => "INA3221",
        0x50 => "CY2XP304",
        0x60 => "PCA9555",
        0x70 => "ADT7473",
        0x80 => "SI1930UC",
        0x90 => "HDCP_EEPROM",
        0xA0 => "INTERNAL_A0",
        0xB0 => "GT21X",
        0xB1 => "GF11X",
        0xD0 => "ANX9805",
        0xFF => "UNUSED",
        _ => "UNKNOWN",
    }
}

/// 一条外部 I2C 设备。`address` = 8 位 I2C 地址（0x80 = 7 位 0x40）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtdevDevice {
    pub index: usize,
    pub type_code: u8,
    pub type_name: &'static str,
    pub address: u8,
    /// 默认总线（DCB I2C 端口 bit4）。
    pub bus: u8,
    pub unk02_0: u8,
    pub unk02_5: u8,
    pub unk03: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtdevTable {
    pub offset: usize,
    pub version: u8,
    pub entries: Vec<ExtdevDevice>,
}

/// 解析 EXTDEV 表（v0x30/0x40，rlen 4）。
pub fn parse_extdev(data: &[u8], offset: usize) -> Option<ExtdevTable> {
    let (version, hlen, cnt, rlen) = (
        *data.get(offset)?,
        *data.get(offset + 1)?,
        usize::from(*data.get(offset + 2)?),
        usize::from(*data.get(offset + 3)?),
    );
    if !matches!(version, 0x30 | 0x40) || rlen < 4 || cnt > 64 {
        return None;
    }
    let mut entries = Vec::new();
    for i in 0..cnt {
        let e = offset + usize::from(hlen) + i * 4;
        let (t, addr, b2, b3) = (
            *data.get(e)?,
            *data.get(e + 1)?,
            *data.get(e + 2)?,
            *data.get(e + 3)?,
        );
        entries.push(ExtdevDevice {
            index: i,
            type_code: t,
            type_name: extdev_type_name(t),
            address: addr,
            bus: b2 >> 4 & 1,
            unk02_0: b2 & 0x0F,
            unk02_5: b2 >> 5,
            unk03: b3,
        });
    }
    Some(ExtdevTable {
        offset,
        version,
        entries,
    })
}

// ── CONN 表（DCB +0x14）─────────────────────────────────────────────────

/// connector 表类型名（envytools bios.h L722-754）。
pub fn conn_type_name(t: u8) -> &'static str {
    match t {
        0x00 => "VGA",
        0x10 => "COMPOSITE",
        0x11 => "S_VIDEO",
        0x12 => "S_VIDEO_COMPOSITE",
        0x13 => "RGB",
        0x20 => "DVI_I_TV_S_VIDEO",
        0x30 => "DVI_I",
        0x31 => "DVI_D",
        0x38 => "DMS59_0",
        0x39 => "DMS59_1",
        0x40 => "LVDS",
        0x41 => "LVDS_SPWG",
        0x46 => "DP",
        0x47 => "EDP",
        0x60 => "STEREO",
        0x61 => "HDMI",
        0x63 => "HDMI_C",
        0x64 => "DMS59_DP0",
        0x65 => "DMS59_DP1",
        0x70 => "WFD",
        0x71 => "USB_C",
        0xFF => "UNUSED",
        _ => "UNKNOWN",
    }
}

/// 一条 connector 表记录：类型 + tag + HPD/DP_AUX 线位图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnEntry {
    pub index: usize,
    pub type_code: u8,
    pub type_name: &'static str,
    pub tag: u8,
    /// HPD 线号（bit 位序 {12,13,16,17,24,25,26} → 0-6；无 = None）。
    pub hpd: Option<u8>,
    /// DP_AUX 扩展线（bit 14/15 → 0/1）。
    pub dp_ext: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnTable {
    pub offset: usize,
    pub version: u8,
    pub entries: Vec<ConnEntry>,
}

const CONN_HPD_BITS: [u32; 7] = [12, 13, 16, 17, 24, 25, 26];

/// 解析 CONN 表（v0x30/0x40；rlen 4 或 2）。
pub fn parse_conn(data: &[u8], offset: usize) -> Option<ConnTable> {
    let (version, hlen, cnt, rlen) = (
        *data.get(offset)?,
        *data.get(offset + 1)?,
        usize::from(*data.get(offset + 2)?),
        usize::from(*data.get(offset + 3)?),
    );
    if !matches!(version, 0x30 | 0x40) || cnt > 64 {
        return None;
    }
    let mut entries = Vec::new();
    for i in 0..cnt {
        let e = offset + usize::from(hlen) + i * rlen;
        let mut val = 0u32;
        for j in 0..rlen.min(4) {
            val |= u32::from(*data.get(e + j)?) << (8 * j);
        }
        let b0 = val as u8;
        let b1 = (val >> 8) as u8;
        let hpd = CONN_HPD_BITS
            .iter()
            .position(|&bit| val & (1 << bit) != 0)
            .map(|p| p as u8);
        let dp_ext = (0..2)
            .find(|&j| val & (1 << (j + 14)) != 0)
            .map(|p| p as u8);
        entries.push(ConnEntry {
            index: i,
            type_code: b0,
            type_name: conn_type_name(b0),
            tag: b1 & 0x0F,
            hpd,
            dp_ext,
        });
    }
    Some(ConnTable {
        offset,
        version,
        entries,
    })
}

// ── MUX 表（DCB hlen≥27 时 +0x19）───────────────────────────────────────

/// 一条显示 MUX 记录：DCB 条目号 + 四个控制通道（OUT/UNK1/HPD/DDC 的
/// 通道名来自 envytools mux.c 子槽序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuxEntry {
    pub index: usize,
    /// DCB 条目号（0x1F=未用）。
    pub dcb_index: u8,
    /// 四个子控制通道：loc（0=GPIO/1=XPIO）、line、val。
    pub lanes: [(u8, u8, bool); 4],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuxTable {
    pub offset: usize,
    pub version: u8,
    pub entries: Vec<MuxEntry>,
}

/// 解析 MUX 表（仅 v0x10，hlen 4 / rlen 5）。
pub fn parse_mux(data: &[u8], offset: usize) -> Option<MuxTable> {
    let (version, hlen, cnt, rlen) = (
        *data.get(offset)?,
        *data.get(offset + 1)?,
        usize::from(*data.get(offset + 2)?),
        usize::from(*data.get(offset + 3)?),
    );
    if version != 0x10 || rlen < 5 || cnt > 32 {
        return None;
    }
    let mut entries = Vec::new();
    for i in 0..cnt {
        let e = offset + usize::from(hlen) + i * rlen;
        let idx = *data.get(e)?;
        let mut lanes = [(0u8, 0u8, false); 4];
        for (j, lane) in lanes.iter_mut().enumerate() {
            let b = *data.get(e + 1 + j)?;
            *lane = (b & 1, (b >> 1) & 0x1F, b >> 6 & 1 != 0);
        }
        entries.push(MuxEntry {
            index: i,
            dcb_index: idx,
            lanes,
        });
    }
    Some(MuxTable {
        offset,
        version,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_1rom() -> Option<Vec<u8>> {
        std::fs::read("../1.rom").ok()
    }

    /// TU106 1.rom 实测：DCB v4.1 @0x521F，I2C 15 端口（PMGR）、GPIO 32 行
    /// （FAN_TACH@13 线）、EXTDEV 全 UNUSED、CONN 5 非空（DP×2/HDMI/USB-C/
    /// HDMI）、无 MUX。
    #[test]
    fn dcb_ext_tu106_when_present() {
        let Some(d) = load_1rom() else {
            eprintln!("skip: 1.rom not present");
            return;
        };
        let ext = parse_all(&d).expect("dcb ext");
        assert_eq!(ext.dcb_offset, 0x521F);
        let ptrs = ext.header_ptrs;
        assert_eq!(ptrs.i2c, 0x52C2);
        assert_eq!(ptrs.gpio, 0x4A2D);
        assert_eq!(ptrs.extdev, 0x53B3);
        assert_eq!(ptrs.conn, 0x5438);
        assert_eq!(ptrs.mux, 0);

        let i2c = ext.i2c.expect("i2c");
        assert_eq!(i2c.version, 0x41);
        assert_eq!(i2c.default_ports, [2, 0]);
        assert_eq!(i2c.ports.len(), 15);
        // 端口 0：e0 03 00 10 → v=0x100003E0 → drive=0、auxch=0x1F（缺席）。
        assert_eq!(
            i2c.ports[0].kind,
            I2cPortKind::Pmgr {
                drive: Some(0),
                auxch: None
            }
        );
        // 端口 3：03 00 00 10 → v=0x10000003 → drive=3、auxch=0。
        assert_eq!(
            i2c.ports[3].kind,
            I2cPortKind::Pmgr {
                drive: Some(3),
                auxch: Some(0)
            }
        );

        let gpio = ext.gpio.expect("gpio");
        assert_eq!(gpio.version, 0x41);
        assert_eq!(gpio.entries.len(), 32);
        let tach = gpio
            .entries
            .iter()
            .find(|e| e.tag_name == "FAN_TACH")
            .expect("fan tach line");
        assert_eq!(tach.line, 13);
        let pwm = gpio
            .entries
            .iter()
            .find(|e| e.tag_name == "FAN_PWM")
            .expect("fan pwm line");
        assert_eq!(pwm.line, 16);
        assert!(pwm.param);

        let extdev = ext.extdev.expect("extdev");
        assert_eq!(extdev.version, 0x40);
        // 移动 2070 无外部 I2C 传感器：32 条全 UNUSED。
        assert!(extdev.entries.iter().all(|e| e.type_code == 0xFF));

        let conn = ext.conn.expect("conn");
        assert_eq!(conn.version, 0x40);
        let active: Vec<&ConnEntry> = conn
            .entries
            .iter()
            .filter(|e| e.type_code != 0xFF)
            .collect();
        assert_eq!(active.len(), 5);
        // HPD 线位图 {12,13,16,17,24,25,26}→0-6：conn[0] bit17=HPD_3、
        // conn[1] bit16=HPD_2、conn[2] bit24=HPD_4、conn[3] bit25=HPD_5。
        assert_eq!((active[0].type_name, active[0].hpd), ("DP", Some(3)));
        assert_eq!((active[1].type_name, active[1].hpd), ("DP", Some(2)));
        assert_eq!(active[2].hpd, Some(4));
        assert_eq!(active[3].hpd, Some(5));
        assert_eq!(active[2].type_name, "HDMI");
        assert_eq!(active[3].type_name, "USB_C");
        assert_eq!(active[4].type_name, "HDMI");
    }

    /// GP102.rom（1080 Ti 级 Pascal）对照：EXTDEV 唯一非空 = INA3221@0x80。
    #[test]
    fn dcb_ext_gp102_when_present() {
        let Ok(d) = std::fs::read("../reverse/gpuz/GP102.rom") else {
            eprintln!("skip: GP102.rom not present");
            return;
        };
        let ext = parse_all(&d).expect("dcb ext");
        let extdev = ext.extdev.expect("extdev");
        let active: Vec<&ExtdevDevice> = extdev
            .entries
            .iter()
            .filter(|e| e.type_code != 0xFF)
            .collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].type_name, "INA3221");
        assert_eq!(active[0].address, 0x80);
        let i2c = ext.i2c.expect("i2c");
        assert_eq!(i2c.version, 0x41);
        assert_eq!(i2c.ports.len(), 15);
    }
}
