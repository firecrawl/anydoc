//! Document-model → DocLang export for the non-PDF formats.
//!
//! These formats parse into [`Document`] with structure already resolved
//! (headings, lists, tables, notes), so the exporter is a direct mapping —
//! no geometry, hence no `<location>` elements and a single unbroken page.
//! Anchors and notes become `<thread>`/`<xref>` pairs; embedded images
//! become `<picture>` elements backed by `assets/` parts.

use crate::model::{
    Block, CellSlot, Document, ImageSource, Inline, LinkTarget, List, MarkerKind, Note, NoteKind,
    Style, Table, TableKind,
};
use crate::render::doclang::markup::{self, DocMeta};
use crate::render::doclang::package::{self, Part, SourcePart};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

/// Export a parsed [`Document`] to a `.dclx` (OPC ZIP) DocLang archive.
/// `source` carries the original bytes for the `/source` part;
/// `content_type` is its MIME type.
pub(crate) fn to_doclang(
    doc: &Document,
    source: &[u8],
    content_type: &'static str,
) -> Result<Vec<u8>, crate::error::ConvertError> {
    let mut ctx = Ctx::new(doc);
    let mut body = String::new();
    emit_blocks(&mut body, &doc.blocks, &mut ctx);

    // Notes collect at the end of the body, each a `<footnote>` threaded
    // to the `<xref>`s its `Inline::NoteRef`s emitted.
    let mut notes: Vec<(&Note, u32, usize)> = ctx
        .notes
        .iter()
        .filter_map(|(id, (thread, num))| {
            doc.notes.iter().find(|n| n.id.as_str() == *id).map(|n| (n, *thread, *num))
        })
        .collect();
    notes.sort_by_key(|(_, _, num)| *num);
    for (note, thread, _) in notes {
        out_footnote(&mut body, note, thread, &mut ctx);
    }

    let meta = DocMeta { title: None, sha256: hex(&Sha256::digest(source)), page_sizes: Vec::new() };
    let document_xml = markup::emit_document(&meta, &[body]);
    package::pack(
        document_xml.as_bytes(),
        &ctx.parts,
        Some(SourcePart { bytes: source, content_type }),
    )
}

/// Render state shared by every emit function: thread ids for anchors and
/// notes, note ordinals, and the asset-part table built during the walk.
struct Ctx<'a> {
    /// AnchorId → `<thread>`/`<xref>` id (1-based).
    threads: HashMap<&'a str, u32>,
    /// Note id → (`thread_id`, 1-based note number).
    notes: HashMap<&'a str, (u32, usize)>,
    /// `AssetId` index → archive URI under `assets/`.
    asset_paths: HashMap<usize, String>,
    /// Parts to store alongside `document.xml`.
    parts: Vec<Part>,
}

impl<'a> Ctx<'a> {
    fn new(doc: &'a Document) -> Self {
        let mut ctx = Ctx {
            threads: HashMap::new(),
            notes: HashMap::new(),
            asset_paths: HashMap::new(),
            parts: Vec::new(),
        };
        ctx.collect(doc);
        ctx
    }

    /// One pass over the document assigning thread ids (anchors, then
    /// notes), note numbers, and asset part paths before anything emits.
    fn collect(&mut self, doc: &'a Document) {
        let mut next = 1u32;
        let mut seen = HashSet::new();
        let blocks: Vec<&[Block]> = std::iter::once(doc.blocks.as_slice())
            .chain(doc.notes.iter().map(|n| n.blocks.as_slice()))
            .collect();
        for &blocks in &blocks {
            collect_anchors(blocks, &mut |id| {
                if seen.insert(id) {
                    self.threads.insert(id, next);
                    next += 1;
                }
            });
        }

        // Note numbers in first-reference order, unreferenced notes last —
        // the same convention the Markdown renderer uses.
        let valid: HashSet<&str> = doc
            .notes
            .iter()
            .filter(|n| !n.blocks.iter().all(block_is_blank))
            .map(|n| n.id.as_str())
            .collect();
        let mut order: Vec<&str> = Vec::new();
        let mut seen_refs = HashSet::new();
        collect_note_refs(&doc.blocks, &valid, &mut order, &mut seen_refs, &doc.notes);
        for note in &doc.notes {
            if valid.contains(note.id.as_str()) && seen_refs.insert(note.id.as_str()) {
                order.push(note.id.as_str());
            }
        }
        for (i, id) in order.iter().enumerate() {
            self.notes.insert(id, (next, i + 1));
            next += 1;
        }

        // Asset parts in first-reference order.
        let mut seen_assets = HashSet::new();
        for &blocks in &blocks {
            collect_assets(blocks, &mut |id| {
                if seen_assets.insert(id) {
                    let asset = &doc.assets[id];
                    let path =
                        format!("assets/img{}.{}", self.parts.len(), asset_ext(&asset.media_type));
                    self.asset_paths.insert(id, path.clone());
                    self.parts.push(Part { path, bytes: asset.bytes.clone() });
                }
            });
        }
    }
}

