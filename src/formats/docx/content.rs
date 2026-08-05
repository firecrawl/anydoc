//! Block and inline walking for WordprocessingML parts.

use crate::error::ConvertError;
use crate::formats::docx::numbering::{Counters, Numbering};
use crate::formats::docx::styles::{Styles, on_off, rpr_delta};
use crate::model::{
    Block, Cell, GridBuilder, ImageSource, Inline, LinkTarget, Style, TableKind, inlines_are_empty,
};
use crate::package::Package;
use crate::package::relationships::{RelTarget, Relationships, TargetMode, rel_target_bytes};
use crate::package::xml::{Element, ns};
use crate::shared::delta::rebase_emphasis;
use crate::shared::fields::{FieldFrame, field_result};
use crate::shared::header::resolve_header_rows;
use crate::shared::list::{ListEntry, ListKey, flush_list};
use crate::shared::text::{clean_text, is_xml_space};
use std::cell::RefCell;
use std::collections::HashMap;

/// Namespaces whose markup this frontend understands; `mc:Choice` branches
/// requiring anything else fall back to `mc:Fallback`.
const SUPPORTED_NS: &[&str] = &[
    ns::W,
    ns::A,
    ns::PIC,
    ns::WP,
    ns::MC,
    ns::CHART,
    ns::DGM,
    ns::VML,
    ns::O_VML,
    ns::WPS,
    ns::WPG,
];

use crate::shared::assets::AssetSink;

pub(super) struct Ctx<'a, 'b> {
    pub pkg: &'b RefCell<Package<'a>>,
    pub rels: Relationships,
    pub base_part: String,
    pub styles: &'b Styles<'b>,
    pub numbering: &'b Numbering,
    pub counters: &'b RefCell<Counters>,
    pub assets: &'b RefCell<AssetSink>,
}

impl<'a, 'b> Ctx<'a, 'b> {
    /// The same document-wide dependencies scoped to another package part
    /// (notes parts): only the relationships and base path change.
    pub fn for_part(&self, rels: Relationships, base_part: String) -> Ctx<'a, 'b> {
        Ctx {
            pkg: self.pkg,
            rels,
            base_part,
            styles: self.styles,
            numbering: self.numbering,
            counters: self.counters,
            assets: self.assets,
        }
    }

    /// Load an internal relationship target's bytes, resolved against this
    /// part. Failures degrade (log + `None`) per the unified policy;
    /// resource-limit errors always propagate.
    fn rel_part(&self, rel_id: &str) -> Result<Option<RelTarget>, ConvertError> {
        rel_target_bytes(self.pkg, &self.rels, &self.base_part, rel_id)
    }

    fn add_asset(
        &self,
        media_type: String,
        part: String,
        bytes: &[u8],
    ) -> Result<crate::model::AssetId, ConvertError> {
        self.assets.borrow_mut().add(media_type, part, bytes)
    }

    /// Pick the branch of an `mc:AlternateContent` to process.
    fn alternate_branch<'e>(&self, alt: &'e Element) -> Option<&'e Element> {
        crate::shared::mc::alternate_branch(alt, SUPPORTED_NS)
    }
}

pub(super) enum ParaKind {
    Heading {
        level: u8,
        /// The visible number of a numbered heading (with its trailing
        /// separator), prepended to the content: headings have no native
        /// numbering in the output.
        label: Option<String>,
        /// The heading style's own emphasis, subtracted from its runs.
        base: Style,
    },
    ListItem {
        ilvl: usize,
        key: ListKey,
        number: u64,
        label: Option<String>,
    },
    Plain,
}

/// A paragraph's content in source order: inline runs interleaved with
/// block attachments (text boxes, charts) at their anchor positions.
pub(super) enum Piece {
    Inlines(Vec<Inline>),
    Blocks(Vec<Block>),
}

pub(super) fn parse_blocks(parent: &Element, ctx: &Ctx) -> Result<Vec<Block>, ConvertError> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut list_run: Vec<ListEntry> = Vec::new();
    collect_blocks(parent, ctx, &mut blocks, &mut list_run)?;
    flush_list(&mut blocks, &mut list_run);
    Ok(blocks)
}

