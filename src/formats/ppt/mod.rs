//! Legacy PowerPoint 97-2003 binary (.ppt): OLE2 container, record stream.
//! Slides resolve through the persist directory (the only default path);
//! text comes from TextHeaderAtom + TextCharsAtom/TextBytesAtom with
//! StyleTextPropAtom runs and TxMasterStyleAtom defaults applied. Raw
//! stream-order scanning exists only as an explicitly labelled recovery for
//! files whose persist directory is unusable. Speaker notes are included
//! (fixed policy), rendered as a quote after their slide.

mod styletext;

use crate::error::ConvertError;
use crate::model::{Block, Cell, Document, GridBuilder, Inline, Style, TableKind, inlines_are_empty};
use crate::package::limits;
use crate::shared::binary::{get_u32, read_ole_stream, utf16le_units};
use crate::shared::delta::{StyleDelta, rebase_emphasis};
use crate::shared::header::resolve_header_rows;
use crate::shared::list::{ListEntry, ListKey, MarkerKind, flush_list};
use crate::shared::officeart::record_at;
use crate::shared::text::clean_text;
use std::collections::HashMap;
use std::io::Cursor;
use styletext::{CharProps, MasterLevel, StyleRuns};

/// One master's per-text-type level defaults, keyed by TxMasterStyleAtom
/// instance (the text type).
type MasterStyles = HashMap<u16, Vec<MasterLevel>>;

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let cursor = Cursor::new(bytes);
    let mut ole = cfb::CompoundFile::open(cursor)
        .map_err(|e| ConvertError::malformed(format!("not an OLE2 compound file: {e}")))?;
    let data = read_ole_stream(&mut ole, "PowerPoint Document")?;
    let current_user = read_ole_stream(&mut ole, "Current User").unwrap_or_default();
    if get_u32(&current_user, 12) == Some(0xF3D1_C4DF) {
        return Err(ConvertError::Encrypted);
    }

    let mut ex = Extractor::default();
    if !ex.parse_slides(&data, &current_user)? {
        // Labelled recovery path: the persist directory is unusable, so text
        // is taken in raw stream order (may include superseded edits).
        log::warn!("ppt persist directory unusable; recovering text in raw stream order");
        ex = Extractor::default();
        ex.recovering = true;
        ex.walk(&data)?;
        ex.end_segment(None);
    }
    if ex.encrypted {
        return Err(ConvertError::Encrypted);
    }
    let assets = collect_pictures(&mut ole)?;
    Ok(Document { blocks: ex.into_blocks(), notes: Vec::new(), assets })
}

/// Retain the deck's embedded pictures from the `Pictures` stream (OfficeArt
/// BStore file blocks). Pictures are document-level assets; per-slide
/// placement is not resolved. Unsupported formats degrade with a log.
fn collect_pictures<R: std::io::Read + std::io::Seek>(
    ole: &mut cfb::CompoundFile<R>,
) -> Result<Vec<crate::model::Asset>, ConvertError> {
    use crate::shared::officeart;
    let Ok(pictures) = read_ole_stream(ole, "Pictures") else {
        return Ok(Vec::new());
    };
    let mut sink = crate::shared::assets::AssetSink::new();
    let mut pos = 0usize;
    let mut index = 0u32;
    while let Some((ver_inst, rec_type, body)) = officeart::record_at(&pictures, pos) {
        pos += 8 + body.len();
        index += 1;
        if index > 100_000 {
            break;
        }
        let cap = limits::MAX_ENTRY_BYTES as usize;
        let blip = match rec_type {
            0xF007 => officeart::fbse_blip(body, cap),
            _ => officeart::decode_blip(ver_inst, rec_type, body, cap),
        };
        match blip {
            Some(blip) => {
                sink.add(
                    blip.media_type.to_string(),
                    format!("pictures/{index}.{}", blip.extension),
                    &blip.bytes,
                )?;
            }
            None => log::debug!("skipping unsupported Pictures record 0x{rec_type:04X}"),
        }
    }
    Ok(sink.assets)
}

/// Iterate the records laid out back to back in `data`.
fn children(data: &[u8]) -> impl Iterator<Item = (u16, u16, &[u8])> {
    let mut pos = 0;
    std::iter::from_fn(move || {
        let (ver_inst, rec_type, body) = record_at(data, pos)?;
        pos += 8 + body.len();
        Some((ver_inst, rec_type, body))
    })
}

/// A text shape being accumulated: header type, text, then styling.
struct PendingShape {
    tx_type: u8,
    text: String,
    styles: Option<StyleRuns>,
}