/// File extension for an asset's media type (`image/svg+xml` → `svg`).
fn asset_ext(media_type: &str) -> String {
    let subtype = media_type.rsplit('/').next().unwrap_or(media_type);
    let base = subtype.split('+').next().unwrap_or(subtype);
    let ext: String = base.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
    if ext.is_empty() { "bin".to_string() } else { ext }
}

fn block_is_blank(block: &Block) -> bool {
    matches!(block, Block::Paragraph(inlines) if !inlines_render(inlines))
}

/// Visit every anchor id in reading order (a heading's own anchor first,
/// then the inline anchors inside its content).
fn collect_anchors<'b>(blocks: &'b [Block], f: &mut impl FnMut(&'b str)) {
    fn walk_inlines<'b>(inlines: &'b [Inline], f: &mut impl FnMut(&'b str)) {
        for inline in inlines {
            match inline {
                Inline::Anchor(id) => f(id),
                Inline::Link { content, .. } => walk_inlines(content, f),
                _ => {}
            }
        }
    }
    fn walk_blocks<'b>(blocks: &'b [Block], f: &mut impl FnMut(&'b str)) {
        for block in blocks {
            match block {
                Block::Heading { anchor, content, .. } => {
                    if let Some(id) = anchor {
                        f(id);
                    }
                    walk_inlines(content, f);
                }
                Block::Paragraph(inlines) => walk_inlines(inlines, f),
                Block::List(list) => {
                    for item in &list.items {
                        walk_blocks(&item.blocks, f);
                    }
                }
                Block::Table(t) => {
                    for row in &t.grid {
                        for slot in row {
                            if let CellSlot::Origin(cell) = slot {
                                walk_blocks(&cell.blocks, f);
                            }
                        }
                    }
                }
                Block::BlockQuote(inner) => walk_blocks(inner, f),
                Block::CodeBlock { .. } | Block::Rule | Block::Math(_) => {}
            }
        }
    }
    walk_blocks(blocks, f);
}

/// Visit note ids in first-reference order, recursing into note bodies.
fn collect_note_refs<'b>(
    blocks: &'b [Block],
    valid: &HashSet<&'b str>,
    order: &mut Vec<&'b str>,
    seen: &mut HashSet<&'b str>,
    notes: &'b [Note],
) {
    fn walk_inlines<'b>(
        inlines: &'b [Inline],
        valid: &HashSet<&'b str>,
        order: &mut Vec<&'b str>,
        seen: &mut HashSet<&'b str>,
        notes: &'b [Note],
    ) {
        for inline in inlines {
            match inline {
                Inline::NoteRef(id) => {
                    if valid.contains(id.as_str())
                        && seen.insert(id.as_str())
                        && let Some(note) = notes.iter().find(|n| n.id == *id)
                    {
                        order.push(id.as_str());
                        collect_note_refs(&note.blocks, valid, order, seen, notes);
                    }
                }
                Inline::Link { content, .. } => walk_inlines(content, valid, order, seen, notes),
                _ => {}
            }
        }
    }
    for block in blocks {
        match block {
            Block::Paragraph(i) | Block::Heading { content: i, .. } => {
                walk_inlines(i, valid, order, seen, notes)
            }
            Block::List(list) => {
                for item in &list.items {
                    collect_note_refs(&item.blocks, valid, order, seen, notes);
                }
            }
            Block::Table(t) => {
                for row in &t.grid {
                    for slot in row {
                        if let CellSlot::Origin(cell) = slot {
                            collect_note_refs(&cell.blocks, valid, order, seen, notes);
                        }
                    }
                }
            }
            Block::BlockQuote(inner) => collect_note_refs(inner, valid, order, seen, notes),
            Block::CodeBlock { .. } | Block::Rule | Block::Math(_) => {}
        }
    }
}

/// Visit every embedded `Asset` id in reading order.
fn collect_assets(blocks: &[Block], f: &mut impl FnMut(usize)) {
    fn walk_inlines(inlines: &[Inline], f: &mut impl FnMut(usize)) {
        for inline in inlines {
            match inline {
                Inline::Image { source: ImageSource::Asset(id), .. } => f(id.0),
                Inline::Link { content, .. } => walk_inlines(content, f),
                _ => {}
            }
        }
    }
    fn walk_blocks(blocks: &[Block], f: &mut impl FnMut(usize)) {
        for block in blocks {
            match block {
                Block::Paragraph(i) | Block::Heading { content: i, .. } => walk_inlines(i, f),
                Block::List(list) => {
                    for item in &list.items {
                        walk_blocks(&item.blocks, f);
                    }
                }
                Block::Table(t) => {
                    for row in &t.grid {
                        for slot in row {
                            if let CellSlot::Origin(cell) = slot {
                                walk_blocks(&cell.blocks, f);
                            }
                        }
                    }
                }
                Block::BlockQuote(inner) => walk_blocks(inner, f),
                Block::CodeBlock { .. } | Block::Rule | Block::Math(_) => {}
            }
        }
    }
    walk_blocks(blocks, f);
}

