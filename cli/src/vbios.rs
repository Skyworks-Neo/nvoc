//! VBIOS BIT-table parser — extracts structure from a raw NVIDIA VBIOS image
//! read via `NvAPI_GPU_GetVbiosImage` (0xFC13EE11, escape 0x0700004F).
//!
//! Layout (validated live against a GT730/GF108 VBIOS, driver 391.35,
//! image 122368 B — see reverse/tmp-re/vbios/gt730.rom):
//!
//! - Offset 0: boot-sector magic `55 AA`. The classic "BIT pointer at 0x48"
//!   convention does NOT hold on this image (0x48 points elsewhere); the
//!   reliable locator is scanning for the ASCII "BIT" signature (a single
//!   occurrence in practice).
//! - BIT header: `"BIT"` + u8 version + u16 header size, then ~10 bytes of
//!   header-internal metadata (varies), then the token table.
//! - Token table: 6-byte entries `{u8 token, u8 version, u16 size LE,
//!   u16 offset LE}` where `offset` is relative to the BIT header. A zero
//!   token byte terminates the table (the trailing bytes are NOT all zero on
//!   real images). The table start is located by anchor-scanning: the
//!   position where the longest chain of plausible entries runs to a zero
//!   terminator (validated: 19 tokens B/C/D/A/I/L/M/N/P/S/T/U/V/x/d/p/i/u
//!   on the GT730 image).
//!
//! Fermi note (FermiBiosEditor 1.55 RE): the Fermi model is discrete
//! pstate × clock × VID entries in the 'P' (Performance) table — there is
//! no per-voltage V/F curve in this generation's VBIOS. The GT730 'P' table
//! carries layout byte 0x81, which no 2010-era tool decodes; until that
//! layout is reverse-engineered the sub-tables are surfaced as raw bytes.

use serde_json::{Value, json};

/// One BIT token-table entry.
#[derive(Debug, Clone)]
pub struct BitToken {
    /// Token character ('P' = Performance, 'V' = Voltage, 'M' = VID ladder,
    /// 'S' = Strings, ...).
    pub token: char,
    /// Sub-table version.
    pub version: u8,
    /// Sub-table byte size.
    pub size: u16,
    /// Sub-table offset, relative to the BIT header.
    pub offset: u16,
}

/// Perf-table layout bytes seen across generations: FermiBiosEditor
/// {0x10,0x11,0x12} (2010 layouts), 0x81 (GT730/GF108, undecoded), 0x50
/// (Pascal — nouveau only models up to 0x40, so 0x50 is undecoded too).
pub const KNOWN_PERF_LAYOUTS: [u8; 5] = [0x10, 0x11, 0x12, 0x50, 0x81];

/// Parsed BIT-table summary of a VBIOS image.
#[derive(Debug, Clone)]
pub struct BitSummary {
    /// Byte offset of the "BIT" signature in the image.
    pub bit_offset: usize,
    /// BIT header version byte.
    pub bit_version: u8,
    /// Base the token sub-table offsets resolve against. Fermi-era images
    /// carry BIT-RELATIVE offsets; Pascal images carry IMAGE-RELATIVE ones
    /// (data segment at image start, BIT header in its standard slot —
    /// live P100: BIT@0x1e2 and the token offsets only resolve at base 0;
    /// resolving BIT-relative read garbage, e.g. perf layout 0x0a instead
    /// of the real 0x50). Auto-detected via the 'P' layout byte: the first
    /// of {bit_offset, 0} whose resolved 'P' table carries a known layout
    /// byte at +1 or +2 wins; without a 'P' token the legacy base stands.
    pub data_base: usize,
    /// All token-table entries.
    pub tokens: Vec<BitToken>,
}

impl BitSummary {
    pub fn to_json(&self) -> Value {
        json!({
            "bit_offset": self.bit_offset,
            "data_base": self.data_base,
            "bit_version": self.bit_version,
            "tokens": self.tokens.iter().map(|t| json!({
                "token": t.token.to_string(),
                "version": t.version,
                "size": t.size,
                "offset": t.offset,
            })).collect::<Vec<_>>(),
        })
    }

