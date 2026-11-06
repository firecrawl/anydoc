//! Full STSH (style sheet) parsing per MS-DOC: STD records with their
//! `istdBase` inheritance chains and UPX formatting payloads, resolved into
//! effective per-style character formatting and paragraph properties.

use crate::formats::doc::sprm::{PapDelta, apply_pap_sprms, apply_style_chpx};
use crate::model::Style;
use crate::shared::binary::{get_u16, get_u32, utf16le_units};
use crate::shared::blockstyle::{self, BlockStyle};
use std::collections::HashMap;

const ISTD_NIL: u16 = 0x0FFF;

#[derive(Debug, Clone, Default)]
pub struct ResolvedStyle {
    /// Effective character formatting of the chain.
    pub chp: Style,
    /// Effective paragraph properties of the chain.
    pub pap: PapDelta,
    /// Heading level for built-in `heading N` styles.
    pub heading: Option<u8>,
    /// The block container the style's name designates.
    pub block: Option<BlockStyle>,
}

#[derive(Debug, Default)]
pub struct Stylesheet {
    resolved: HashMap<u16, ResolvedStyle>,
    /// Returned for unknown istds so `get` never allocates.
    default: ResolvedStyle,
}

impl Stylesheet {
    pub fn get(&self, istd: u16) -> &ResolvedStyle {
        self.resolved.get(&istd).unwrap_or(&self.default)
    }
}

struct Std {
    sti: u16,
    istd_base: u16,
    /// The block container this style's name designates.
    block: Option<BlockStyle>,
    /// Paragraph style: (papx grpprl incl. leading istd, chpx grpprl);
    /// character style: (empty, chpx grpprl).
    upx_papx: Vec<u8>,
    upx_chpx: Vec<u8>,
    is_paragraph: bool,
}

pub fn parse(word_doc: &[u8], table: &[u8]) -> Stylesheet {
    let Some(fc) = get_u32(word_doc, 0xA2).map(|v| v as usize) else {
        return Stylesheet::default();
    };
    let Some(lcb) = get_u32(word_doc, 0xA6).map(|v| v as usize) else {
        return Stylesheet::default();
    };
    let Some(stsh) = table.get(fc..fc.saturating_add(lcb)) else {
        return Stylesheet::default();
    };
    let (Some(cb_stshi), Some(cstd), Some(cb_std_base)) =
        (get_u16(stsh, 0), get_u16(stsh, 2), get_u16(stsh, 4))
    else {
        return Stylesheet::default();
    };

    let mut stds: HashMap<u16, Std> = HashMap::new();
    let mut pos = 2 + cb_stshi as usize;
    for istd in 0..cstd {
        let Some(cb_std) = get_u16(stsh, pos) else {
            break;
        };
        pos += 2;
        if cb_std == 0 {
            continue;
        }
        let Some(record) = stsh.get(pos..).and_then(|rest| rest.get(..cb_std as usize)) else {
            break;
        };
        pos += cb_std as usize;
        if let Some(std) = parse_std(record, cb_std_base as usize) {
            stds.insert(istd, std);
        }
    }

    let mut sheet = Stylesheet::default();
    let mut memo: HashMap<u16, ResolvedStyle> = HashMap::new();
    for &istd in stds.keys() {
        let resolved = resolve(istd, &stds, &mut memo);
        sheet.resolved.insert(istd, resolved);
    }
    sheet
}