fn collect_blocks(
    parent: &Element,
    ctx: &Ctx,
    blocks: &mut Vec<Block>,
    list_run: &mut Vec<ListEntry>,
) -> Result<(), ConvertError> {
    for child in parent.child_elems() {
        if child.is(ns::MC, "AlternateContent") {
            if let Some(branch) = ctx.alternate_branch(child) {
                collect_blocks(branch, ctx, blocks, list_run)?;
            }
            continue;
        }
        if child.ns.as_deref().is_none_or(|n| n != ns::W) {
            continue;
        }
        match child.local.as_str() {
            "p" => {
                let (kind, pieces) = parse_paragraph(child, ctx)?;
                emit_paragraph(kind, pieces, blocks, list_run);
            }
            "tbl" => {
                flush_list(blocks, list_run);
                blocks.extend(parse_table(child, ctx)?);
            }
            "sdt" => {
                if let Some(content) = child.find(ns::W, "sdtContent") {
                    collect_blocks(content, ctx, blocks, list_run)?;
                }
            }
            "customXml" => collect_blocks(child, ctx, blocks, list_run)?,
            _ => {}
        }
    }
    Ok(())
}

fn emit_paragraph(
    kind: ParaKind,
    pieces: Vec<Piece>,
    blocks: &mut Vec<Block>,
    list_run: &mut Vec<ListEntry>,
) {
    match kind {
        ParaKind::ListItem { ilvl, key, number, label } => {
            // Attachments anchored in a list paragraph stay inside the item,
            // so the surrounding list run is never severed.
            let (inlines, item_blocks) = split_pieces(pieces);
            let mut item = vec![Block::Paragraph(inlines)];
            item.extend(item_blocks);
            list_run.push(ListEntry { level: ilvl, key, number, label, blocks: item });
        }
        ParaKind::Heading { level, label, base } => {
            flush_list(blocks, list_run);
            let (mut content, after) = split_pieces(pieces);
            if !inlines_are_empty(&content) {
                rebase_emphasis(&mut content, base);
                if let Some(label) = label {
                    content.insert(0, Inline::Text { text: label, style: Style::PLAIN });
                }
                blocks.push(Block::Heading { level, anchor: None, content });
            }
            blocks.extend(after);
        }
        ParaKind::Plain => {
            flush_list(blocks, list_run);
            // Emit in source order: paragraph text split around attachments.
            for piece in pieces {
                match piece {
                    Piece::Inlines(inlines) => blocks.push(Block::Paragraph(inlines)),
                    Piece::Blocks(b) => blocks.extend(b),
                }
            }
        }
    }
}

fn parse_paragraph(p: &Element, ctx: &Ctx) -> Result<(ParaKind, Vec<Piece>), ConvertError> {
    let ppr = p.find(ns::W, "pPr");
    let pstyle_id = ppr.and_then(|pr| pr.find(ns::W, "pStyle")).and_then(|e| e.attr(ns::W, "val"));

    // Direct paragraph properties overlay the style chain: an explicit
    // outlineLvl of 9 ("no outline level") turns a style heading off.
    let direct_outline: Option<Option<u8>> = ppr
        .and_then(|pr| pr.find(ns::W, "outlineLvl"))
        .and_then(|e| e.attr(ns::W, "val"))
        .and_then(|v| v.parse::<u8>().ok())
        .map(|l| if l < 9 { Some(l + 1) } else { None });
    let style_heading = match pstyle_id {
        Some(id) => ctx.styles.heading_level(id)?,
        None => None,
    };
    let heading = direct_outline.or(style_heading).flatten();

    // Numbering resolves independently of heading semantics: a numbered
    // heading advances its sequence and keeps its number visible.
    let numbering = resolve_numbering(ppr, pstyle_id, ctx)?;

    // Toggle properties: the paragraph style chain's true-parity flips the
    // docDefaults base. Headings use the same resolution as body text.
    let parity = match pstyle_id {
        Some(id) => ctx.styles.run_toggles(id)?,
        None => Default::default(),
    };
    let paragraph_level = parity.apply_over(ctx.styles.doc_defaults);

    let kind = match heading {
        Some(level) => {
            let label = match &numbering {
                Some((_, key, number, label)) if key.marker.ordered() => {
                    Some(format!("{} ", label.clone().unwrap_or_else(|| key.marker.label(*number))))
                }
                _ => None,
            };
            ParaKind::Heading { level, label, base: paragraph_level }
        }
        None => match numbering {
            Some((ilvl, key, number, label)) => ParaKind::ListItem { ilvl, key, number, label },
            None => ParaKind::Plain,
        },
    };

    let mut walker = InlineWalker::new(ctx, paragraph_level);
    walker.walk(p)?;
    Ok((kind, walker.finish()))
}

