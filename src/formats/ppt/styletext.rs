//! `StyleTextPropAtom` and `TxMasterStyleAtom` parsing: paragraph and
//! character formatting runs keyed by text range, per MS-PPT's
//! TextPFException / TextCFException layouts. Parsing is defensive - a
//! malformed exception aborts styling for that atom (logged), never the text.

use crate::model::VertAlign;
use crate::shared::binary::{get_u16, get_u32};

#[derive(Debug, Clone, Copy, Default)]
pub struct ParaProps {
    pub count: usize,
    pub depth: u16,
    /// From bulletFlags' fHasBullet, when present.
    pub bullet: Option<bool>,
}

/// Character-run exception: each property is tri-state — `None` inherits the
/// master's per-level default.
#[derive(Debug, Clone, Copy, Default)]
pub struct CharProps {
    pub count: usize,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub vert_align: Option<VertAlign>,
}

/// One indent level's defaults from a `TxMasterStyleAtom`, tri-state.
#[derive(Debug, Clone, Copy, Default)]
pub struct MasterLevel {
    pub bullet: Option<bool>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
}

#[derive(Debug, Default)]
pub struct StyleRuns {
    pub paragraphs: Vec<ParaProps>,
    pub chars: Vec<CharProps>,
}

/// Parse a `StyleTextPropAtom` body for text of `text_len` characters.
pub fn parse_style_text(body: &[u8], text_len: usize) -> StyleRuns {
    let mut runs = StyleRuns::default();
    let mut pos = 0usize;
    // Paragraph runs cover text_len + 1 (the implicit final paragraph mark).
    let mut covered = 0usize;
    while covered <= text_len {
        let Some(count) = get_u32(body, pos).map(|v| v as usize) else {
            break;
        };
        let Some(depth) = get_u16(body, pos + 4) else {
            break;
        };
        pos += 6;
        let Some((pf, next)) = parse_pf_exception(body, pos) else {
            log::debug!("unparseable paragraph style run; keeping styling parsed so far");
            return runs;
        };
        pos = next;
        runs.paragraphs.push(ParaProps { count, depth, bullet: pf });
        covered += count;
        if count == 0 {
            break;
        }
    }
    let mut covered = 0usize;
    while covered <= text_len {
        let Some(count) = get_u32(body, pos).map(|v| v as usize) else {
            break;
        };
        pos += 4;
        let Some((cf, next)) = parse_cf_exception(body, pos) else {
            log::debug!("unparseable character style run; dropping remaining styling");
            break;
        };
        pos = next;
        runs.chars.push(CharProps {
            count,
            bold: cf.bold,
            italic: cf.italic,
            vert_align: cf.vert_align,
        });
        covered += count;
        if count == 0 {
            break;
        }
    }
    runs
}

/// TextPFException: mask + fields in mask-bit order. Returns the bullet
/// state (if the mask carried bulletFlags) and the next offset.
fn parse_pf_exception(body: &[u8], mut pos: usize) -> Option<(Option<bool>, usize)> {
    let mask = get_u32(body, pos)?;
    pos += 4;
    let mut bullet = None;
    // masks.bulletFlags: any of hasBullet/bulletHasFont/bulletHasColor/
    // bulletHasSize present -> a 16-bit bulletFlags field. The fHasBullet
    // value is specified only when masks.hasBullet itself is set.
    if mask & 0x000F != 0 {
        let flags = get_u16(body, pos)?;
        if mask & 0x0001 != 0 {
            bullet = Some(flags & 0x0001 != 0);
        }
        pos += 2;
    }
    if mask & 0x0080 != 0 {
        pos += 2; // bulletChar
    }
    if mask & 0x0010 != 0 {
        pos += 2; // bulletFontRef
    }
    if mask & 0x0040 != 0 {
        pos += 2; // bulletSize
    }
    if mask & 0x0020 != 0 {
        pos += 4; // bulletColor
    }
    if mask & 0x0800 != 0 {
        pos += 2; // textAlignment
    }
    if mask & 0x1000 != 0 {
        pos += 2; // lineSpacing
    }
    if mask & 0x2000 != 0 {
        pos += 2; // spaceBefore
    }
    if mask & 0x4000 != 0 {
        pos += 2; // spaceAfter
    }
    if mask & 0x0100 != 0 {
        pos += 2; // leftMargin
    }
    if mask & 0x0400 != 0 {
        pos += 2; // indent
    }
    if mask & 0x8000 != 0 {
        pos += 2; // defaultTabSize
    }
    if mask & 0x0010_0000 != 0 {
        // tabStops: count-prefixed array of 4-byte stops.
        let count = get_u16(body, pos)? as usize;
        pos += 2 + count * 4;
    }
    if mask & 0x0001_0000 != 0 {
        pos += 2; // fontAlign
    }
    // charWrap/wordWrap/overflow share one wrapFlags field.
    if mask & 0x000E_0000 != 0 {
        pos += 2;
    }
    if mask & 0x0020_0000 != 0 {
        pos += 2; // textDirection
    }
    if pos > body.len() {
        return None;
    }
    Some((bullet, pos))
}