fn parse_std(record: &[u8], cb_std_base: usize) -> Option<Std> {
    let first = get_u16(record, 0)?;
    let sti = first & 0x0FFF;
    let second = get_u16(record, 2)?;
    let sgc = second & 0x000F; // 1 = paragraph, 2 = character
    let istd_base = (second >> 4) & 0x0FFF;
    let cupx = get_u16(record, 4).map(|v| v & 0x000F).unwrap_or(0);

    // The name (Xstz) follows the fixed STDF area; UPX payloads follow the
    // name, each 2-byte aligned relative to the record start.
    let name_off = cb_std_base.max(10);
    let name_len = get_u16(record, name_off)? as usize;
    // Unicode name: length prefix + UTF-16 chars + terminator.
    let name_bytes = name_len.checked_mul(2)?;
    let name_units: Vec<u16> = utf16le_units(
        record
            .get(name_off..)
            .and_then(|rest| rest.get(2..))
            .and_then(|rest| rest.get(..name_bytes))
            .unwrap_or_default(),
    )
    .collect();
    let name = String::from_utf16_lossy(&name_units);
    let mut upx_pos = name_off.checked_add(4)?.checked_add(name_bytes)?;

    let mut upx: Vec<&[u8]> = Vec::new();
    for _ in 0..cupx {
        if upx_pos % 2 == 1 {
            upx_pos += 1;
        }
        let cb = get_u16(record, upx_pos)? as usize;
        let payload = record.get(upx_pos..)?.get(2..)?.get(..cb)?;
        upx.push(payload);
        upx_pos += 2 + cb;
    }

    let is_paragraph = sgc == 1;
    let (upx_papx, upx_chpx) = if is_paragraph {
        (
            upx.first().copied().unwrap_or_default().to_vec(),
            upx.get(1).copied().unwrap_or_default().to_vec(),
        )
    } else {
        (Vec::new(), upx.first().copied().unwrap_or_default().to_vec())
    };
    Some(Std {
        sti,
        istd_base,
        block: blockstyle::from_style_name(&name),
        upx_papx,
        upx_chpx,
        is_paragraph,
    })
}

/// Resolve one style's `istdBase` chain, memoized across styles. Cycles
/// (self-referencing bases exist in corrupt files) are cut by a visited set
/// and resolve from their acyclic prefix; valid chains of any depth resolve
/// fully.
fn resolve(
    istd: u16,
    stds: &HashMap<u16, Std>,
    memo: &mut HashMap<u16, ResolvedStyle>,
) -> ResolvedStyle {
    if let Some(hit) = memo.get(&istd) {
        return hit.clone();
    }
    // Walk root-ward collecting the unresolved suffix of the chain.
    let mut chain: Vec<u16> = Vec::new();
    let mut visiting: std::collections::HashSet<u16> = std::collections::HashSet::new();
    let mut base = ResolvedStyle::default();
    let mut cursor = Some(istd);
    while let Some(cur) = cursor {
        if let Some(hit) = memo.get(&cur) {
            base = hit.clone();
            break;
        }
        if !visiting.insert(cur) {
            log::warn!("style istdBase cycle at istd {cur}");
            break;
        }
        let Some(std) = stds.get(&cur) else {
            break;
        };
        chain.push(cur);
        cursor = (std.istd_base != ISTD_NIL && std.istd_base != cur).then_some(std.istd_base);
    }
    // Apply the chain root-to-leaf over the resolved base.
    for &cur in chain.iter().rev() {
        let std = &stds[&cur];
        if std.is_paragraph && std.upx_papx.len() >= 2 {
            // The paragraph UPX starts with the style's own istd.
            let mut delta = PapDelta::default();
            apply_pap_sprms(&std.upx_papx[2..], &[], &mut delta);
            base.pap = base.pap.merge(delta);
        }
        base.chp = apply_style_chpx(&std.upx_chpx, base.chp);
        if let sti @ 1..=9 = std.sti {
            base.heading = Some(sti as u8);
        }
        base.block = std.block.or(base.block);
        memo.insert(cur, base.clone());
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style(sti: u16, istd_base: u16) -> Std {
        Std {
            sti,
            istd_base,
            block: None,
            upx_papx: Vec::new(),
            upx_chpx: Vec::new(),
            is_paragraph: true,
        }
    }

    #[test]
    fn deep_istd_base_chains_resolve_fully() {
        // A valid chain far beyond the old 16-style cutoff must still
        // inherit from its root.
        let mut stds = HashMap::new();
        stds.insert(0u16, style(3, ISTD_NIL)); // built-in heading 3
        for i in 1u16..=40 {
            stds.insert(i, style(0x0FFE, i - 1));
        }
        let mut memo = HashMap::new();
        assert_eq!(resolve(40, &stds, &mut memo).heading, Some(3));
    }

    #[test]
    fn istd_base_cycles_resolve_from_the_acyclic_prefix() {
        let mut stds = HashMap::new();
        stds.insert(0u16, style(2, 1)); // based on a member of the cycle
        stds.insert(1u16, style(0x0FFE, 2));
        stds.insert(2u16, style(0x0FFE, 1));
        let mut memo = HashMap::new();
        assert_eq!(resolve(0, &stds, &mut memo).heading, Some(2));
    }
}
