//! Legacy Word 97-2003 binary (.doc), implementing the published resolution
//! algorithms: OLE2 container, FIB, piece table with `Prm`s, CHPX/PAPX runs
//! applied over the STSH style chains in specification order, and the
//! PlfLst/PlfLfo list tables. Character positions are counted in UTF-16
//! units, matching the CP-indexed PLC structures.

mod lists;
mod sprm;
mod stsh;

use crate::error::ConvertError;
use crate::model::{
    Block, Document, DocumentMeta, ImageSource, Inline, Note, NoteKind, Style, inlines_are_empty,
    inlines_to_plain_text,
};
use crate::package::limits;
use crate::shared::assets::AssetSink;
use crate::shared::binary::{get_u16, get_u32, read_ole_stream};
use crate::shared::delta::rebase_emphasis;
use crate::shared::fields::{FieldFrame, field_result};
use crate::shared::grid::{CellProp, GridRow, build_edge_table};
use crate::shared::list::MarkerKind;
use crate::shared::list::{ListEntry, ListKey, flush_list};
use lists::{LEVELS, ListDef, Lists};
use sprm::{PapDelta, Tap, apply_chpx, apply_pap_sprms, chpx_istd};
use std::collections::HashMap;
use std::io::Cursor;
use stsh::Stylesheet;

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let cursor = Cursor::new(bytes);
    let mut ole = cfb::CompoundFile::open(cursor)
        .map_err(|e| ConvertError::malformed(format!("not an OLE2 compound file: {e}")))?;

    let word_doc = read_ole_stream(&mut ole, "WordDocument")?;
    if get_u16(&word_doc, 0) != Some(0xA5EC) {
        return Err(ConvertError::malformed_part("WordDocument", "invalid FIB magic".to_string()));
    }
    let flags = get_u16(&word_doc, 0x0A).unwrap_or(0);
    if flags & 0x0100 != 0 {
        return Err(ConvertError::Encrypted);
    }
    let (table_name, other_table) =
        if flags & 0x0200 != 0 { ("1Table", "0Table") } else { ("0Table", "1Table") };
    let table = read_ole_stream(&mut ole, table_name)
        .or_else(|_| read_ole_stream(&mut ole, other_table))
        .unwrap_or_default();

    let ccp_text = get_u32(&word_doc, 0x4C).unwrap_or(0) as usize;
    let ccp_ftn = get_u32(&word_doc, 0x50).unwrap_or(0) as usize;
    let ccp_hdd = get_u32(&word_doc, 0x54).unwrap_or(0) as usize;
    let ccp_mcr = get_u32(&word_doc, 0x58).unwrap_or(0) as usize;
    let ccp_atn = get_u32(&word_doc, 0x5C).unwrap_or(0) as usize;
    let ccp_edn = get_u32(&word_doc, 0x60).unwrap_or(0) as usize;
    let fc_clx = get_u32(&word_doc, 0x1A2).unwrap_or(0) as usize;
    let lcb_clx = get_u32(&word_doc, 0x1A6).unwrap_or(0) as usize;

    let (pieces, prcs) = if lcb_clx > 0 {
        parse_clx(&table, fc_clx, lcb_clx)?
    } else {
        (legacy_single_piece(&word_doc), Vec::new())
    };
    let total_cp = ccp_text + ccp_ftn + ccp_hdd + ccp_mcr + ccp_atn + ccp_edn;
    // Compressed (8-bit) piece text decodes in the document's ANSI code
    // page: FibBase.lid, or FibRgW97.lidFE when fFarEast is set.
    let lid = if flags & 0x4000 != 0 {
        get_u16(&word_doc, 0x3C).filter(|&l| l != 0).or_else(|| get_u16(&word_doc, 0x06))
    } else {
        get_u16(&word_doc, 0x06)
    };
    let encoding = lid_encoding(lid.unwrap_or(0));
    let text = extract_text(&word_doc, &pieces, total_cp, encoding);

    let data = read_ole_stream(&mut ole, "Data").unwrap_or_default();
    let chpx_runs = parse_fkps(&word_doc, &table, 0xFA, FkpKind::Chpx, &data);
    let papx_runs = parse_fkps(&word_doc, &table, 0x102, FkpKind::Papx, &data);
    let stylesheet = stsh::parse(&word_doc, &table);
    let list_tables = lists::parse(&word_doc, &table);

    // Note references (CP-indexed) and note body ranges.
    let mut note_refs: HashMap<usize, String> = HashMap::new();
    let mut note_ranges: Vec<(usize, usize, String, NoteKind)> = Vec::new();
    let ftn_base = ccp_text;
    let edn_base = ccp_text + ccp_ftn + ccp_hdd + ccp_mcr + ccp_atn;
    for (ref_off, txt_off, base, prefix, kind) in [
        (0xAA, 0xB2, ftn_base, "fn", NoteKind::Footnote),
        (0x20A, 0x212, edn_base, "en", NoteKind::Endnote),
    ] {
        let (ref_cps, n_refs) = parse_plc(&word_doc, &table, ref_off, 2);
        let (txt_cps, _) = parse_plc(&word_doc, &table, txt_off, 0);
        for i in 0..n_refs {
            note_refs.insert(text.index_of_cp(ref_cps[i] as usize), format!("{prefix}{i}"));
            if i + 1 < txt_cps.len() {
                let lo = text.index_of_cp(base + txt_cps[i] as usize);
                let hi = text.index_of_cp(base + txt_cps[i + 1] as usize);
                note_ranges.push((lo, hi, format!("{prefix}{i}"), kind));
            }
        }
    }
    let main_end = text.index_of_cp(ccp_text);

    let piece_prcs: Vec<Option<usize>> = pieces.iter().map(|p| p.prm_prc).collect();
    let assembler = Assembler {
        text,
        chpx: Runs::new(chpx_runs),
        papx: Runs::new(papx_runs),
        stylesheet,
        lists: list_tables,
        prcs,
        piece_prcs,
        note_refs,
        counters: std::cell::RefCell::new(Counters::default()),
        data,
        assets: std::cell::RefCell::new(AssetSink::new()),
    };
    let blocks = assembler.build_blocks(0, main_end)?;
    let mut notes = Vec::new();
    for (lo, hi, id, kind) in note_ranges {
        let lo = lo.min(assembler.text.chars.len());
        let hi = hi.min(assembler.text.chars.len());
        if lo >= hi {
            continue;
        }
        notes.push(Note { id, kind, blocks: assembler.build_blocks(lo, hi)? });
    }
    let assets = std::mem::take(&mut assembler.assets.borrow_mut().assets);
    Ok(Document { meta: DocumentMeta::default(), blocks, notes, assets })
}