#[derive(Default)]
struct Extractor {
    /// Finished segments: (blocks, pairing id, is_notes).
    segments: Vec<(Vec<Block>, Option<u32>, bool)>,
    current: Vec<Block>,
    current_is_notes: bool,
    list_run: Vec<ListEntry>,
    pending: Option<PendingShape>,
    /// Master style tables in master-list order: (masterId, styles).
    masters: Vec<(u32, MasterStyles)>,
    /// Index into `masters` for the slide being extracted (0 fallback).
    active_master: usize,
    shape_counter: u64,
    encrypted: bool,
    /// Raw-stream recovery: no persist lists, so notes descend inline.
    recovering: bool,
    /// Records visited across the whole extraction, capped.
    records: u64,
}

/// The persist-resolved layout of the presentation: slide/notes lists from
/// the current DocumentContainer, persist id -> offset for every container.
struct DocLayout<'a> {
    persist: HashMap<u32, usize>,
    slide_list: &'a [u8],
    notes_list: Option<&'a [u8]>,
    master_list: Option<&'a [u8]>,
}

/// Resolve the UserEditAtom chain into the persist directory and find the
/// DocumentContainer's SlideListWithText instances. `None` means the persist
/// directory is unusable and the caller falls back to raw-order recovery.
fn locate_document<'a>(data: &'a [u8], current_user: &[u8]) -> Option<DocLayout<'a>> {
    let mut persist: HashMap<u32, usize> = HashMap::new();
    let mut doc_persist: Option<u32> = None;
    let mut edit_off = get_u32(current_user, 16)? as usize;
    for _ in 0..100 {
        if edit_off == 0 {
            break;
        }
        let (_, rec_type, body) = record_at(data, edit_off)?;
        if rec_type != 0x0FF5 {
            return None;
        }
        if doc_persist.is_none() {
            doc_persist = get_u32(body, 16);
        }
        let dir_off = get_u32(body, 12)? as usize;
        if let Some((_, 0x1772, dir)) = record_at(data, dir_off) {
            let mut pos = 0;
            while pos + 4 <= dir.len() {
                let head = get_u32(dir, pos)?;
                let id = head & 0xF_FFFF;
                let count = (head >> 20) as usize;
                pos += 4;
                for k in 0..count {
                    // Newer edits win: keep the first offset seen.
                    persist.entry(id + k as u32).or_insert(get_u32(dir, pos)? as usize);
                    pos += 4;
                }
            }
        }
        let prev = get_u32(body, 8)? as usize;
        if prev == edit_off {
            break;
        }
        edit_off = prev;
    }

    let doc_off = *persist.get(&doc_persist?)?;
    let Some((_, 0x03E8, doc)) = record_at(data, doc_off) else {
        return None;
    };
    let (.., slide_list) =
        children(doc).find(|&(ver_inst, rec_type, _)| rec_type == 0x0FF0 && ver_inst >> 4 == 0)?;
    let notes_list = children(doc)
        .find(|&(ver_inst, rec_type, _)| rec_type == 0x0FF0 && ver_inst >> 4 == 2)
        .map(|(.., body)| body);
    let master_list = children(doc)
        .find(|&(ver_inst, rec_type, _)| rec_type == 0x0FF0 && ver_inst >> 4 == 1)
        .map(|(.., body)| body);
    Some(DocLayout { persist, slide_list, notes_list, master_list })
}

/// One master's TxMasterStyleAtoms, keyed by text-type instance.
fn master_styles(master: &[u8]) -> MasterStyles {
    let mut styles = MasterStyles::new();
    for (ver_inst, rec_type, body) in children(master) {
        if rec_type == 0x0FA3 {
            let instance = ver_inst >> 4;
            styles.entry(instance).or_insert_with(|| styletext::parse_master_style(body, instance));
        }
    }
    styles
}

/// Masters in master-list order (MasterPersistAtoms: persistIdRef at 0,
/// masterId at 12); falls back to a persist-directory scan when the list is
/// absent so single-master decks still get their defaults.
fn collect_masters(
    master_list: Option<&[u8]>,
    persist: &HashMap<u32, usize>,
    data: &[u8],
) -> Vec<(u32, MasterStyles)> {
    let mut out = Vec::new();
    if let Some(list) = master_list {
        for (_, rec_type, body) in children(list) {
            if rec_type != 0x03F3 {
                continue;
            }
            let (Some(persist_ref), Some(master_id)) = (get_u32(body, 0), get_u32(body, 12)) else {
                continue;
            };
            if let Some(&off) = persist.get(&persist_ref)
                && let Some((_, 0x03F8, master)) = record_at(data, off)
            {
                out.push((master_id, master_styles(master)));
            }
        }
    }
    if out.is_empty() {
        let mut offs: Vec<usize> = persist.values().copied().collect();
        offs.sort_unstable();
        for off in offs {
            if let Some((_, 0x03F8, master)) = record_at(data, off) {
                out.push((0, master_styles(master)));
            }
        }
    }
    out
}

