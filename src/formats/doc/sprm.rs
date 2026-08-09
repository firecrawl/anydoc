//! Sprm (single property modifier) walking and application, per MS-DOC:
//! character toggles resolve against the style-chain base (0 = off, 1 = on,
//! 0x80 = style's value, 0x81 = style's value inverted).

use crate::model::{Style, VertAlign};
use crate::shared::binary::{get_u16, get_u32};

fn sprm_operand_len(sprm: u16, operand: &[u8]) -> usize {
    match sprm >> 13 {
        0 | 1 => 1,
        2 | 4 | 5 => 2,
        3 => 4,
        7 => 3,
        _ => {
            if sprm == 0xD608 {
                operand.get(..2).map(|b| u16::from_le_bytes([b[0], b[1]]) as usize + 1).unwrap_or(0)
            } else {
                operand.first().map(|&b| b as usize + 1).unwrap_or(0)
            }
        }
    }
}

pub fn walk_sprms(grpprl: &[u8], mut f: impl FnMut(u16, &[u8])) {
    let mut pos = 0;
    while pos + 2 <= grpprl.len() {
        let sprm = u16::from_le_bytes([grpprl[pos], grpprl[pos + 1]]);
        pos += 2;
        let len = sprm_operand_len(sprm, &grpprl[pos..]);
        let Some(operand) = grpprl.get(pos..).and_then(|rest| rest.get(..len)) else {
            break;
        };
        f(sprm, operand);
        pos += len;
    }
}

fn get_i16(operand: &[u8], at: usize) -> Option<i16> {
    operand.get(at..at + 2).map(|b| i16::from_le_bytes([b[0], b[1]]))
}

/// Resolve a toggle operand against the style-chain base value.
fn toggle(operand: &[u8], base: bool) -> Option<bool> {
    match operand.first() {
        Some(0) => Some(false),
        Some(1) => Some(true),
        Some(0x80) => Some(base),
        Some(0x81) => Some(!base),
        _ => None,
    }
}

/// The `sprmCIstd` character-style reference in a CHPX, if any.
pub fn chpx_istd(grpprl: &[u8]) -> Option<u16> {
    let mut istd = None;
    walk_sprms(grpprl, |sprm, operand| {
        if sprm == 0x4A30 {
            istd = get_u16(operand, 0);
        }
    });
    istd
}

/// The `sprmCPicLocation` Data-stream offset in a CHPX, if any: where an
/// inline picture's PICF + OfficeArt data begins.
pub fn chpx_pic_location(grpprl: &[u8]) -> Option<u32> {
    let mut location = None;
    walk_sprms(grpprl, |sprm, operand| {
        if sprm == 0x6A03 {
            location = get_u32(operand, 0);
        }
    });
    location
}

/// Apply a CHPX grpprl over `current`, resolving toggle operands against the
/// style chain's value (`style_base`), per the published algorithm.
pub fn apply_chpx(grpprl: &[u8], current: Style, style_base: Style) -> Style {
    let mut style = current;
    walk_sprms(grpprl, |sprm, operand| match sprm {
        // sprmCFBold / sprmCFItalic / sprmCFStrike are toggles.
        0x0835 => {
            if let Some(v) = toggle(operand, style_base.bold) {
                style.bold = v;
            }
        }
        0x0836 => {
            if let Some(v) = toggle(operand, style_base.italic) {
                style.italic = v;
            }
        }
        0x0837 => {
            if let Some(v) = toggle(operand, style_base.strike) {
                style.strike = v;
            }
        }
        // sprmCIss: 0 none, 1 superscript, 2 subscript. Not a toggle, so it
        // resolves against nothing and simply sets.
        0x2A48 => {
            if let Some(&iss) = operand.first() {
                style.vert_align = match iss {
                    1 => VertAlign::Superscript,
                    2 => VertAlign::Subscript,
                    _ => VertAlign::Baseline,
                };
            }
        }
        // sprmCHpsPos: a signed half-point offset from the baseline, which is
        // how Word records "raised/lowered by" rather than the checkbox.
        0x4845 => {
            if let Some(pos) = get_i16(operand, 0) {
                style.vert_align = match pos {
                    0 => VertAlign::Baseline,
                    pos if pos > 0 => VertAlign::Superscript,
                    _ => VertAlign::Subscript,
                };
            }
        }
        _ => {}
    });
    style
}