/// Read a PLC's CP array; n is the number of data elements.
fn parse_plc(word_doc: &[u8], table: &[u8], fib_off: usize, data_size: usize) -> (Vec<u32>, usize) {
    let fc = get_u32(word_doc, fib_off).unwrap_or(0) as usize;
    let lcb = get_u32(word_doc, fib_off + 4).unwrap_or(0) as usize;
    let Some(plc) = table.get(fc..fc.saturating_add(lcb)) else {
        return (Vec::new(), 0);
    };
    if lcb < 8 {
        return (Vec::new(), 0);
    }
    let n = if data_size == 0 { lcb / 4 - 1 } else { (lcb - 4) / (4 + data_size) };
    let mut cps = Vec::with_capacity(n + 1);
    for i in 0..=n {
        cps.push(get_u32(plc, i * 4).unwrap_or(0));
    }
    (cps, n)
}

// ---------------------------------------------------------------------------
// Piece table

struct Piece {
    cp_start: usize,
    cp_end: usize,
    fc: usize,
    compressed: bool,
    /// Property modifier: an index into the Clx `Prc` array, when complex.
    prm_prc: Option<usize>,
}

fn parse_clx(
    table: &[u8],
    fc: usize,
    lcb: usize,
) -> Result<(Vec<Piece>, Vec<Vec<u8>>), ConvertError> {
    let clx =
        table.get(fc..fc + lcb).ok_or_else(|| ConvertError::malformed("Clx out of bounds"))?;
    let mut prcs: Vec<Vec<u8>> = Vec::new();
    let mut pos = 0;
    loop {
        match clx.get(pos) {
            Some(1) => {
                let cb = get_u16(clx, pos + 1).ok_or_else(|| ConvertError::malformed("bad Prc"))?
                    as usize;
                if let Some(grpprl) = clx.get(pos + 3..pos + 3 + cb) {
                    prcs.push(grpprl.to_vec());
                }
                pos += 3 + cb;
            }
            Some(2) => {
                let lcb_plc = get_u32(clx, pos + 1)
                    .ok_or_else(|| ConvertError::malformed("bad Pcdt"))?
                    as usize;
                let plc = clx
                    .get(pos + 5..pos + 5 + lcb_plc)
                    .ok_or_else(|| ConvertError::malformed("PlcPcd out of bounds"))?;
                let pieces = parse_plc_pcd(plc, &mut prcs)?;
                return Ok((pieces, prcs));
            }
            _ => return Err(ConvertError::malformed("malformed Clx")),
        }
    }
}

fn parse_plc_pcd(plc: &[u8], prcs: &mut Vec<Vec<u8>>) -> Result<Vec<Piece>, ConvertError> {
    if plc.len() < 4 + 12 {
        return Err(ConvertError::malformed("empty piece table"));
    }
    let n = (plc.len() - 4) / 12;
    let mut pieces = Vec::with_capacity(n);
    for i in 0..n {
        let cp_start =
            get_u32(plc, i * 4).ok_or_else(|| ConvertError::malformed("bad cp"))? as usize;
        let cp_end =
            get_u32(plc, (i + 1) * 4).ok_or_else(|| ConvertError::malformed("bad cp"))? as usize;
        let pcd_off = (n + 1) * 4 + i * 8;
        let fc_raw = get_u32(plc, pcd_off + 2).ok_or_else(|| ConvertError::malformed("bad pcd"))?;
        let prm = get_u16(plc, pcd_off + 6).unwrap_or(0);
        let compressed = fc_raw & 0x4000_0000 != 0;
        let fc = (fc_raw & 0x3FFF_FFFF) as usize;
        let fc = if compressed { fc / 2 } else { fc };
        // Prm: the complex form points into the Prc array; the compressed
        // form (Prm0) carries one (isprm, val) pair that decodes to a
        // one-sprm grpprl.
        let prm_prc = if prm & 1 != 0 {
            let idx = (prm >> 1) as usize;
            (idx < prcs.len()).then_some(idx)
        } else if prm != 0 {
            match prm0_grpprl(prm) {
                Some(grpprl) => {
                    prcs.push(grpprl);
                    Some(prcs.len() - 1)
                }
                None => {
                    log::debug!("Prm0 sprm outside the converted model: 0x{prm:04X}");
                    None
                }
            }
        } else {
            None
        };
        pieces.push(Piece { cp_start, cp_end, fc, compressed, prm_prc });
    }
    Ok(pieces)
}