/// True when the inlines would render any visible content or structure.
fn inlines_render(inlines: &[Inline]) -> bool {
    inlines.iter().any(|i| match i {
        Inline::Text { text, .. } => !text.trim().is_empty(),
        Inline::Link { content, target } => !target.is_empty() || inlines_render(content),
        Inline::Image { .. }
        | Inline::NoteRef(_)
        | Inline::Checkbox(_)
        | Inline::Anchor(_) => true,
        Inline::Math(tex) => !tex.trim().is_empty(),
        Inline::LineBreak => false,
    })
}

fn emit_blocks(out: &mut String, blocks: &[Block], ctx: &mut Ctx) {
    for block in blocks {
        emit_block(out, block, ctx);
    }
}

fn emit_block(out: &mut String, block: &Block, ctx: &mut Ctx) {
    match block {
        Block::Heading { level, anchor, content } => {
            if !inlines_render(content) && anchor.is_none() {
                return;
            }
            let _ = write!(out, "<heading level=\"{}\">", (*level).max(1));
            if let Some(id) = anchor
                && let Some(thread) = ctx.threads.get(id.as_str())
            {
                let _ = write!(out, "<thread thread_id=\"{thread}\"/>");
            }
            emit_inlines(out, content, ctx);
            out.push_str("</heading>");
        }
        Block::Paragraph(inlines) => {
            if !inlines_render(inlines) {
                return;
            }
            out.push_str("<text>");
            emit_inlines(out, inlines, ctx);
            out.push_str("</text>");
        }
        Block::List(list) => emit_list(out, list, ctx),
        // Trivial layout tables are scaffolding; render their content directly.
        Block::Table(t) if t.kind == TableKind::Layout && t.is_single_cell() => {
            let CellSlot::Origin(cell) = &t.grid[0][0] else { unreachable!() };
            emit_blocks(out, &cell.blocks, ctx);
        }
        Block::Table(t) => emit_table(out, t, ctx),
        Block::BlockQuote(inner) => emit_blocks(out, inner, ctx),
        Block::CodeBlock { lang, text } => {
            out.push_str("<code>");
            if let Some(lang) = lang.as_deref().filter(|l| !l.trim().is_empty()) {
                let _ = write!(out, "<label value=\"{}\"/>", markup::esc(lang.trim()));
            }
            out.push_str("<content>");
            out.push_str(&markup::esc(text.trim_end_matches('\n')));
            out.push_str("</content></code>");
        }
        Block::Rule => out.push_str("<text>---</text>"),
        Block::Math(tex) => {
            let tex = tex.trim();
            if tex.is_empty() {
                return;
            }
            let _ = write!(out, "<formula>{}</formula>", markup::esc(tex));
        }
    }
}

fn emit_list(out: &mut String, list: &List, ctx: &mut Ctx) {
    if list.items.is_empty() {
        return;
    }
    if list.ordered() {
        out.push_str("<list class=\"ordered\">");
    } else {
        out.push_str("<list>");
    }
    for (i, item) in list.items.iter().enumerate() {
        out.push_str("<ldiv><marker>");
        if let Some(label) = &item.marker_label {
            out.push_str(&markup::esc(label));
        } else {
            match list.marker {
                MarkerKind::Bullet => out.push('•'),
                kind => {
                    let _ = write!(out, "{}", kind.label(list.start.saturating_add(i as u64)));
                }
            }
        }
        out.push_str("</marker></ldiv>");
        emit_blocks(out, &item.blocks, ctx);
    }
    out.push_str("</list>");
}

fn emit_table(out: &mut String, table: &Table, ctx: &mut Ctx) {
    out.push_str("<table>");
    for (r, row) in table.grid.iter().enumerate() {
        if r > 0 {
            out.push_str("<nl/>");
        }
        for (c, slot) in row.iter().enumerate() {
            match slot {
                CellSlot::Origin(cell) => {
                    let token = if cell.is_empty() {
                        "ecel"
                    } else if r < table.header_rows {
                        "ched"
                    } else {
                        "fcel"
                    };
                    let _ = write!(out, "<{token}/>");
                    emit_blocks(out, &cell.blocks, ctx);
                }
                CellSlot::Covered { origin_row, origin_col } => {
                    // Same leader-relative rule the PDF path uses: same
                    // row as the origin extends horizontally (`lcel`),
                    // same column vertically (`ucel`), otherwise `xcel`.
                    let token = if r == *origin_row {
                        "lcel"
                    } else if c == *origin_col {
                        "ucel"
                    } else {
                        "xcel"
                    };
                    let _ = write!(out, "<{token}/>");
                }
            }
        }
    }
    out.push_str("</table>");
}