impl Extractor {
    /// Walk slides in presentation order: the UserEditAtom chain yields the
    /// persist directory, the DocumentContainer's SlideListWithText yields
    /// slide order and outline text, each slide container its own textboxes.
    /// `Ok(false)` means the persist directory was unusable.
    fn parse_slides(&mut self, data: &[u8], current_user: &[u8]) -> Result<bool, ConvertError> {
        let Some(layout) = locate_document(data, current_user) else {
            return Ok(false);
        };
        self.masters = collect_masters(layout.master_list, &layout.persist, data);
        self.walk_slide_list(layout.slide_list, &layout.persist, data, false, 0x03EE)?;
        if let Some(notes_list) = layout.notes_list {
            self.walk_slide_list(notes_list, &layout.persist, data, true, 0x03F0)?;
        }
        Ok(true)
    }

    fn walk_slide_list(
        &mut self,
        list: &[u8],
        persist: &HashMap<u32, usize>,
        data: &[u8],
        is_notes: bool,
        container_type: u16,
    ) -> Result<(), ConvertError> {
        // (persistIdRef, slideId) of the page whose container is pending.
        let mut pending: Option<(u32, u32)> = None;
        for (ver_inst, rec_type, body) in children(list) {
            match rec_type {
                // SlidePersistAtom: the next slide/notes page begins.
                0x03F3 => {
                    let id =
                        self.finish_slide(pending.take(), persist, data, container_type, is_notes)?;
                    self.end_segment(id);
                    self.current_is_notes = is_notes;
                    pending = get_u32(body, 0).map(|p| (p, get_u32(body, 12).unwrap_or(0)));
                    if !is_notes {
                        self.select_master(pending.map(|(p, _)| p), persist, data);
                    }
                }
                _ => self.record(ver_inst, rec_type, body)?,
            }
        }
        let id = self.finish_slide(pending, persist, data, container_type, is_notes)?;
        self.end_segment(id);
        Ok(())
    }

    /// Emit a slide's own textboxes after its outline text. Returns the
    /// segment's pairing id: the slideId for slides, or the owning slide's
    /// id (NotesAtom slideIdRef) for notes pages.
    fn finish_slide(
        &mut self,
        pending: Option<(u32, u32)>,
        persist: &HashMap<u32, usize>,
        data: &[u8],
        container_type: u16,
        is_notes: bool,
    ) -> Result<Option<u32>, ConvertError> {
        let Some((persist_ref, slide_id)) = pending else {
            return Ok(None);
        };
        let mut id = if is_notes { None } else { (slide_id != 0).then_some(slide_id) };
        if let Some(off) = persist.get(&persist_ref)
            && let Some((_, t, body)) = record_at(data, *off)
            && t == container_type
        {
            if is_notes {
                // NotesAtom.slideIdRef names the owning slide (0 = none).
                id = children(body)
                    .find(|&(_, t, _)| t == 0x03F1)
                    .and_then(|(.., atom)| get_u32(atom, 0))
                    .filter(|&v| v != 0);
            }
            self.walk(body)?;
        }
        Ok(id)
    }

    fn end_segment(&mut self, id: Option<u32>) {
        self.flush_shape();
        flush_list(&mut self.current, &mut self.list_run);
        if !self.current.is_empty() {
            let blocks = std::mem::take(&mut self.current);
            self.segments.push((blocks, id, self.current_is_notes));
        }
    }

    fn into_blocks(mut self) -> Vec<Block> {
        self.end_segment(None);
        let mut slides: Vec<(Option<u32>, Vec<Block>)> = Vec::new();
        let mut notes: Vec<(Option<u32>, Vec<Block>)> = Vec::new();
        for (blocks, id, is_notes) in self.segments {
            if is_notes {
                notes.push((id, blocks));
            } else {
                slides.push((id, blocks));
            }
        }
        // Notes pages pair to slides by their stored slide id, not by list
        // position: the notes list may be sparse (notes on only some
        // slides), which order-based zipping would misattribute.
        let mut used = vec![false; notes.len()];
        let mut out = Vec::new();
        for (sid, blocks) in slides {
            out.extend(blocks);
            for (i, (nid, nblocks)) in notes.iter_mut().enumerate() {
                if !used[i] && sid.is_some() && *nid == sid {
                    used[i] = true;
                    out.push(Block::BlockQuote(std::mem::take(nblocks)));
                }
            }
        }
        // Notes without a resolvable owner keep document order at the end.
        for (used, (_, nblocks)) in used.into_iter().zip(notes) {
            if !used && !nblocks.is_empty() {
                out.push(Block::BlockQuote(nblocks));
            }
        }
        out
    }