/// Decode a `Prm0` (compressed piece Prm): the 7-bit `isprm` selects a Sprm
/// per the [MS-DOC] Prm0 table, `val` is its one-byte operand. Only sprms
/// whose properties the converter models materialize; the rest have no
/// representable effect.
fn prm0_grpprl(prm: u16) -> Option<Vec<u8>> {
    let isprm = (prm >> 1) & 0x7F;
    let val = (prm >> 8) as u8;
    let sprm: u16 = match isprm {
        0x0C => 0x260A, // sprmPIlvl
        0x18 => 0x2416, // sprmPFInTable
        0x19 => 0x2417, // sprmPFTtp
        0x55 => 0x0835, // sprmCFBold
        0x56 => 0x0836, // sprmCFItalic
        0x57 => 0x0837, // sprmCFStrike
        0x78 => 0x2640, // sprmPOutLvl
        _ => return None,
    };
    Some(vec![(sprm & 0xFF) as u8, (sprm >> 8) as u8, val])
}

fn legacy_single_piece(word_doc: &[u8]) -> Vec<Piece> {
    let fc_min = get_u32(word_doc, 0x18).unwrap_or(0) as usize;
    let fc_mac = get_u32(word_doc, 0x1C).unwrap_or(0) as usize;
    if fc_mac <= fc_min {
        return Vec::new();
    }
    vec![Piece {
        cp_start: 0,
        cp_end: fc_mac - fc_min,
        fc: fc_min,
        compressed: true,
        prm_prc: None,
    }]
}

// ---------------------------------------------------------------------------
// Text extraction: CPs count UTF-16 units, chars are decoded scalars.

struct TextStream {
    chars: Vec<char>,
    fcs: Vec<u32>,
    /// CP of each char (running UTF-16 unit count).
    cps: Vec<u32>,
    /// Piece index of each char, for Prm application.
    piece_of: Vec<u32>,
}

impl TextStream {
    /// First char index at or after the given CP (`chars.len()` when the CP
    /// is past the end).
    fn index_of_cp(&self, cp: usize) -> usize {
        self.cps.partition_point(|&c| (c as usize) < cp)
    }
}

/// The ANSI code page for a Word language id (MS-LCID primary language;
/// Chinese needs the region to pick Simplified vs Traditional).
fn lid_encoding(lid: u16) -> &'static encoding_rs::Encoding {
    use encoding_rs::*;
    match lid & 0x03FF {
        0x11 => SHIFT_JIS, // 932
        0x12 => EUC_KR,    // 949
        0x04 => match lid {
            0x0404 | 0x0C04 | 0x1404 | 0x7C04 => BIG5, // 950
            _ => GBK,                                  // 936
        },
        0x01 | 0x20 | 0x29 => WINDOWS_1256, // Arabic script
        0x02 | 0x19 | 0x22 | 0x23 => WINDOWS_1251, // Cyrillic
        0x05 | 0x0E | 0x15 | 0x18 | 0x1A | 0x1B | 0x24 => WINDOWS_1250,
        0x08 => WINDOWS_1253,        // Greek
        0x0D => WINDOWS_1255,        // Hebrew
        0x1E => WINDOWS_874,         // Thai
        0x1F | 0x2C => WINDOWS_1254, // Turkic
        0x25..=0x27 => WINDOWS_1257, // Baltic
        0x2A => WINDOWS_1258,        // Vietnamese
        _ => WINDOWS_1252,
    }
}

/// Whether `b` starts a two-byte sequence in a double-byte code page.
fn is_lead_byte(enc: &'static encoding_rs::Encoding, b: u8) -> bool {
    use encoding_rs::*;
    if enc == SHIFT_JIS {
        matches!(b, 0x81..=0x9F | 0xE0..=0xFC)
    } else if enc == GBK || enc == BIG5 || enc == EUC_KR {
        (0x81..=0xFE).contains(&b)
    } else {
        false
    }
}

fn extract_text(
    word_doc: &[u8],
    pieces: &[Piece],
    total_cp: usize,
    encoding: &'static encoding_rs::Encoding,
) -> TextStream {
    let mut text =
        TextStream { chars: Vec::new(), fcs: Vec::new(), cps: Vec::new(), piece_of: Vec::new() };
    let mut cp = 0usize;
    for (piece_idx, piece) in pieces.iter().enumerate() {
        if cp >= total_cp {
            break;
        }
        let len = piece.cp_end.saturating_sub(piece.cp_start).min(total_cp - cp);
        if piece.compressed {
            let Some(bytes) = word_doc.get(piece.fc..piece.fc + len) else {
                continue;
            };
            // Byte-accurate accounting: in a compressed piece each CP is one
            // byte, so a double-byte character occupies two CPs and its FC is
            // the lead byte's.
            let mut i = 0usize;
            while i < bytes.len() {
                let seq =
                    if is_lead_byte(encoding, bytes[i]) && i + 1 < bytes.len() { 2 } else { 1 };
                let (s, _) = encoding.decode_without_bom_handling(&bytes[i..i + seq]);
                for c in s.chars() {
                    text.chars.push(c);
                    text.fcs.push((piece.fc + i) as u32);
                    text.cps.push(cp as u32);
                    text.piece_of.push(piece_idx as u32);
                }
                cp += seq;
                i += seq;
            }
        } else {
            let Some(bytes) = word_doc.get(piece.fc..piece.fc + len * 2) else {
                continue;
            };
            let units: Vec<u16> =
                bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let mut unit_idx = 0usize;
            for r in char::decode_utf16(units.iter().copied()) {
                let c = r.unwrap_or('\u{fffd}');
                text.chars.push(c);
                text.fcs.push((piece.fc + unit_idx * 2) as u32);
                text.cps.push(cp as u32);
                text.piece_of.push(piece_idx as u32);
                unit_idx += c.len_utf16();
                cp += c.len_utf16();
            }
        }
    }
    text
}