fn emit_inlines(out: &mut String, inlines: &[Inline], ctx: &mut Ctx) {
    for inline in inlines {
        match inline {
            Inline::Text { text, style } => emit_text(out, text, *style),
            Inline::Link { content, target } => emit_link(out, content, target, ctx),
            Inline::Image { alt, source } => emit_image(out, alt, source, ctx),
            Inline::Anchor(id) => {
                if let Some(thread) = ctx.threads.get(id.as_str()) {
                    let _ = write!(out, "<text><thread thread_id=\"{thread}\"/></text>");
                }
            }
            Inline::NoteRef(id) => {
                if let Some((thread, num)) = ctx.notes.get(id.as_str()) {
                    let _ = write!(
                        out,
                        "<text><xref thread_id=\"{thread}\"/><superscript>{num}</superscript></text>"
                    );
                } else {
                    log::debug!("unresolved note reference: {id}");
                }
            }
            Inline::LineBreak => out.push('\n'),
            Inline::Math(tex) => {
                let tex = tex.trim();
                if !tex.is_empty() {
                    let _ = write!(out, "<formula>{}</formula>", markup::esc(tex));
                }
            }
            Inline::Checkbox(checked) => {
                let class = if *checked { "selected" } else { "unselected" };
                let _ = write!(out, "<checkbox class=\"{class}\"/>");
            }
        }
    }
}

/// Styled text run. `<code>` sits outermost — the other formatting
/// elements admit raw text only, while `<code>` admits them.
fn emit_text(out: &mut String, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if style.code {
        out.push_str("<code>");
    }
    if style.bold {
        out.push_str("<bold>");
    }
    if style.italic {
        out.push_str("<italic>");
    }
    if style.strike {
        out.push_str("<strikethrough>");
    }
    out.push_str(&markup::esc(text));
    if style.strike {
        out.push_str("</strikethrough>");
    }
    if style.italic {
        out.push_str("</italic>");
    }
    if style.bold {
        out.push_str("</bold>");
    }
    if style.code {
        out.push_str("</code>");
    }
}

fn emit_link(out: &mut String, content: &[Inline], target: &LinkTarget, ctx: &mut Ctx) {
    let head = match target {
        LinkTarget::External(url) | LinkTarget::Relative(url) if !url.is_empty() => {
            format!("<text><href uri=\"{}\"/>", markup::esc(url))
        }
        LinkTarget::Anchor(id) => match ctx.threads.get(id.as_str()) {
            Some(thread) => format!("<text><xref thread_id=\"{thread}\"/>"),
            None => {
                log::debug!("unresolved internal link target: {id}");
                emit_inlines(out, content, ctx);
                return;
            }
        },
        _ => {
            emit_inlines(out, content, ctx);
            return;
        }
    };
    out.push_str(&head);
    emit_inlines(out, content, ctx);
    out.push_str("</text>");
}

fn emit_image(out: &mut String, alt: &str, source: &ImageSource, ctx: &mut Ctx) {
    let uri = match source {
        ImageSource::External(url) => Some(url.clone()),
        ImageSource::Asset(id) => ctx.asset_paths.get(&id.0).cloned(),
        ImageSource::Unavailable => None,
    };
    match uri {
        Some(uri) => {
            out.push_str("<picture>");
            let alt = alt.trim();
            if !alt.is_empty() {
                let _ = write!(out, "<description>{}</description>", markup::esc(alt));
            }
            let _ = write!(out, "<src uri=\"{}\"/>", markup::esc(&uri));
            out.push_str("</picture>");
        }
        // No usable source: degrade to the alt text, like the Markdown path.
        None => {
            let alt = alt.trim();
            if !alt.is_empty() {
                out.push_str(&markup::esc(alt));
            }
        }
    }
}

fn out_footnote(out: &mut String, note: &Note, thread: u32, ctx: &mut Ctx) {
    out.push_str("<footnote>");
    if note.kind == NoteKind::Endnote {
        out.push_str("<label value=\"endnote\"/>");
    }
    let _ = write!(out, "<thread thread_id=\"{thread}\"/>");
    emit_blocks(out, &note.blocks, ctx);
    out.push_str("</footnote>");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