    /// Pick the master the slide references (SlideAtom.masterIdRef at
    /// offset 12); the first listed master is the deterministic fallback.
    fn select_master(&mut self, slide: Option<u32>, persist: &HashMap<u32, usize>, data: &[u8]) {
        self.active_master = slide
            .and_then(|id| persist.get(&id))
            .and_then(|&off| record_at(data, off))
            .filter(|&(_, t, _)| t == 0x03EE)
            .and_then(|(_, _, body)| children(body).find(|&(_, rec_type, _)| rec_type == 0x03EF))
            .and_then(|(.., atom)| get_u32(atom, 12))
            .and_then(|mid| self.masters.iter().position(|&(id, _)| id == mid))
            .unwrap_or(0);
    }

    /// Iterative container walk over an explicit stack with fixed depth and
    /// record-count bounds — nesting or record counts beyond any real
    /// presentation are attack shapes and hard-fail.
    fn walk(&mut self, data: &[u8]) -> Result<(), ConvertError> {
        let mut stack: Vec<(&[u8], usize)> = vec![(data, 0)];
        while let Some((buf, pos)) = stack.last_mut() {
            let Some((ver_inst, rec_type, body)) = record_at(buf, *pos) else {
                stack.pop();
                continue;
            };
            *pos += 8 + body.len();
            self.charge_record()?;
            if ver_inst & 0xF != 0xF {
                self.atom(rec_type, body);
                continue;
            }
            match rec_type {
                // CryptSession10Container: the stream is encrypted.
                0x2F14 => self.encrypted = true,
                // Notes containers are walked via the notes list; recovery
                // has no lists, so their text is taken inline (as notes).
                // A NotesAtom slideIdRef in the masters range (high bit
                // set) marks the notes master: template chrome, excluded.
                0x03F0 if self.recovering => {
                    let master = children(body)
                        .find(|&(_, t, _)| t == 0x03F1)
                        .and_then(|(.., atom)| get_u32(atom, 0))
                        .is_some_and(|id| id & 0x8000_0000 != 0);
                    if !master {
                        self.end_segment(None);
                        self.current_is_notes = true;
                        self.walk(body)?;
                        self.end_segment(None);
                        self.current_is_notes = false;
                    }
                }
                // Notes, master, and handout containers are walked via their
                // own lists, not inline.
                0x03F0 | 0x03F8 | 0x0FC9 => {}
                // Only instance 0 of SlideListWithText holds slide text here.
                0x0FF0 if ver_inst >> 4 != 0 => {}
                // Fork (sznb): a table is a group shape flagged by the
                // tableProperties property; rebuild its grid from the cell
                // rectangles instead of emitting every cell as a paragraph.
                0xF003 if is_table_group(body) => {
                    if let Some(table) = self.table_from_group(body)? {
                        self.current.push(table);
                    }
                }
                _ => {
                    if stack.len() >= limits::MAX_RECORD_DEPTH {
                        return Err(ConvertError::ResourceLimit {
                            limit: "max_record_depth",
                            detail: format!("record nesting exceeds {}", limits::MAX_RECORD_DEPTH),
                        });
                    }
                    stack.push((body, 0));
                }
            }
        }
        Ok(())
    }

    /// One record outside `walk` (slide-list traversal): containers descend
    /// through the bounded walk, atoms extract directly.
    fn record(&mut self, ver_inst: u16, rec_type: u16, body: &[u8]) -> Result<(), ConvertError> {
        self.charge_record()?;
        if ver_inst & 0xF == 0xF {
            match rec_type {
                0x2F14 => self.encrypted = true,
                0x03F0 | 0x03F8 | 0x0FC9 => {}
                0x0FF0 if ver_inst >> 4 != 0 => {}
                _ => self.walk(body)?,
            }
        } else {
            self.atom(rec_type, body);
        }
        Ok(())
    }