// ---------------------------------------------------------------------------
// Formatting runs (CHPX / PAPX out of FKP pages)

#[derive(Clone, Copy, PartialEq)]
enum FkpKind {
    Chpx,
    Papx,
}

#[derive(Default)]
struct RunProps {
    /// Raw CHPX grpprl (application depends on the style base).
    chpx: Vec<u8>,
    istd: u16,
    pap: PapDelta,
}

struct Run {
    fc_start: u32,
    fc_end: u32,
    props: RunProps,
}

struct Runs {
    runs: Vec<Run>,
}

impl Runs {
    fn new(mut runs: Vec<Run>) -> Self {
        runs.sort_by_key(|r| r.fc_start);
        Runs { runs }
    }

    fn lookup(&self, fc: u32) -> Option<&RunProps> {
        let idx = self.runs.partition_point(|r| r.fc_start <= fc);
        if idx == 0 {
            return None;
        }
        let run = &self.runs[idx - 1];
        (fc < run.fc_end).then_some(&run.props)
    }
}

fn parse_fkps(
    word_doc: &[u8],
    table: &[u8],
    fib_off: usize,
    kind: FkpKind,
    data: &[u8],
) -> Vec<Run> {
    let mut runs = Vec::new();
    let fc = get_u32(word_doc, fib_off).unwrap_or(0) as usize;
    let lcb = get_u32(word_doc, fib_off + 4).unwrap_or(0) as usize;
    let Some(plc) = table.get(fc..fc.saturating_add(lcb)) else {
        return runs;
    };
    if plc.len() < 8 {
        return runs;
    }
    let n = (plc.len() - 4) / 8;
    for i in 0..n {
        let Some(pn_raw) = get_u32(plc, (n + 1) * 4 + i * 4) else {
            continue;
        };
        let pn = (pn_raw & 0x3F_FFFF) as usize;
        let Some(page) = word_doc.get(pn * 512..pn * 512 + 512) else {
            continue;
        };
        parse_fkp_page(page, kind, data, &mut runs);
    }
    runs
}

fn parse_fkp_page(page: &[u8], kind: FkpKind, data: &[u8], runs: &mut Vec<Run>) {
    let count = page[511] as usize;
    if count == 0 {
        return;
    }
    let entry_size = if kind == FkpKind::Papx { 13 } else { 1 };
    for k in 0..count {
        let Some(fc_start) = get_u32(page, k * 4) else {
            continue;
        };
        let Some(fc_end) = get_u32(page, (k + 1) * 4) else {
            continue;
        };
        let b_offset_pos = (count + 1) * 4 + k * entry_size;
        let Some(&b_offset) = page.get(b_offset_pos) else {
            continue;
        };
        let mut props = RunProps::default();
        if b_offset != 0 {
            let off = b_offset as usize * 2;
            match kind {
                FkpKind::Chpx => {
                    if let Some(&cb) = page.get(off)
                        && let Some(grpprl) = page.get(off + 1..off + 1 + cb as usize)
                    {
                        props.chpx = grpprl.to_vec();
                    }
                }
                FkpKind::Papx => {
                    if let Some(&cb) = page.get(off) {
                        let (start, len) = if cb == 0 {
                            let cb2 = page.get(off + 1).copied().unwrap_or(0) as usize;
                            (off + 2, cb2 * 2)
                        } else {
                            (off + 1, cb as usize * 2 - 1)
                        };
                        if let Some(grpprl) = page.get(start..start + len)
                            && grpprl.len() >= 2
                        {
                            props.istd = u16::from_le_bytes([grpprl[0], grpprl[1]]);
                            apply_pap_sprms(&grpprl[2..], data, &mut props.pap);
                        }
                    }
                }
            }
        }
        runs.push(Run { fc_start, fc_end, props });
    }
}

// ---------------------------------------------------------------------------
// Numbering counters ([MS-DOC] number sequence semantics)

#[derive(Default)]
struct Counters {
    /// Numbering state keyed by list identity (`lsid`): every LFO
    /// referencing the same list continues its sequence.
    state: HashMap<u32, ([u64; LEVELS], [bool; LEVELS])>,
    /// (ilfo, level) pairs whose first-use start-at override has fired.
    overridden: std::collections::HashSet<(u16, usize)>,
}