    /// Raw bytes of a token's sub-table, if present. Offsets resolve
    /// against [`BitSummary::data_base`] (BIT-relative on Fermi-era
    /// images, image-relative on Pascal).
    pub fn token_raw<'a>(&self, image: &'a [u8], token: char) -> Option<&'a [u8]> {
        self.tokens.iter().find(|t| t.token == token).and_then(|t| {
            image
                .get(self.data_base + t.offset as usize..)?
                .get(..t.size as usize)
        })
    }
}

/// A sub-table surfaced as raw bytes: layout decoded, contents not. Used for
/// generation-specific tables whose entry encoding is not yet
/// reverse-engineered ('P' with layout 0x81 on GF108, the 'M' VID ladder,
/// the small 'V'/'U' voltage parameter blocks).
#[derive(Debug, Clone)]
pub struct RawBlock {
    /// Which BIT token this block came from.
    pub token: char,
    /// Table-level metadata shown alongside the bytes: for 'P' this is the
    /// layout byte at table offset +1 ({0x10,0x11,0x12} are the 2010
    /// FermiBiosEditor layouts; 0x81 on GT730/GF108 is undecoded).
    pub layout_version: Option<u8>,
    pub raw: Vec<u8>,
}

impl RawBlock {
    pub fn to_json(&self) -> Value {
        json!({
            "token": self.token.to_string(),
            "layout_version": self.layout_version,
            "raw_hex": hex(&self.raw),
        })
    }
}