    /// Fork (sznb): text of one shape container as blocks of its own, with
    /// the slide-level text state set aside so nothing leaks across.
    fn capture(&mut self, body: &[u8]) -> Result<Vec<Block>, ConvertError> {
        self.flush_shape();
        flush_list(&mut self.current, &mut self.list_run);
        let saved_current = std::mem::take(&mut self.current);
        let saved_list = std::mem::take(&mut self.list_run);
        let walked = self.walk(body);
        self.flush_shape();
        flush_list(&mut self.current, &mut self.list_run);
        let blocks = std::mem::replace(&mut self.current, saved_current);
        self.list_run = saved_list;
        walked?;
        Ok(blocks)
    }

    /// Fork (sznb): rebuild a table group. PowerPoint 97-2003 stores a table
    /// as a group of cell rectangles plus border lines; column / row
    /// boundaries come from the cell anchors, and a rectangle crossing
    /// boundaries is a merged cell with the matching spans. Without anchored
    /// cells the group's text is emitted in order and `None` returned.
    fn table_from_group(&mut self, group: &[u8]) -> Result<Option<Block>, ConvertError> {
        let mut cells: Vec<(Rect, &[u8])> = Vec::new();
        let mut loose: Vec<&[u8]> = Vec::new();
        // The first SpContainer is the group shape itself.
        for (_, rec_type, body) in children(group).skip(1) {
            self.charge_record()?;
            if rec_type != 0xF004 {
                loose.push(body);
                continue;
            }
            let shape_type =
                children(body).find(|&(_, t, _)| t == 0xF00A).map(|(vi, ..)| vi >> 4).unwrap_or(0);
            // msosptLine: cell borders.
            if shape_type == 20 {
                continue;
            }
            match anchor_of(body) {
                Some(r) if r.right > r.left && r.bottom > r.top => cells.push((r, body)),
                _ => loose.push(body),
            }
        }
        let xs = boundaries(cells.iter().flat_map(|(r, _)| [r.left, r.right]));
        let ys = boundaries(cells.iter().flat_map(|(r, _)| [r.top, r.bottom]));
        let (ncols, nrows) = (xs.len().saturating_sub(1), ys.len().saturating_sub(1));
        if cells.is_empty()
            || ncols == 0
            || nrows == 0
            || (ncols as u64) * (nrows as u64) > limits::MAX_EXPANSION
        {
            for (_, body) in cells {
                loose.push(body);
            }
            for body in loose {
                let blocks = self.capture(body)?;
                self.current.extend(blocks);
            }
            return Ok(None);
        }
        // Claim each cell's grid area; an origin landing on an owned position
        // (overlapping rectangles) is dropped rather than double-placed.
        let mut owned = vec![vec![false; ncols]; nrows];
        let mut placed: Vec<((usize, usize), u32, u32, &[u8])> = Vec::new();
        for (rect, body) in &cells {
            let (c0, c1) = (snap(rect.left, &xs), snap(rect.right, &xs));
            let (r0, r1) = (snap(rect.top, &ys), snap(rect.bottom, &ys));
            if c1 <= c0 || r1 <= r0 || owned[r0][c0] {
                continue;
            }
            for row in owned.iter_mut().take(r1).skip(r0) {
                for slot in row.iter_mut().take(c1).skip(c0) {
                    *slot = true;
                }
            }
            placed.push(((r0, c0), (c1 - c0) as u32, (r1 - r0) as u32, *body));
        }
        let mut origins: HashMap<(usize, usize), Cell> = HashMap::new();
        for (pos, col_span, row_span, body) in placed {
            let blocks = self.capture(body)?;
            origins.insert(pos, Cell::spanning(blocks, col_span, row_span));
        }
        let mut builder = GridBuilder::new();
        // Authored slide tables keep blank tail rows (as Word sources do).
        builder.keep_empty_rows();
        for (r, row) in owned.iter().enumerate() {
            builder.next_row();
            for (c, &is_owned) in row.iter().enumerate() {
                if let Some(cell) = origins.remove(&(r, c)) {
                    builder.place(cell)?;
                } else if !is_owned {
                    builder.place(Cell::default())?;
                }
                // Positions a span covers materialize inside the builder.
            }
        }
        let mut table = builder.finish(TableKind::Data);
        if table.grid.is_empty() {
            return Ok(None);
        }
        table.header_rows = resolve_header_rows(&table, 0);
        Ok(Some(Block::Table(table)))
    }

    fn charge_record(&mut self) -> Result<(), ConvertError> {
        self.records += 1;
        if self.records > limits::MAX_RECORDS {
            return Err(ConvertError::ResourceLimit {
                limit: "max_records",
                detail: format!("record stream exceeds {} records", limits::MAX_RECORDS),
            });
        }
        Ok(())
    }