impl Counters {
    /// Advance the sequence for a paragraph numbered by (`ilfo`, `level`)
    /// and return its effective number plus, when the level's number text is
    /// not reproducible from the marker kind alone, the composite label.
    fn next(&mut self, ilfo: u16, list: &ListDef, level: usize) -> (u64, Option<String>) {
        let level = level.min(LEVELS - 1);
        let def = &list.levels[level];
        let first_use = self.overridden.insert((ilfo, level));
        let (values, started) = self.state.entry(list.lsid).or_default();
        let value = match list.start_override[level] {
            // An LFO start-at override restarts the shared sequence at its
            // value the first time that LFO numbers this level.
            Some(s) if first_use => s,
            _ if started[level] => values[level].saturating_add(1),
            _ => def.start,
        };
        values[level] = value;
        started[level] = true;
        // Using this level restarts deeper levels according to their own
        // restart rules (fNoRestart/ilvlRestartLim).
        for (deeper, deeper_def) in list.levels.iter().enumerate().skip(level + 1) {
            let triggered = match deeper_def.restart {
                None => true,
                Some(lim) => (level as u32) < lim,
            };
            if triggered {
                started[deeper] = false;
            }
        }
        let label = composite_label(list, values, started, level);
        (value, label)
    }
}

/// Render a level's number text against the current sequence values;
/// `None` when it matches the default label the renderer produces anyway.
fn composite_label(
    list: &ListDef,
    values: &[u64; LEVELS],
    started: &[bool; LEVELS],
    level: usize,
) -> Option<String> {
    let def = &list.levels[level];
    let marker = def.marker?;
    crate::shared::numbering::composite_label(
        &def.pattern,
        marker,
        values[level],
        |l| list.levels[l.min(LEVELS - 1)].marker.unwrap_or(MarkerKind::Decimal),
        |l| {
            let l = l.min(LEVELS - 1);
            if started[l] { values[l] } else { list.levels[l].start }
        },
    )
}

// ---------------------------------------------------------------------------
// Assembly: text stream + formatting runs -> model

struct Assembler {
    text: TextStream,
    chpx: Runs,
    papx: Runs,
    stylesheet: Stylesheet,
    lists: Lists,
    prcs: Vec<Vec<u8>>,
    piece_prcs: Vec<Option<usize>>,
    note_refs: HashMap<usize, String>,
    counters: std::cell::RefCell<Counters>,
    /// The Data stream, for `sprmCPicLocation` picture payloads.
    data: Vec<u8>,
    assets: std::cell::RefCell<AssetSink>,
}

/// A paragraph's resolved properties: the style chain's contribution merged
/// with the PAPX and any piece `Prm`.
struct EffectivePap {
    istd: u16,
    effective: PapDelta,
}

struct ParaBuilder {
    inlines: Vec<Inline>,
    fields: Vec<FieldFrame>,
    text: String,
    style: Style,
}

impl ParaBuilder {
    fn new() -> Self {
        ParaBuilder {
            inlines: Vec::new(),
            fields: Vec::new(),
            text: String::new(),
            style: Style::PLAIN,
        }
    }

    fn flush_text(&mut self) {
        if self.text.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.text);
        let inline = Inline::Text { text, style: self.style };
        match self.fields.last_mut() {
            Some(f) if !f.in_result => f.instr.push_str(&inlines_to_plain_text(&[inline])),
            Some(f) => f.inlines.push(inline),
            None => self.inlines.push(inline),
        }
    }

    fn push_char(&mut self, c: char, style: Style) {
        if style != self.style {
            self.flush_text();
            self.style = style;
        }
        self.text.push(c);
    }

    fn push_inline(&mut self, inline: Inline) {
        self.flush_text();
        match self.fields.last_mut() {
            Some(f) if !f.in_result => {}
            Some(f) => f.inlines.push(inline),
            None => self.inlines.push(inline),
        }
    }

    fn field_begin(&mut self) {
        self.flush_text();
        self.fields.push(FieldFrame::default());
    }

    fn field_separate(&mut self) {
        self.flush_text();
        if let Some(f) = self.fields.last_mut() {
            f.in_result = true;
        }
    }

    fn field_end(&mut self) {
        self.flush_text();
        let Some(frame) = self.fields.pop() else {
            return;
        };
        for inline in field_result(&frame.instr, frame.inlines) {
            match self.fields.last_mut() {
                Some(f) if !f.in_result => {}
                Some(f) => f.inlines.push(inline),
                None => self.inlines.push(inline),
            }
        }
    }

    fn finish(mut self) -> Vec<Inline> {
        self.flush_text();
        while !self.fields.is_empty() {
            self.field_end();
        }
        self.inlines
    }
}