/// Resolve a paragraph's effective numbering per ECMA-376: the direct
/// `numPr` children are tri-state and merge property-by-property with the
/// style-inherited `numPr` (a missing `numId`/`ilvl` inherits; an explicit
/// `numId` of 0 suppresses). Returns the level, list identity, effective
/// number, and composite label, advancing the instance counters.
#[expect(clippy::type_complexity)]
fn resolve_numbering(
    ppr: Option<&Element>,
    pstyle_id: Option<&str>,
    ctx: &Ctx,
) -> Result<Option<(usize, ListKey, u64, Option<String>)>, ConvertError> {
    let direct = ppr.and_then(|pr| pr.find(ns::W, "numPr"));
    let direct_num_id: Option<u64> = direct
        .and_then(|numpr| numpr.find(ns::W, "numId"))
        .and_then(|e| e.attr(ns::W, "val"))
        .and_then(|v| v.parse().ok());
    let direct_ilvl: Option<usize> = direct
        .and_then(|numpr| numpr.find(ns::W, "ilvl"))
        .and_then(|e| e.attr(ns::W, "val"))
        .and_then(|v| v.parse().ok());

    let num_id = match direct_num_id {
        Some(id) => Some(id),
        None => match pstyle_id {
            Some(id) => ctx.styles.style_num_pr(id)?,
            None => None,
        },
    };
    let Some(num_id) = num_id else {
        return Ok(None);
    };
    if num_id == 0 {
        // numId 0 is explicitly suppressed numbering.
        return Ok(None);
    }
    let Some(instance) = ctx.numbering.instance(num_id) else {
        log::debug!("paragraph references undefined numbering instance {num_id}");
        return Ok(None);
    };
    let ilvl = match (direct_ilvl, pstyle_id) {
        (Some(l), _) => l,
        // Style-referenced numbering carries no usable ilvl (§17.3.1.19);
        // the level comes from the abstract levels' pStyle bindings.
        (None, Some(id)) => ctx.styles.style_numbering_level(id, instance)?.unwrap_or(0),
        (None, None) => 0,
    };
    let def = &instance.levels[ilvl.min(crate::formats::docx::numbering::LEVELS - 1)];
    // A numFmt of "none" is explicitly suppressed numbering.
    let Some(marker) = def.marker else {
        return Ok(None);
    };
    let (number, label) = if marker.ordered() {
        ctx.counters.borrow_mut().next(num_id, ilvl, instance)
    } else {
        (0, None)
    };
    Ok(Some((ilvl, ListKey { instance: num_id, marker }, number, label)))
}

struct InlineWalker<'a, 'b, 'e> {
    ctx: &'e Ctx<'a, 'b>,
    base: Style,
    pieces: Vec<Piece>,
    current: Vec<Inline>,
    fields: Vec<FieldFrame>,
}

impl<'a, 'b, 'e> InlineWalker<'a, 'b, 'e> {
    fn new(ctx: &'e Ctx<'a, 'b>, base: Style) -> Self {
        InlineWalker { ctx, base, pieces: Vec::new(), current: Vec::new(), fields: Vec::new() }
    }

    fn push(&mut self, inline: Inline) {
        match self.fields.last_mut() {
            Some(f) if f.in_result => f.inlines.push(inline),
            Some(_) => {}
            None => self.current.push(inline),
        }
    }

    /// Attach block content at the current position in run order.
    fn push_blocks(&mut self, blocks: Vec<Block>) {
        if blocks.is_empty() {
            return;
        }
        if !self.current.is_empty() {
            self.pieces.push(Piece::Inlines(std::mem::take(&mut self.current)));
        }
        self.pieces.push(Piece::Blocks(blocks));
    }