/// Tri-state character style bits from a TextCFException.
#[derive(Debug, Clone, Copy, Default)]
struct CfStyle {
    bold: Option<bool>,
    italic: Option<bool>,
    vert_align: Option<VertAlign>,
}

/// TextCFException: mask (+ optional style bitfield) + sized fields. Each
/// style bit is specified only when its own mask bit is set (per-bit
/// tri-state); strike-through is not in the 97-2003 style bits.
fn parse_cf_exception(body: &[u8], mut pos: usize) -> Option<(CfStyle, usize)> {
    let mask = get_u32(body, pos)?;
    pos += 4;
    let mut bold = None;
    let mut italic = None;
    if mask & 0xFFFF != 0 {
        let style = get_u16(body, pos)?;
        if mask & 0x0001 != 0 {
            bold = Some(style & 0x0001 != 0);
        }
        if mask & 0x0002 != 0 {
            italic = Some(style & 0x0002 != 0);
        }
        pos += 2;
    }
    if mask & 0x0001_0000 != 0 {
        pos += 2; // fontRef
    }
    if mask & 0x0020_0000 != 0 {
        pos += 2; // oldEAFontRef
    }
    if mask & 0x0040_0000 != 0 {
        pos += 2; // ansiFontRef
    }
    if mask & 0x0080_0000 != 0 {
        pos += 2; // symbolFontRef
    }
    if mask & 0x0002_0000 != 0 {
        pos += 2; // size
    }
    if mask & 0x0004_0000 != 0 {
        pos += 4; // color
    }
    // position: the baseline offset as a signed percentage of the font size.
    let mut vert_align = None;
    if mask & 0x0008_0000 != 0 {
        vert_align = Some(match get_u16(body, pos)? as i16 {
            0 => VertAlign::Baseline,
            offset if offset > 0 => VertAlign::Superscript,
            _ => VertAlign::Subscript,
        });
        pos += 2;
    }
    if pos > body.len() {
        return None;
    }
    Some((CfStyle { bold, italic, vert_align }, pos))
}

/// A `TxMasterStyleAtom`: per-indent-level tri-state defaults
/// (index = depth).
pub fn parse_master_style(body: &[u8], instance: u16) -> Vec<MasterLevel> {
    let Some(levels) = get_u16(body, 0).map(|v| v as usize) else {
        return Vec::new();
    };
    let mut pos = 2usize;
    let mut out = Vec::new();
    for _ in 0..levels.min(10) {
        // Levels for body-family text types carry a leading depth field in
        // format version >= 9 atoms; instances >= 5 always do.
        if instance >= 5 {
            pos += 2;
        }
        let Some((bullet, next)) = parse_pf_exception(body, pos) else {
            break;
        };
        pos = next;
        let Some((cf, next)) = parse_cf_exception(body, pos) else {
            break;
        };
        pos = next;
        out.push(MasterLevel { bullet, bold: cf.bold, italic: cf.italic });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TextCFException carrying only the position field.
    fn exception(position: i16) -> Vec<u8> {
        let mut out = 0x0008_0000u32.to_le_bytes().to_vec();
        out.extend_from_slice(&position.to_le_bytes());
        out
    }

    #[test]
    fn the_position_percentage_says_which_side_of_the_baseline() {
        for (position, expected) in
            [(0i16, VertAlign::Baseline), (30, VertAlign::Superscript), (-25, VertAlign::Subscript)]
        {
            let (style, pos) = parse_cf_exception(&exception(position), 0).unwrap();
            assert_eq!(style.vert_align, Some(expected), "position {position}");
            assert_eq!(pos, 6);
        }
    }

    #[test]
    fn an_absent_position_leaves_the_run_where_the_master_put_it() {
        let (style, _) = parse_cf_exception(&0u32.to_le_bytes(), 0).unwrap();
        assert_eq!(style.vert_align, None);
    }
}