impl Assembler {
    fn build_blocks(&self, lo: usize, hi: usize) -> Result<Vec<Block>, ConvertError> {
        let mut blocks: Vec<Block> = Vec::new();
        let mut list_run: Vec<ListEntry> = Vec::new();
        let mut cell_blocks: Vec<Block> = Vec::new();
        let mut row: Vec<Vec<Block>> = Vec::new();
        let mut table_rows: Vec<DocRow> = Vec::new();
        let mut para = ParaBuilder::new();

        let mut i = lo;
        while i < hi.min(self.text.chars.len()) {
            let c = self.text.chars[i];
            let fc = self.text.fcs[i];
            if let Some(id) = self.note_refs.get(&i) {
                para.push_inline(Inline::NoteRef(id.clone()));
                i += 1;
                continue;
            }
            match c {
                '\r' | '\u{7}' | '\u{c}' | '\u{e}' => {
                    let pap = self.effective_pap(fc, i);
                    let inlines = std::mem::replace(&mut para, ParaBuilder::new()).finish();
                    let is_cell_mark = c == '\u{7}';
                    if pap.effective.in_table.unwrap_or(false) || is_cell_mark {
                        // Nested-table content (table depth > 1, inner
                        // cell/row terminators) flattens into the outer
                        // cell as paragraphs.
                        let inner = pap.effective.itap.unwrap_or(1) > 1
                            || pap.effective.inner_cell.unwrap_or(false)
                            || pap.effective.inner_ttp.unwrap_or(false);
                        if inner {
                            if !inlines_are_empty(&inlines) {
                                cell_blocks.push(Block::Paragraph(inlines));
                            }
                        } else if is_cell_mark && pap.effective.ttp.unwrap_or(false) {
                            // Row end: the TTP mark's PAPX carries the TAP
                            // (boundaries, merge flags, header row).
                            if !row.is_empty() {
                                table_rows.push(DocRow {
                                    cells: std::mem::take(&mut row),
                                    tap: pap.effective.tap.clone(),
                                });
                            }
                        } else if is_cell_mark {
                            if !inlines_are_empty(&inlines) {
                                cell_blocks.push(Block::Paragraph(inlines));
                            }
                            row.push(std::mem::take(&mut cell_blocks));
                        } else if !inlines_are_empty(&inlines) {
                            cell_blocks.push(Block::Paragraph(inlines));
                        }
                    } else {
                        Self::flush_table(
                            &mut blocks,
                            &mut table_rows,
                            &mut row,
                            &mut cell_blocks,
                        )?;
                        self.emit_paragraph(&pap, inlines, &mut blocks, &mut list_run);
                    }
                }
                '\u{b}' => para.push_inline(Inline::LineBreak),
                '\u{13}' => para.field_begin(),
                '\u{14}' => para.field_separate(),
                '\u{15}' => para.field_end(),
                '\t' => {
                    let style = self.char_style(fc, i);
                    para.push_char(' ', style);
                }
                '\u{1e}' => {
                    let style = self.char_style(fc, i);
                    para.push_char('-', style);
                }
                // Inline picture special character: extract the payload
                // pointed at by sprmCPicLocation.
                '\u{1}' => {
                    if let Some(image) = self.picture_at(fc)? {
                        para.push_inline(image);
                    }
                }
                '\u{2}' | '\u{5}' | '\u{8}' | '\u{1f}' => {}
                c if c.is_control() => {}
                c => {
                    let style = self.char_style(fc, i);
                    para.push_char(c, style);
                }
            }
            i += 1;
        }
        let inlines = para.finish();
        if !inlines_are_empty(&inlines) {
            blocks.push(Block::Paragraph(inlines));
        }
        Self::flush_table(&mut blocks, &mut table_rows, &mut row, &mut cell_blocks)?;
        flush_list(&mut blocks, &mut list_run);
        Ok(blocks)
    }

    /// Effective character style in specification order: paragraph/character
    /// style chain -> CHPX (toggles vs the style base) -> piece Prm.
    fn char_style(&self, fc: u32, char_index: usize) -> Style {
        let para_istd = self.papx.lookup(fc).map(|p| p.istd).unwrap_or(0);
        let chpx = self.chpx.lookup(fc).map(|p| p.chpx.as_slice()).unwrap_or(&[]);
        let istd = chpx_istd(chpx).unwrap_or(para_istd);
        let base = self.stylesheet.get(istd).chp;
        let mut style = apply_chpx(chpx, base, base);
        if let Some(&piece_idx) = self.text.piece_of.get(char_index)
            && let Some(piece_prc) = self.piece_prm(piece_idx as usize)
        {
            style = apply_chpx(piece_prc, style, base);
        }
        style
    }

    fn piece_prm(&self, piece_idx: usize) -> Option<&[u8]> {
        let prc_idx = (*self.piece_prcs.get(piece_idx)?)?;
        self.prcs.get(prc_idx).map(Vec::as_slice)
    }

    /// Paragraph properties in specification order: style chain -> PAPX ->
    /// piece Prm.
    fn effective_pap(&self, fc: u32, char_index: usize) -> EffectivePap {
        let (istd, papx_delta) = match self.papx.lookup(fc) {
            Some(p) => (p.istd, p.pap.clone()),
            None => (0, PapDelta::default()),
        };
        let style = self.stylesheet.get(istd);
        let mut effective = style.pap.clone().merge(papx_delta);
        if let Some(&piece_idx) = self.text.piece_of.get(char_index)
            && let Some(prm) = self.piece_prm(piece_idx as usize)
        {
            let mut prm_delta = PapDelta::default();
            apply_pap_sprms(prm, &[], &mut prm_delta);
            effective = effective.merge(prm_delta);
        }
        EffectivePap { istd, effective }
    }

    fn emit_paragraph(
        &self,
        pap: &EffectivePap,
        inlines: Vec<Inline>,
        blocks: &mut Vec<Block>,
        list_run: &mut Vec<ListEntry>,
    ) {
        if inlines_are_empty(&inlines) {
            flush_list(blocks, list_run);
            return;
        }
        let style = self.stylesheet.get(pap.istd);
        let heading = style.heading.or(pap.effective.outline.flatten());
        if let Some(level) = heading {
            flush_list(blocks, list_run);
            let mut content = inlines;
            rebase_emphasis(&mut content, style.chp);
            // A numbered heading advances its sequence and keeps its number
            // visible - headings have no native numbering in the output.
            if let Some(label) = self.heading_label(pap) {
                content.insert(0, Inline::Text { text: label, style: Style::PLAIN });
            }
            blocks.push(Block::Heading { level, anchor: None, content });
            return;
        }
        // ilfo 0xF801 marks a paragraph whose list numbering is suppressed.
        let ilfo = pap.effective.ilfo.unwrap_or(0);
        if ilfo != 0 && ilfo != 0xF801 {
            let ilvl = pap.effective.ilvl.unwrap_or(0) as usize;
            let fallback;
            let list = match self.lists.get(ilfo) {
                Some(list) => list,
                None => {
                    fallback = ListDef::unknown(ilfo);
                    &fallback
                }
            };
            let def = &list.levels[ilvl.min(LEVELS - 1)];
            if let Some(marker) = def.marker {
                let (number, label) = if marker.ordered() {
                    self.counters.borrow_mut().next(ilfo, list, ilvl)
                } else {
                    (0, None)
                };
                list_run.push(ListEntry {
                    level: ilvl,
                    key: ListKey { instance: list.lsid as u64, marker },
                    number,
                    label,
                    blocks: vec![Block::Paragraph(inlines)],
                });
                return;
            }
            // Marker "none": numbering suppressed, plain paragraph.
        }
        flush_list(blocks, list_run);
        blocks.push(Block::Paragraph(inlines));
    }