    fn walk(&mut self, elem: &Element) -> Result<(), ConvertError> {
        for child in elem.child_elems() {
            if child.is(ns::MC, "AlternateContent") {
                if let Some(branch) = self.ctx.alternate_branch(child) {
                    self.walk(branch)?;
                }
                continue;
            }
            if child.ns.as_deref().is_none_or(|n| n != ns::W) {
                continue;
            }
            match child.local.as_str() {
                "pPr" => {}
                "r" => self.walk_run(child)?,
                "hyperlink" => {
                    let target = self.hyperlink_link_target(child);
                    let mut inner = InlineWalker::new(self.ctx, self.base);
                    inner.walk(child)?;
                    let (content, attachments) = split_pieces(inner.finish());
                    if let Some(target) = target {
                        // An empty label still keeps a resolved target: the
                        // renderer shows the URL as the link text.
                        self.push(Inline::Link { content, target });
                    } else {
                        for inline in content {
                            self.push(inline);
                        }
                    }
                    self.push_blocks(attachments);
                }
                "fldSimple" => {
                    let instr = child.attr(ns::W, "instr").unwrap_or("").to_string();
                    let mut inner = InlineWalker::new(self.ctx, self.base);
                    inner.walk(child)?;
                    let (content, attachments) = split_pieces(inner.finish());
                    self.push_field_result(&instr, content);
                    self.push_blocks(attachments);
                }
                "bookmarkStart" => {
                    if let Some(name) = child.attr(ns::W, "name")
                        && name != "_GoBack"
                    {
                        self.push(Inline::Anchor(name.to_string()));
                    }
                }
                "sdt" => {
                    if let Some(content) = child.find(ns::W, "sdtContent") {
                        self.walk(content)?;
                    }
                }
                // `moveTo` is moved-in text — part of the final document;
                // `customXml` wraps ordinary run content.
                "smartTag" | "ins" | "bdo" | "dir" | "moveTo" | "customXml" => self.walk(child)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn hyperlink_link_target(&self, link: &Element) -> Option<LinkTarget> {
        if let Some(id) = link.attr_qualified(ns::R, "id")
            && let Some(rel) = self.ctx.rels.get(id)
        {
            return Some(crate::shared::fields::classify_rel_target(
                rel.mode == TargetMode::External,
                &rel.target,
            ));
        }
        link.attr(ns::W, "anchor").map(|a| LinkTarget::Anchor(a.to_string()))
    }

    fn walk_run(&mut self, run: &Element) -> Result<(), ConvertError> {
        let style = match run.find(ns::W, "rPr") {
            Some(rpr) => {
                // Character-style chain: another toggle layer over the
                // paragraph-level value. Direct formatting is absolute.
                let char_parity = match rpr.find(ns::W, "rStyle").and_then(|e| e.attr(ns::W, "val"))
                {
                    Some(id) => self.ctx.styles.run_toggles(id)?,
                    None => Default::default(),
                };
                let with_char = char_parity.apply_over(self.base);
                rpr_delta(rpr).apply(with_char)
            }
            None => self.base,
        };
        self.walk_run_content(run, style)
    }

    fn walk_run_content(&mut self, run: &Element, style: Style) -> Result<(), ConvertError> {
        for child in run.child_elems() {
            if child.is(ns::MC, "AlternateContent") {
                if let Some(branch) = self.ctx.alternate_branch(child) {
                    self.walk_run_content(branch, style)?;
                }
                continue;
            }
            let in_w = child.ns.as_deref().is_some_and(|n| n == ns::W);
            if !in_w {
                continue;
            }
            match child.local.as_str() {
                "t" => {
                    // Open XML text-space contract: edge whitespace in w:t
                    // is significant only under xml:space="preserve";
                    // unmarked edges are discarded (Word never renders them).
                    // Only XML whitespace counts: a no-break space is
                    // character data, not whitespace the contract may drop.
                    let preserved = child.attr_qualified(ns::XML, "space") == Some("preserve");
                    let raw = child.text();
                    // The contract applies to the XML text, before
                    // normalization turns a no-break space into a space.
                    let text = clean_text(if preserved {
                        raw.as_ref()
                    } else {
                        raw.trim_matches(is_xml_space)
                    });
                    if !text.is_empty() {
                        self.push(Inline::Text { text, style });
                    }
                }
                "tab" | "ptab" => self.push(Inline::Text { text: " ".into(), style: Style::PLAIN }),
                // Markdown has no pages or columns, but every w:br still
                // separates the runs around it: dropping a page break
                // outright would join the words on either side. One left at
                // the end of a paragraph is trimmed when the block renders.
                "br" => self.push(Inline::LineBreak),
                "cr" => self.push(Inline::LineBreak),
                "footnoteReference" => {
                    if let Some(id) = child.attr(ns::W, "id") {
                        self.push(Inline::NoteRef(format!("fn{id}")));
                    }
                }
                "endnoteReference" => {
                    if let Some(id) = child.attr(ns::W, "id") {
                        self.push(Inline::NoteRef(format!("en{id}")));
                    }
                }
                "drawing" | "pict" | "object" => self.walk_drawing(child)?,
                "fldChar" => match child.attr(ns::W, "fldCharType") {
                    Some("begin") => self.fields.push(FieldFrame::default()),
                    Some("separate") => {
                        if let Some(f) = self.fields.last_mut() {
                            f.in_result = true;
                        }
                    }
                    Some("end") => {
                        if let Some(FieldFrame { instr, inlines, .. }) = self.fields.pop() {
                            self.push_field_result(&instr, inlines);
                        }
                    }
                    _ => {}
                },
                "instrText" => {
                    if let Some(f) = self.fields.last_mut() {
                        f.instr.push_str(&child.text());
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Drawings, VML picts, and embedded objects: text boxes become block
    /// attachments at this position; images/charts/diagrams/objects resolve
    /// through relationships.
    fn walk_drawing(&mut self, elem: &Element) -> Result<(), ConvertError> {
        // Text boxes first: their content is the drawing's content.
        let mut boxes = Vec::new();
        collect_text_boxes(elem, &mut boxes);
        if !boxes.is_empty() {
            let mut blocks = Vec::new();
            for tb in boxes {
                blocks.extend(parse_blocks(tb, self.ctx)?);
            }
            self.push_blocks(blocks);
            return Ok(());
        }

        let descr = elem
            .first_descendant(ns::WP, "docPr")
            .and_then(|d| d.attr(ns::WP, "descr"))
            .map(clean_text)
            .unwrap_or_default();

        // Charts and SmartArt render their textual content.
        if let Some(chart_ref) = elem.first_descendant(ns::CHART, "chart")
            && let Some(rel_id) = chart_ref.attr_qualified(ns::R, "id")
        {
            self.push_blocks(self.chart_blocks(rel_id)?);
            return Ok(());
        }
        if let Some(rel_ids) = elem.first_descendant(ns::DGM, "relIds")
            && let Some(rel_id) = rel_ids.attr_qualified(ns::R, "dm")
        {
            self.push_blocks(self.diagram_blocks(rel_id)?);
            return Ok(());
        }

        // Embedded OLE objects come before images: standard Word OLE markup
        // carries a VML preview image next to the o:OLEObject, and the
        // object's identity and payload must win over its preview.
        if let Some(ole) = elem.first_descendant(ns::O_VML, "OLEObject") {
            let prog_id = ole.attr(ns::O_VML, "ProgID").unwrap_or("object").to_string();
            let alt = if descr.trim().is_empty() {
                format!("Embedded object: {prog_id}")
            } else {
                descr.clone()
            };
            let source = match ole.attr_qualified(ns::R, "id") {
                Some(rel_id) => match self.ctx.rel_part(rel_id)? {
                    Some((part, bytes)) => Some(ImageSource::Asset(self.ctx.add_asset(
                        "application/vnd.ms-ole-object".into(),
                        part,
                        &bytes,
                    )?)),
                    None => None,
                },
                None => None,
            };
            self.push(Inline::Image { alt, source: source.unwrap_or(ImageSource::Unavailable) });
            return Ok(());
        }

        // Bitmap images: DrawingML blip or VML imagedata.
        let image_rel = elem
            .first_descendant(ns::A, "blip")
            .and_then(|b| {
                b.attr_qualified(ns::R, "embed").or_else(|| b.attr_qualified(ns::R, "link"))
            })
            .or_else(|| {
                elem.first_descendant(ns::VML, "imagedata")
                    .and_then(|i| i.attr_qualified(ns::R, "id"))
            });
        if let Some(rel_id) = image_rel {
            // External-mode targets (`r:link`) become external image
            // sources; embedded targets are retained as assets.
            let source = crate::shared::assets::rel_image_source(
                self.ctx.pkg,
                &self.ctx.rels,
                &self.ctx.base_part,
                self.ctx.assets,
                rel_id,
            )?;
            match source {
                Some(source) => self.push(Inline::Image { alt: descr, source }),
                None => {
                    if !descr.trim().is_empty() {
                        self.push(Inline::Image { alt: descr, source: ImageSource::Unavailable });
                    }
                }
            }
            return Ok(());
        }

        if !descr.trim().is_empty() {
            self.push(Inline::Image { alt: descr, source: ImageSource::Unavailable });
        }
        Ok(())
    }

    /// Textual extraction of a chart part via `shared::drawingml`.
    fn chart_blocks(&self, rel_id: &str) -> Result<Vec<Block>, ConvertError> {
        let Some((part, bytes)) = self.ctx.rel_part(rel_id)? else {
            return Ok(Vec::new());
        };
        match crate::package::xml::parse_xml(&bytes) {
            Ok(root) => Ok(crate::shared::drawingml::chart_blocks(&root)),
            Err(e) if e.is_fatal() => Err(e),
            Err(e) => {
                log::warn!("skipping corrupt chart part {part}: {e}");
                Ok(Vec::new())
            }
        }
    }

    /// Textual extraction of a SmartArt data part via `shared::drawingml`.
    fn diagram_blocks(&self, rel_id: &str) -> Result<Vec<Block>, ConvertError> {
        let Some((part, bytes)) = self.ctx.rel_part(rel_id)? else {
            return Ok(Vec::new());
        };
        match crate::package::xml::parse_xml(&bytes) {
            Ok(root) => Ok(crate::shared::drawingml::diagram_blocks(&root)),
            Err(e) if e.is_fatal() => Err(e),
            Err(e) => {
                log::warn!("skipping corrupt diagram part {part}: {e}");
                Ok(Vec::new())
            }
        }
    }

    fn push_field_result(&mut self, instr: &str, content: Vec<Inline>) {
        for inline in field_result(instr, content) {
            self.push(inline);
        }
    }

    fn finish(mut self) -> Vec<Piece> {
        while let Some(frame) = self.fields.pop() {
            for inline in frame.inlines {
                self.push(inline);
            }
        }
        if !self.current.is_empty() {
            self.pieces.push(Piece::Inlines(self.current));
        }
        self.pieces
    }
}

fn split_pieces(pieces: Vec<Piece>) -> (Vec<Inline>, Vec<Block>) {
    let mut inlines = Vec::new();
    let mut blocks = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Inlines(mut i) => inlines.append(&mut i),
            Piece::Blocks(b) => blocks.extend(b),
        }
    }
    (inlines, blocks)
}

/// Find text-box content (`w:txbxContent`) in a drawing or VML pict, skipping
/// `mc:Fallback` so AlternateContent shapes aren't collected twice.
fn collect_text_boxes<'e>(elem: &'e Element, out: &mut Vec<&'e Element>) {
    for child in elem.child_elems() {
        if child.is(ns::MC, "Fallback") {
            continue;
        }
        if child.is(ns::W, "txbxContent") {
            out.push(child);
        } else {
            collect_text_boxes(child, out);
        }
    }
}

// ---------------------------------------------------------------------------
// Tables

struct TcInfo<'e> {
    elem: Option<&'e Element>,
    /// Legacy `hMerge` continuation cells folded into this origin.
    merged: Vec<&'e Element>,
    col_span: usize,
    row_span: usize,
    /// vMerge continuation: this position belongs to the origin above.
    covered: bool,
}

impl TcInfo<'_> {
    /// Empty single-column filler for `gridBefore`/`gridAfter` positions.
    fn filler() -> Self {
        TcInfo { elem: None, merged: Vec::new(), col_span: 1, row_span: 1, covered: false }
    }
}

/// A `w:trPr` grid filler count (`gridBefore`/`gridAfter`).
fn grid_filler(trpr: Option<&Element>, name: &str) -> usize {
    trpr.and_then(|p| p.find(ns::W, name))
        .and_then(|e| e.attr(ns::W, "val"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
        .min(1000)
}

pub(super) fn parse_table(tbl: &Element, ctx: &Ctx) -> Result<Vec<Block>, ConvertError> {
    // Collect the raw cell matrix first so vertical merges can be resolved
    // into row spans before the grid is built. gridBefore/gridAfter filler
    // materializes as empty cells so every cell keeps its grid column.
    let mut matrix: Vec<Vec<TcInfo>> = Vec::new();
    for tr in tbl.find_all(ns::W, "tr") {
        let trpr = tr.find(ns::W, "trPr");
        let mut row = Vec::new();
        row.extend((0..grid_filler(trpr, "gridBefore")).map(|_| TcInfo::filler()));
        collect_row_cells(tr, &mut row);
        row.extend((0..grid_filler(trpr, "gridAfter")).map(|_| TcInfo::filler()));
        matrix.push(row);
    }
    // Active vertical-merge chains by grid column.
    let mut active: HashMap<usize, (usize, usize)> = HashMap::new();
    let rows_len = matrix.len();
    for r in 0..rows_len {
        let mut col = 0usize;
        let mut next_active: Vec<(usize, (usize, usize))> = Vec::new();
        for i in 0..matrix[r].len() {
            let (covered, span) = (matrix[r][i].covered, matrix[r][i].col_span);
            if covered {
                if let Some(&(orow, oidx)) = active.get(&col) {
                    matrix[orow][oidx].row_span += 1;
                    for c in col..col + span {
                        next_active.push((c, (orow, oidx)));
                    }
                } else {
                    matrix[r][i].covered = false; // stray continuation
                }
            } else {
                for c in col..col + span {
                    next_active.push((c, (r, i)));
                }
            }
            col += span;
        }
        active = next_active.into_iter().collect();
    }

    // tblHeader is ST_OnOff: an explicit false value is not a header row.
    let header_rows = tbl
        .find_all(ns::W, "tr")
        .take_while(|tr| tr.find(ns::W, "trPr").and_then(|p| on_off(p, "tblHeader")) == Some(true))
        .count();

    let mut builder = GridBuilder::new();
    for row in &matrix {
        builder.next_row();
        for tc in row {
            if tc.covered {
                for _ in 0..tc.col_span {
                    builder.covered();
                }
            } else {
                let mut blocks = match tc.elem {
                    Some(elem) => parse_blocks(elem, ctx)?,
                    None => Vec::new(),
                };
                for merged in &tc.merged {
                    blocks.extend(parse_blocks(merged, ctx)?);
                }
                builder.place(Cell::spanning(blocks, tc.col_span as u32, tc.row_span as u32))?;
            }
        }
    }
    let mut table = builder.finish(TableKind::Data);
    if table.grid.is_empty() {
        return Ok(Vec::new());
    }
    table.header_rows = resolve_header_rows(&table, header_rows);
    Ok(vec![Block::Table(table)])
}

fn collect_row_cells<'e>(parent: &'e Element, cells: &mut Vec<TcInfo<'e>>) {
    for child in parent.child_elems() {
        if child.ns.as_deref().is_none_or(|n| n != ns::W) {
            continue;
        }
        match child.local.as_str() {
            "tc" => {
                let tcpr = child.find(ns::W, "tcPr");
                let covered = tcpr
                    .and_then(|p| p.find(ns::W, "vMerge"))
                    .is_some_and(|v| !matches!(v.attr(ns::W, "val"), Some("restart")));
                let col_span: usize = tcpr
                    .and_then(|p| p.find(ns::W, "gridSpan"))
                    .and_then(|e| e.attr(ns::W, "val"))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1)
                    .clamp(1, 1000);
                // Legacy hMerge: a non-restart hMerge cell folds into the
                // preceding origin, widening its span.
                let hmerge_cont = tcpr
                    .and_then(|p| p.find(ns::W, "hMerge"))
                    .is_some_and(|h| !matches!(h.attr(ns::W, "val"), Some("restart")));
                if hmerge_cont
                    && let Some(prev) = cells.last_mut()
                    && !prev.covered
                {
                    prev.col_span += col_span;
                    prev.merged.push(child);
                    continue;
                }
                cells.push(TcInfo {
                    elem: Some(child),
                    merged: Vec::new(),
                    col_span,
                    row_span: 1,
                    covered,
                });
            }
            "sdt" => {
                if let Some(content) = child.find(ns::W, "sdtContent") {
                    collect_row_cells(content, cells);
                }
            }
            "customXml" => collect_row_cells(child, cells),
            _ => {}
        }
    }
}