    fn atom(&mut self, rec_type: u16, body: &[u8]) {
        match rec_type {
            // TextHeaderAtom: a new text shape begins.
            0x0F9F => {
                self.flush_shape();
                self.pending = Some(PendingShape {
                    tx_type: body.first().copied().unwrap_or(1),
                    text: String::new(),
                    styles: None,
                });
            }
            // TextCharsAtom: UTF-16LE.
            0x0FA0 => {
                let units = utf16le_units(body);
                let text: String =
                    char::decode_utf16(units).map(|r| r.unwrap_or('\u{fffd}')).collect();
                self.push_text(text);
            }
            // TextBytesAtom: low bytes of UTF-16 code units.
            0x0FA8 => {
                let text: String = body.iter().map(|&b| b as char).collect();
                self.push_text(text);
            }
            // StyleTextPropAtom: styling for the pending shape's text.
            0x0FA1 => {
                if let Some(pending) = &mut self.pending {
                    let len = pending.text.chars().map(char::len_utf16).sum();
                    pending.styles = Some(styletext::parse_style_text(body, len));
                }
            }
            // ExHyperlinkAtom: explicit degradation — hyperlink targets in
            // the legacy record stream are not resolved to link inlines.
            0x0FD3 => {
                log::debug!("ppt hyperlink records present; targets are not resolved");
            }
            _ => {}
        }
    }

    fn push_text(&mut self, text: String) {
        match &mut self.pending {
            Some(pending) => pending.text.push_str(&text),
            None => {
                self.pending = Some(PendingShape { tx_type: 1, text, styles: None });
            }
        }
    }

    /// Emit the pending shape: paragraphs split on CR, styled by the
    /// character runs, listed by paragraph depth/bullet with master defaults.
    fn flush_shape(&mut self) {
        let Some(shape) = self.pending.take() else {
            return;
        };
        if shape.text.is_empty() {
            return;
        }
        self.shape_counter += 1;
        let shape_id = self.shape_counter;
        let is_title = matches!(shape.tx_type, 0 | 6);
        let styles = shape.styles.unwrap_or_default();
        // The active master's per-level defaults for this text type; local
        // exceptions are tri-state and resolve over these.
        let master_levels: Vec<MasterLevel> = self
            .masters
            .get(self.active_master)
            .and_then(|(_, m)| m.get(&(shape.tx_type as u16)))
            .cloned()
            .unwrap_or_default();
        let level_default =
            |depth: u16| master_levels.get(depth as usize).copied().unwrap_or_default();

        // Cursors over the style runs, counted in UTF-16 units.
        let mut char_runs = styles.chars.iter();
        let mut char_run: Option<&CharProps> = char_runs.next();
        let mut char_left = char_run.map(|r| r.count).unwrap_or(usize::MAX);
        let mut para_runs = styles.paragraphs.iter();
        let mut para_run = para_runs.next();
        let mut para_left = para_run.map(|r| r.count).unwrap_or(usize::MAX);

        let mut paragraphs: Vec<(Vec<Inline>, u16, Option<bool>)> = Vec::new();
        let mut inlines: Vec<Inline> = Vec::new();
        let mut run_text = String::new();
        let mut run_style = Style::PLAIN;
        let para_props = |r: Option<&styletext::ParaProps>| match r {
            Some(p) => (p.depth, p.bullet),
            None => (0, None),
        };

        for c in shape.text.chars() {
            let d = level_default(para_props(para_run).0);
            let style = Style {
                bold: char_run.and_then(|r| r.bold).or(d.bold).unwrap_or(false),
                italic: char_run.and_then(|r| r.italic).or(d.italic).unwrap_or(false),
                strike: false,
                code: false,
            };
            if c == '\r' {
                if !run_text.is_empty() {
                    let text = clean_text(&std::mem::take(&mut run_text));
                    if !text.is_empty() {
                        inlines.push(Inline::Text { text, style: run_style });
                    }
                }
                let (depth, bullet) = para_props(para_run);
                paragraphs.push((std::mem::take(&mut inlines), depth, bullet));
            } else if c == '\u{b}' {
                if !run_text.is_empty() {
                    let text = clean_text(&std::mem::take(&mut run_text));
                    if !text.is_empty() {
                        inlines.push(Inline::Text { text, style: run_style });
                    }
                }
                inlines.push(Inline::LineBreak);
            } else {
                if style != run_style && !run_text.is_empty() {
                    let text = clean_text(&std::mem::take(&mut run_text));
                    if !text.is_empty() {
                        inlines.push(Inline::Text { text, style: run_style });
                    }
                }
                run_style = style;
                run_text.push(c);
            }
            // Advance run cursors by the character's UTF-16 width.
            let width = c.len_utf16();
            char_left = char_left.saturating_sub(width);
            if char_left == 0 {
                char_run = char_runs.next();
                char_left = char_run.map(|r| r.count).unwrap_or(usize::MAX);
            }
            para_left = para_left.saturating_sub(width);
            if para_left == 0 {
                para_run = para_runs.next();
                para_left = para_run.map(|r| r.count).unwrap_or(usize::MAX);
            }
        }
        if !run_text.is_empty() {
            let text = clean_text(&run_text);
            if !text.is_empty() {
                inlines.push(Inline::Text { text, style: run_style });
            }
        }
        if !inlines.is_empty() {
            let (depth, bullet) = para_props(para_run);
            paragraphs.push((inlines, depth, bullet));
        }

        for (mut inlines, depth, bullet) in paragraphs {
            if inlines_are_empty(&inlines) {
                flush_list(&mut self.current, &mut self.list_run);
                continue;
            }
            if is_title {
                flush_list(&mut self.current, &mut self.list_run);
                let d = level_default(depth);
                let base = StyleDelta { bold: d.bold, italic: d.italic, ..Default::default() };
                rebase_emphasis(&mut inlines, base.resolve());
                let anchor = Some(crate::model::inlines_to_plain_text(&inlines));
                self.current.push(Block::Heading { level: 2, anchor, content: inlines });
                continue;
            }
            let bullet = bullet.or(level_default(depth).bullet).unwrap_or(false);
            if bullet {
                self.list_run.push(ListEntry {
                    level: depth as usize,
                    key: ListKey { instance: shape_id, marker: MarkerKind::Bullet },
                    number: 0,
                    label: None,
                    blocks: vec![Block::Paragraph(inlines)],
                });
            } else {
                flush_list(&mut self.current, &mut self.list_run);
                self.current.push(Block::Paragraph(inlines));
            }
        }
    }
}