    /// Extract the inline picture whose PICF + OfficeArt data the character's
    /// CHPX points at (`sprmCPicLocation`), retaining it as an asset.
    /// Unsupported or unreachable payloads degrade with a log.
    fn picture_at(&self, fc: u32) -> Result<Option<Inline>, ConvertError> {
        let Some(offset) = self.chpx.lookup(fc).and_then(|p| sprm::chpx_pic_location(&p.chpx))
        else {
            return Ok(None);
        };
        let offset = offset as usize;
        let Some(lcb) = get_u32(&self.data, offset).map(|v| v as usize) else {
            return Ok(None);
        };
        let Some(cb_header) = get_u16(&self.data, offset + 4).map(|v| v as usize) else {
            return Ok(None);
        };
        let Some(picf) = self.data.get(offset..offset.saturating_add(lcb)) else {
            log::debug!("picture data out of bounds in the Data stream");
            return Ok(None);
        };
        let art = &picf[cb_header.min(picf.len())..];
        let Some(blip) =
            crate::shared::officeart::first_blip(art, limits::MAX_ENTRY_BYTES as usize)
        else {
            log::debug!("skipping picture with no supported blip payload");
            return Ok(None);
        };
        let part = format!("data/pic{offset}.{}", blip.extension);
        let id = self.assets.borrow_mut().add(blip.media_type.to_string(), part, &blip.bytes)?;
        Ok(Some(Inline::Image { alt: String::new(), source: ImageSource::Asset(id) }))
    }

    /// The visible number label of a numbered heading paragraph, with its
    /// trailing separator; advances the list sequence.
    fn heading_label(&self, pap: &EffectivePap) -> Option<String> {
        let ilfo = pap.effective.ilfo.unwrap_or(0);
        if ilfo == 0 || ilfo == 0xF801 {
            return None;
        }
        let ilvl = pap.effective.ilvl.unwrap_or(0) as usize;
        let list = self.lists.get(ilfo)?;
        let def = &list.levels[ilvl.min(LEVELS - 1)];
        let marker = def.marker?;
        if !marker.ordered() {
            return None;
        }
        let (number, label) = self.counters.borrow_mut().next(ilfo, list, ilvl);
        Some(format!("{} ", label.unwrap_or_else(|| marker.label(number))))
    }

    fn flush_table(
        blocks: &mut Vec<Block>,
        table_rows: &mut Vec<DocRow>,
        row: &mut Vec<Vec<Block>>,
        cell_blocks: &mut Vec<Block>,
    ) -> Result<(), ConvertError> {
        // (associated fn, no self)
        if !cell_blocks.is_empty() {
            row.push(std::mem::take(cell_blocks));
        }
        if !row.is_empty() {
            table_rows.push(DocRow { cells: std::mem::take(row), tap: None });
        }
        if table_rows.is_empty() {
            return Ok(());
        }
        let rows = std::mem::take(table_rows).into_iter().map(DocRow::into_grid_row).collect();
        if let Some(block) = build_edge_table(rows)? {
            blocks.push(block);
        }
        Ok(())
    }
}

/// One accumulated table row: cell contents plus the TAP from the row-end
/// mark (absent for rows without `sprmTDefTable`).
struct DocRow {
    cells: Vec<Vec<Block>>,
    tap: Option<Tap>,
}