/// A row's table properties from `sprmTDefTable` and its companion table
/// sprms in the row-end PAPX (TAPX).
#[derive(Debug, Clone, Default)]
pub struct Tap {
    /// Cell boundary positions in twips (`rgdxaCenter`), one more entry
    /// than the cell count.
    pub boundaries: Vec<i16>,
    pub cells: Vec<TapCell>,
    /// `sprmTTableHeader`: the row repeats as a header row.
    pub header: bool,
}

/// Merge flags of one cell (TC80 `tcgrf` / `sprmTVertMerge`).
#[derive(Debug, Clone, Copy, Default)]
pub struct TapCell {
    pub horz_first: bool,
    pub horz_cont: bool,
    pub vert_restart: bool,
    pub vert_cont: bool,
}

/// Paragraph properties a PAPX (or style PAPX) contributes.
#[derive(Debug, Clone, Default)]
pub struct PapDelta {
    pub in_table: Option<bool>,
    pub ttp: Option<bool>,
    pub outline: Option<Option<u8>>,
    pub ilfo: Option<u16>,
    pub ilvl: Option<u8>,
    /// `sprmPItap` table depth (1 = a regular table).
    pub itap: Option<i32>,
    /// `sprmPFInnerTableCell` / `sprmPFInnerTtp`: nested-table cell/row
    /// terminators.
    pub inner_cell: Option<bool>,
    pub inner_ttp: Option<bool>,
    /// Row properties, present on TTP marks.
    pub tap: Option<Tap>,
}

impl PapDelta {
    pub fn merge(self, over: PapDelta) -> PapDelta {
        PapDelta {
            in_table: over.in_table.or(self.in_table),
            ttp: over.ttp.or(self.ttp),
            outline: over.outline.or(self.outline),
            ilfo: over.ilfo.or(self.ilfo),
            ilvl: over.ilvl.or(self.ilvl),
            itap: over.itap.or(self.itap),
            inner_cell: over.inner_cell.or(self.inner_cell),
            inner_ttp: over.inner_ttp.or(self.inner_ttp),
            tap: over.tap.or(self.tap),
        }
    }
}

pub fn apply_pap_sprms(grpprl: &[u8], data: &[u8], delta: &mut PapDelta) {
    walk_sprms(grpprl, |sprm, operand| match sprm {
        // sprmPFInTable / sprmPFTtp
        0x2416 => delta.in_table = Some(operand.first().is_some_and(|&v| v != 0)),
        0x2417 => delta.ttp = Some(operand.first().is_some_and(|&v| v != 0)),
        // sprmPHugePapx: the real grpprl lives length-prefixed in Data.
        0x6646 => {
            if let Some(off) = get_u32(operand, 0).map(|v| v as usize)
                && let Some(cb) = get_u16(data, off).map(|v| v as usize)
                && let Some(huge) = data.get(off..).and_then(|rest| rest.get(2..))
                && let Some(huge) = huge.get(..cb)
            {
                apply_pap_sprms(huge, &[], delta);
            }
        }
        // sprmPOutLvl
        0x2640 => {
            if let Some(&v) = operand.first() {
                delta.outline = Some(if v < 9 { Some(v + 1) } else { None });
            }
        }
        // sprmPIlvl / sprmPIlfo
        0x260A => delta.ilvl = operand.first().copied(),
        0x460B => delta.ilfo = get_u16(operand, 0),
        // sprmPItap / sprmPDtap: table nesting depth.
        0x6649 => delta.itap = get_u32(operand, 0).map(|v| v as i32),
        0x664A => {
            if let Some(d) = get_u32(operand, 0).map(|v| v as i32) {
                delta.itap = Some(delta.itap.unwrap_or(0).saturating_add(d));
            }
        }
        // sprmPFInnerTableCell / sprmPFInnerTtp
        0x244B => delta.inner_cell = Some(operand.first().is_some_and(|&v| v != 0)),
        0x244C => delta.inner_ttp = Some(operand.first().is_some_and(|&v| v != 0)),
        // sprmTDefTable: the row's cell boundaries and TC80 merge flags.
        0xD608 => {
            if let Some(tap) = parse_tdef_table(operand) {
                let header = delta.tap.as_ref().is_some_and(|t| t.header);
                delta.tap = Some(Tap { header, ..tap });
            }
        }
        // sprmTTableHeader
        0x3404 => {
            let on = operand.first().is_some_and(|&v| v != 0);
            match &mut delta.tap {
                Some(tap) => tap.header = on,
                None if on => delta.tap = Some(Tap { header: true, ..Default::default() }),
                None => {}
            }
        }
        // sprmTVertMerge: per-cell vertical merge state.
        0xD62B => {
            if let (Some(&itc), Some(&flag)) = (operand.get(1), operand.get(2))
                && let Some(tap) = &mut delta.tap
                && let Some(cell) = tap.cells.get_mut(itc as usize)
            {
                // VerticalMergeFlag: 1 = continuation, 3 = restart.
                cell.vert_cont = flag == 0x01;
                cell.vert_restart = flag == 0x03;
            }
        }
        _ => {}
    });
}