/// Fork (sznb): a shape rectangle in its parent's coordinate space.
#[derive(Debug, Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

fn get_i32(b: &[u8], off: usize) -> Option<i32> {
    get_u32(b, off).map(|v| v as i32)
}

fn get_i16(b: &[u8], off: usize) -> Option<i32> {
    crate::shared::binary::get_u16(b, off).map(|v| v as i16 as i32)
}

/// Fork (sznb): a group shape is a table when its own shape (the group's
/// first SpContainer) carries tableProperties (0x039F, low bit set) or
/// tableRowProperties (0x03A0) in its primary or tertiary property table.
fn is_table_group(group: &[u8]) -> bool {
    let Some((_, 0xF004, own)) = record_at(group, 0) else {
        return false;
    };
    children(own).filter(|&(_, t, _)| t == 0xF00B || t == 0xF122).any(|(vi, _, opt)| {
        let count = (vi >> 4) as usize;
        (0..count).any(|k| {
            let Some(head) = crate::shared::binary::get_u16(opt, 6 * k) else {
                return false;
            };
            let value = get_u32(opt, 6 * k + 2).unwrap_or(0);
            match head & 0x3FFF {
                0x039F => value & 1 != 0,
                0x03A0 => true,
                _ => false,
            }
        })
    })
}

/// Fork (sznb): a shape's anchor - OfficeArtChildAnchor (0xF00F,
/// left/top/right/bottom i32) inside groups, or the slide-level client
/// anchor (0xF010: top/left/right/bottom, i16 or i32).
fn anchor_of(shape: &[u8]) -> Option<Rect> {
    for (_, rec_type, body) in children(shape) {
        match (rec_type, body.len()) {
            (0xF00F, 16..) => {
                return Some(Rect {
                    left: get_i32(body, 0)?,
                    top: get_i32(body, 4)?,
                    right: get_i32(body, 8)?,
                    bottom: get_i32(body, 12)?,
                });
            }
            (0xF010, 8) => {
                return Some(Rect {
                    top: get_i16(body, 0)?,
                    left: get_i16(body, 2)?,
                    right: get_i16(body, 4)?,
                    bottom: get_i16(body, 6)?,
                });
            }
            (0xF010, 16..) => {
                return Some(Rect {
                    top: get_i32(body, 0)?,
                    left: get_i32(body, 4)?,
                    right: get_i32(body, 8)?,
                    bottom: get_i32(body, 12)?,
                });
            }
            _ => {}
        }
    }
    None
}

/// Edges closer than this merge into one boundary (master units, 576 per
/// inch): writers round each cell's edges independently.
const EDGE_TOLERANCE: i32 = 8;