impl DocRow {
    fn into_grid_row(self) -> GridRow {
        let header = self.tap.as_ref().is_some_and(|t| t.header);
        let tap = self.tap.unwrap_or_default();
        let cells = self
            .cells
            .into_iter()
            .enumerate()
            .map(|(k, blocks)| {
                let tc = tap.cells.get(k).copied().unwrap_or_default();
                // Without a TAP boundary, a synthetic index-based edge keeps
                // the flat column behavior.
                let right =
                    tap.boundaries.get(k + 1).map(|&b| b as i64).unwrap_or((k as i64 + 1) * 1000);
                let prop = CellProp {
                    merge_first: tc.horz_first,
                    merge_cont: tc.horz_cont,
                    vmerge_first: tc.vert_restart,
                    vmerge_cont: tc.vert_cont,
                    right,
                };
                (blocks, prop)
            })
            .collect();
        GridRow { cells, header }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::numbering::NumberText;
    use lists::LevelDef;

    fn list(lsid: u32) -> ListDef {
        let mut levels: [LevelDef; LEVELS] = std::array::from_fn(|_| LevelDef::default());
        levels[0].marker = Some(MarkerKind::Decimal);
        levels[1].marker = Some(MarkerKind::LowerAlpha);
        ListDef { lsid, levels, start_override: [None; LEVELS] }
    }

    #[test]
    fn shared_lsid_continues_across_ilfos() {
        let mut c = Counters::default();
        let l = list(7);
        assert_eq!(c.next(1, &l, 0).0, 1);
        assert_eq!(c.next(2, &l, 0).0, 2);
    }

    #[test]
    fn first_use_override_restarts_the_shared_sequence() {
        let mut c = Counters::default();
        let l = list(7);
        let mut over = list(7);
        over.start_override[0] = Some(10);
        assert_eq!(c.next(1, &l, 0).0, 1);
        assert_eq!(c.next(1, &l, 0).0, 2);
        assert_eq!(c.next(2, &over, 0).0, 10); // fires on first use only
        assert_eq!(c.next(2, &over, 0).0, 11);
        assert_eq!(c.next(1, &l, 0).0, 12); // same lsid: sequence is shared
    }

    #[test]
    fn no_restart_level_survives_shallower_items() {
        let mut c = Counters::default();
        let mut l = list(7);
        l.levels[1].restart = Some(0); // fNoRestart, never restart
        assert_eq!(c.next(1, &l, 0).0, 1);
        assert_eq!(c.next(1, &l, 1).0, 1);
        assert_eq!(c.next(1, &l, 0).0, 2);
        assert_eq!(c.next(1, &l, 1).0, 2); // continues past the level-0 item
        let restarting = list(8);
        assert_eq!(c.next(3, &restarting, 0).0, 1);
        assert_eq!(c.next(3, &restarting, 1).0, 1);
        assert_eq!(c.next(3, &restarting, 0).0, 2);
        assert_eq!(c.next(3, &restarting, 1).0, 1); // default rule restarts
    }

    #[test]
    fn restart_limit_distinguishes_trigger_levels() {
        let mut c = Counters::default();
        let mut l = list(7);
        l.levels[2].marker = Some(MarkerKind::Decimal);
        l.levels[2].restart = Some(2); // restart only after levels 0 or 1
        assert_eq!(c.next(1, &l, 2).0, 1);
        c.next(1, &l, 1); // level 1 < 2: triggers a restart
        assert_eq!(c.next(1, &l, 2).0, 1);
        assert_eq!(c.next(1, &l, 2).0, 2);
    }

    #[test]
    fn composite_label_renders_parent_numbers() {
        let mut c = Counters::default();
        let mut l = list(7);
        l.levels[1].pattern.text = vec![
            NumberText::Level(0),
            NumberText::Literal("-".into()),
            NumberText::Level(1),
            NumberText::Literal(")".into()),
        ];
        c.next(1, &l, 0);
        let (n, label) = c.next(1, &l, 1);
        assert_eq!(n, 1);
        assert_eq!(label.as_deref(), Some("1-a)"));
    }

    #[test]
    fn default_number_text_produces_no_label() {
        let mut c = Counters::default();
        let mut l = list(7);
        l.levels[0].pattern.text = vec![NumberText::Level(0), NumberText::Literal(".".into())];
        assert_eq!(c.next(1, &l, 0), (1, None));
    }

    #[test]
    fn prm0_decodes_modeled_sprms() {
        // isprm 0x55 = sprmCFBold (0x0835), val 0x01, fComplex clear.
        let prm = (0x55u16 << 1) | (0x01 << 8);
        assert_eq!(prm0_grpprl(prm), Some(vec![0x35, 0x08, 0x01]));
        // isprm 0x18 = sprmPFInTable (0x2416).
        let prm = (0x18u16 << 1) | (0x01 << 8);
        assert_eq!(prm0_grpprl(prm), Some(vec![0x16, 0x24, 0x01]));
        // isprm 0x05 (sprmPJc) is outside the converted model.
        assert_eq!(prm0_grpprl(0x05 << 1), None);
    }

    #[test]
    fn tdef_table_parses_boundaries_and_merge_flags() {
        // sprmTDefTable operand: cb, 2 columns, 3 boundaries, 2 TC80s where
        // the second cell continues a vertical merge.
        let mut operand: Vec<u8> = Vec::new();
        operand.extend_from_slice(&0u16.to_le_bytes()); // cb (unused here)
        operand.push(2); // columns
        for b in [0i16, 4000, 8000] {
            operand.extend_from_slice(&b.to_le_bytes());
        }
        // TC80: tcgrf + wWidth + 4 borders.
        let tc80 = |tcgrf: u16| {
            let mut v = tcgrf.to_le_bytes().to_vec();
            v.extend_from_slice(&[0; 18]);
            v
        };
        operand.extend_from_slice(&tc80(0x3 << 5)); // vertMerge = fvmRestart
        operand.extend_from_slice(&tc80(0x1 << 5)); // vertMerge = fvmMerge
        let mut delta = PapDelta::default();
        let mut grpprl = vec![0x08, 0xD6]; // sprmTDefTable
        grpprl.extend_from_slice(&((operand.len() - 1) as u16).to_le_bytes());
        grpprl.extend_from_slice(&operand[2..]);
        apply_pap_sprms(&grpprl, &[], &mut delta);
        let tap = delta.tap.expect("TAP parsed");
        assert_eq!(tap.boundaries, vec![0, 4000, 8000]);
        assert!(tap.cells[0].vert_restart && !tap.cells[0].vert_cont);
        assert!(tap.cells[1].vert_cont && !tap.cells[1].vert_restart);
    }
}