/// Parse a `TDefTableOperand`: cb, NumberOfColumns, `rgdxaCenter`
/// boundaries, then TC80 records (which may be fewer than the columns).
fn parse_tdef_table(operand: &[u8]) -> Option<Tap> {
    const TC80_SIZE: usize = 20;
    let columns = *operand.get(2)? as usize;
    if columns > 63 {
        return None;
    }
    let mut boundaries = Vec::with_capacity(columns + 1);
    for i in 0..=columns {
        boundaries.push(get_u16(operand, 3 + i * 2)? as i16);
    }
    let mut cells = vec![TapCell::default(); columns];
    let tc_base = 3 + (columns + 1) * 2;
    for (i, cell) in cells.iter_mut().enumerate() {
        let Some(tcgrf) = get_u16(operand, tc_base + i * TC80_SIZE) else {
            break; // fewer TC80s than columns: defaults apply
        };
        // TCGRF: horzMerge (bits 0-1), textFlow (2-4), vertMerge (5-6).
        let horz = tcgrf & 0x3;
        cell.horz_cont = horz == 1;
        cell.horz_first = horz >= 2;
        let vert = (tcgrf >> 5) & 0x3;
        cell.vert_cont = vert == 1;
        cell.vert_restart = vert == 3;
    }
    Some(Tap { boundaries, cells, header: false })
}

/// A CHPX grpprl interpreted as a character-style *definition* layer: the
/// parent's value is the base for its toggles.
pub fn apply_style_chpx(grpprl: &[u8], parent: Style) -> Style {
    apply_chpx(grpprl, parent, parent)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chpx(sprm: u16, operand: &[u8]) -> Vec<u8> {
        let mut out = sprm.to_le_bytes().to_vec();
        out.extend_from_slice(operand);
        out
    }

    #[test]
    fn sprm_ciss_says_which_way_and_zero_says_neither() {
        for (iss, expected) in
            [(0u8, VertAlign::Baseline), (1, VertAlign::Superscript), (2, VertAlign::Subscript)]
        {
            let style = apply_chpx(&chpx(0x2A48, &[iss]), Style::PLAIN, Style::PLAIN);
            assert_eq!(style.vert_align, expected, "iss {iss}");
        }
    }

    #[test]
    fn sprm_chpspos_reads_the_sign_of_the_offset() {
        for (offset, expected) in
            [(0i16, VertAlign::Baseline), (6, VertAlign::Superscript), (-6, VertAlign::Subscript)]
        {
            let style =
                apply_chpx(&chpx(0x4845, &offset.to_le_bytes()), Style::PLAIN, Style::PLAIN);
            assert_eq!(style.vert_align, expected, "offset {offset}");
        }
    }

    #[test]
    fn a_script_sprm_leaves_the_other_properties_alone() {
        let base = Style { bold: true, ..Style::PLAIN };
        let style = apply_chpx(&chpx(0x2A48, &[1]), base, base);
        assert!(style.bold);
        assert_eq!(style.vert_align, VertAlign::Superscript);
    }

    #[test]
    fn a_truncated_operand_changes_nothing() {
        let style = apply_chpx(&[0x45, 0x48, 0x06], Style::PLAIN, Style::PLAIN);
        assert_eq!(style.vert_align, VertAlign::Baseline);
    }
}