/// Extract the structural view from a parsed BIT summary: the perf table and
/// the voltage-related tables, each as a raw block (see [`RawBlock`]).
/// Returns what is present; tokens absent on a given image are skipped.
pub fn parse_fermi_model(image: &[u8], bit: &BitSummary) -> Value {
    let perf = bit
        .token_raw(image, 'P')
        .filter(|raw| raw.len() >= 2)
        .map(|raw| RawBlock {
            token: 'P',
            layout_version: perf_layout_byte(image, raw),
            raw: raw.to_vec(),
        })
        .map(|b| b.to_json());
    let voltage_blocks: Vec<Value> = ['M', 'V', 'U']
        .iter()
        .filter_map(|t| {
            bit.token_raw(image, *t).map(|raw| {
                RawBlock {
                    token: *t,
                    layout_version: None,
                    raw: raw.to_vec(),
                }
                .to_json()
            })
        })
        .collect();
    json!({
        "model": "fermi-perftable",
        "note": "Fermi perf model = discrete pstate x clock x VID entries (no V/F curve); \
                 entry decode for this generation is not implemented — raw bytes below",
        "perf_table": perf,
        "voltage_blocks": voltage_blocks,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Locate the BIT signature ("BIT") — the reliable locator (the 0x48 pointer
/// convention does not hold on all images; validated GT730: 0x48 → 0x1000,
/// BIT actually at 0x1a6).
pub fn find_bit(image: &[u8]) -> Option<usize> {
    image.windows(3).position(|w| w == b"BIT")
}

/// Parse the BIT header + token table.
///
/// The token table starts a few bytes past the BIT signature (header
/// metadata varies by generation); the start is anchor-scanned: the
/// position where the longest chain of plausible 6-byte entries
/// `{token, version, size, offset}` runs to a zero terminator.
pub fn parse_bit(image: &[u8]) -> Result<BitSummary, String> {
    let bit = find_bit(image).ok_or_else(|| "no BIT signature".to_string())?;
    if bit + 8 > image.len() {
        return Err("BIT signature at end of image".to_string());
    }
    let bit_version = image[bit + 3];

    // Anchor-scan the token-table start over the header region.
    let scan_limit = (bit + 64).min(image.len() - 6);
    let mut best: Option<(usize, Vec<BitToken>)> = None;
    for start in bit + 4..scan_limit {
        let mut tokens = Vec::new();
        let mut p = start;
        let mut ok = true;
        while p + 6 <= image.len() && p < bit + 512 {
            let tok = image[p];
            let ver = image[p + 1];
            let size = u16::from_le_bytes([image[p + 2], image[p + 3]]);
            let off = u16::from_le_bytes([image[p + 4], image[p + 5]]);
            if tok == 0 {
                break; // token byte 0 terminates the table
            }
            // plausibility: printable token, small version, table in image
            let plausible = tok.is_ascii_graphic()
                && ver <= 10
                && bit + off as usize + size as usize <= image.len();
            if !plausible {
                ok = false;
                break;
            }
            tokens.push(BitToken {
                token: tok as char,
                version: ver,
                size,
                offset: off,
            });
            p += 6;
        }
        // accept the longest chain that actually reached a terminator
        if ok
            && tokens.len() >= 4
            && p < image.len()
            && image[p] == 0
            && best.as_ref().is_none_or(|(_, t)| t.len() < tokens.len())
        {
            best = Some((start, tokens));
        }
    }

    let (_, tokens) =
        best.ok_or_else(|| "BIT token table not found (no anchored chain)".to_string())?;
    let data_base = detect_data_base(image, bit, &tokens);
    Ok(BitSummary {
        bit_offset: bit,
        bit_version,
        data_base,
        tokens,
    })
}

/// Pick the base token offsets resolve against (see [`BitSummary::data_base`]).
/// Legacy BIT-relative is tried first so Fermi-era behavior is preserved;
/// the image-relative base only wins when the legacy resolve is garbage.
/// Two accepted forms:
/// - flat: the 'P' table itself carries a known layout byte at +1/+2
///   (Fermi-era {0x10,0x11,0x12}, GT730 0x81);
/// - pointer directory (Pascal): the 'P' table is an array of u32 image
///   offsets and the first target's first byte is the layout version
///   (live P100: P@750 → ptr 0xb05a → image[0xb05a] == 0x50). The
///   directory form self-validates the base — a wrong base resolves the
///   first u32 to an out-of-image pointer (BIT-relative on P100 gives
///   0x740aee ≫ image size).
fn detect_data_base(image: &[u8], bit: usize, tokens: &[BitToken]) -> usize {
    let Some(p) = tokens.iter().find(|t| t.token == 'P') else {
        return bit;
    };
    for &base in &[bit, 0] {
        let raw = image
            .get(base + p.offset as usize..)
            .and_then(|s| s.get(..p.size as usize));
        if let Some(raw) = raw {
            if perf_layout_byte(image, raw).is_some() {
                return base;
            }
        }
    }
    bit
}

/// The perf table's layout/version byte. Flat form: at +1 on the Fermi-era
/// layouts (GT730's 0x81 also at +1); directory form (Pascal): the first
/// u32 of the directory points at the real table whose first byte is the
/// version (P100: 0x50 — nouveau only models up to 0x40).
pub fn perf_layout_byte(image: &[u8], raw: &[u8]) -> Option<u8> {
    for i in [1usize, 2] {
        if raw.get(i).is_some_and(|b| KNOWN_PERF_LAYOUTS.contains(b)) {
            return Some(raw[i]);
        }
    }
    let ptr = raw
        .get(..4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)?;
    image
        .get(ptr)
        .copied()
        .filter(|&b| KNOWN_PERF_LAYOUTS.contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_scan_finds_signature() {
        let mut img = vec![0u8; 0x200];
        img[0] = 0x55;
        img[1] = 0xAA;
        img[0x1a6..0x1a9].copy_from_slice(b"BIT");
        assert_eq!(find_bit(&img), Some(0x1a6));
    }

    #[test]
    fn pascal_directory_data_base() {
        // Pascal form: token offsets are IMAGE-relative and the 'P' token is
        // a u32 pointer directory; the first target's first byte is the perf
        // table version. BIT-relative resolution reads a garbage first u32
        // that lands outside the image — the discriminator.
        let mut img = vec![0u8; 0x10000];
        img[0] = 0x55;
        img[1] = 0xAA;
        let bit = 0x1e2; // Pascal standard slot
        img[bit..bit + 3].copy_from_slice(b"BIT");
        img[bit + 3] = 0x00;
        // real perf table at 0xb05a, version byte 0x50
        img[0xb05a] = 0x50;
        let t = bit + 16;
        // 'P' directory entry: offset 750 (image-relative), size 16
        img[t..t + 6].copy_from_slice(&[b'P', 2, 16, 0, 0xf6, 0x02]);
        // directory body: first u32 = 0xb05a
        let dir = 750usize;
        img[dir..dir + 4].copy_from_slice(&0xb05au32.to_le_bytes());
        // + 3 more tokens to clear the >=4 chain bar
        img[t + 6..t + 12].copy_from_slice(&[b'B', 2, 4, 0, 0x10, 0x03]);
        img[t + 12..t + 18].copy_from_slice(&[b'S', 2, 8, 0, 0x20, 0x03]);
        img[t + 18..t + 24].copy_from_slice(&[b'V', 1, 6, 0, 0x30, 0x03]);
        img[t + 24..t + 30].copy_from_slice(&[0; 6]);
        let sum = parse_bit(&img).expect("parse");
        assert_eq!(sum.data_base, 0, "image-relative base must win");
        let raw = sum.token_raw(&img, 'P').expect("P raw");
        assert_eq!(perf_layout_byte(&img, raw), Some(0x50));
    }

    #[test]
    fn token_table_anchor_scan() {
        // minimal synthetic BIT: sig + version + padding, then four entries
        // and a terminator (chain length must clear the >=4 plausibility bar)
        let mut img = vec![0u8; 0x400];
        img[0] = 0x55;
        img[1] = 0xAA;
        let bit = 0x40;
        img[bit..bit + 3].copy_from_slice(b"BIT");
        img[bit + 3] = 0x00;
        let t = bit + 16;
        img[t..t + 6].copy_from_slice(&[b'P', 2, 4, 0, 0x00, 0x01]);
        img[t + 6..t + 12].copy_from_slice(&[b'V', 1, 6, 0, 0x10, 0x01]);
        img[t + 12..t + 18].copy_from_slice(&[b'S', 2, 8, 0, 0x20, 0x01]);
        img[t + 18..t + 24].copy_from_slice(&[b'i', 2, 10, 0, 0x30, 0x01]);
        img[t + 24..t + 30].copy_from_slice(&[0; 6]);
        let sum = parse_bit(&img).expect("parse");
        assert_eq!(sum.bit_offset, bit);
        assert_eq!(sum.tokens.len(), 4);
        assert_eq!(sum.tokens[0].token, 'P');
        assert_eq!(sum.tokens[1].token, 'V');
        assert_eq!(sum.tokens[1].size, 6);
        assert_eq!(sum.tokens[3].token, 'i');
    }

    #[test]
    fn no_bit_signature_errors() {
        let img = vec![0u8; 0x100];
        assert!(parse_bit(&img).is_err());
    }

    #[test]
    fn token_raw_slices_sub_table() {
        let mut img = vec![0u8; 0x400];
        img[0] = 0x55;
        img[1] = 0xAA;
        let bit = 0x40;
        img[bit..bit + 3].copy_from_slice(b"BIT");
        img[bit + 3] = 0x00;
        let t = bit + 16;
        // 'P' size=4 at BIT+0x100 with payload DE AD BE EF
        img[t..t + 6].copy_from_slice(&[b'P', 2, 4, 0, 0x00, 0x01]);
        img[t + 6..t + 12].copy_from_slice(&[b'V', 1, 2, 0, 0x10, 0x01]);
        img[t + 12..t + 18].copy_from_slice(&[b'S', 2, 2, 0, 0x20, 0x01]);
        img[t + 18..t + 24].copy_from_slice(&[b'i', 2, 2, 0, 0x30, 0x01]);
        img[t + 24..t + 30].copy_from_slice(&[0; 6]);
        img[bit + 0x100..bit + 0x104].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let sum = parse_bit(&img).expect("parse");
        assert_eq!(
            sum.token_raw(&img, 'P'),
            Some(&[0xde, 0xad, 0xbe, 0xef][..])
        );
        assert_eq!(sum.token_raw(&img, 'Q'), None);
    }
}