/// Fork (sznb): sorted distinct edges, merging edges within the tolerance.
fn boundaries(edges: impl Iterator<Item = i32>) -> Vec<i32> {
    let mut v: Vec<i32> = edges.collect();
    v.sort_unstable();
    let mut out: Vec<i32> = Vec::new();
    for e in v {
        if out.last().is_none_or(|&last| e - last > EDGE_TOLERANCE) {
            out.push(e);
        }
    }
    out
}

/// Index of the boundary nearest to `edge`.
fn snap(edge: i32, bounds: &[i32]) -> usize {
    bounds
        .iter()
        .enumerate()
        .min_by_key(|&(_, &b)| (b - edge).abs())
        .map(|(i, _)| i)
        .unwrap_or(0)
}

#[cfg(test)]
mod table_tests {
    use super::*;
    use crate::model::CellSlot;

    #[test]
    fn boundaries_merge_close_edges() {
        assert_eq!(boundaries([0, 100, 103, 200, 0].into_iter()), vec![0, 100, 200]);
        assert_eq!(snap(104, &[0, 100, 200]), 1);
        assert_eq!(snap(199, &[0, 100, 200]), 2);
    }

    /// One record: header (ver_inst, type, len) + body.
    fn rec(ver_inst: u16, rec_type: u16, body: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&ver_inst.to_le_bytes());
        v.extend_from_slice(&rec_type.to_le_bytes());
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    fn shape(shape_type: u16, rect: [i32; 4], text: Option<&str>) -> Vec<u8> {
        let mut body = rec((shape_type << 4) | 2, 0xF00A, &[0; 8]);
        let anchor: Vec<u8> = rect.iter().flat_map(|v| v.to_le_bytes()).collect();
        body.extend(rec(0, 0xF00F, &anchor));
        if let Some(t) = text {
            let mut tb = rec(0, 0x0F9F, &[4, 0, 0, 0]);
            let units: Vec<u8> = t.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            tb.extend(rec(0, 0x0FA0, &units));
            body.extend(rec(0xF, 0xF00D, &tb));
        }
        rec(0xF, 0xF004, &body)
    }

    fn group(table: bool) -> Vec<u8> {
        let mut own = rec(0x1, 0xF009, &[0; 16]);
        if table {
            let mut opt = Vec::new();
            opt.extend_from_slice(&0x039Fu16.to_le_bytes());
            opt.extend_from_slice(&1u32.to_le_bytes());
            own.extend(rec((1 << 4) | 3, 0xF122, &opt));
        }
        let mut g = rec(0xF, 0xF004, &own);
        // 2 cols x 3 rows; column 0 rows 1-2 merged; edges off by 1 snap.
        g.extend(shape(1, [0, 0, 100, 50], Some("A")));
        g.extend(shape(1, [100, 0, 200, 50], Some("B")));
        g.extend(shape(1, [0, 50, 100, 150], Some("M")));
        g.extend(shape(1, [100, 50, 200, 100], Some("C")));
        g.extend(shape(1, [101, 100, 200, 150], None));
        g.extend(shape(20, [0, 50, 200, 50], None));
        g
    }

    fn text_of(slot: &CellSlot) -> String {
        match slot {
            CellSlot::Origin(c) => c
                .blocks
                .iter()
                .map(|b| match b {
                    Block::Paragraph(i) => crate::model::inlines_to_plain_text(i),
                    _ => String::new(),
                })
                .collect(),
            CellSlot::Covered { .. } => "<covered>".into(),
        }
    }

    #[test]
    fn table_group_rebuilds_grid_with_spans() {
        let g = group(true);
        assert!(is_table_group(&g));
        let mut ex = Extractor::default();
        let Some(Block::Table(t)) = ex.table_from_group(&g).unwrap() else {
            panic!("expected a table")
        };
        assert_eq!(t.grid.len(), 3);
        assert!(t.grid.iter().all(|r| r.len() == 2));
        let texts: Vec<Vec<String>> =
            t.grid.iter().map(|r| r.iter().map(text_of).collect()).collect();
        assert_eq!(texts, vec![vec!["A", "B"], vec!["M", "C"], vec!["<covered>", ""]]);
        match &t.grid[1][0] {
            CellSlot::Origin(c) => assert_eq!((c.col_span, c.row_span), (1, 2)),
            _ => panic!("merged origin missing"),
        }
        assert!(ex.current.is_empty(), "cell text must not leak into the slide");
    }

    #[test]
    fn plain_group_is_not_a_table() {
        assert!(!is_table_group(&group(false)));
    }
}
