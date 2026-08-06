//! OOXML PresentationML (.pptx / .pptm / .ppsx): slides in `sldIdLst` order,
//! with the full text cascade - slide -> layout -> master placeholder /
//! `txStyles` -> presentation defaults. Speaker notes are included (fixed
//! policy), rendered as a quote after each slide's content.

mod cascade;

use crate::error::ConvertError;
use crate::model::{
    Block, Cell, Document, GridBuilder, ImageSource, Inline, LinkTarget, Style, TableKind,
    inlines_are_empty,
};
use crate::package::relationships::{
    RelTarget, Relationships, TargetMode, read_rels, rel_target_bytes, rel_type, rels_part_for,
};
use crate::package::xml::{Element, ns, parse_xml};
use crate::package::{Package, archive::probe_ole, path};
use crate::shared::assets::AssetSink;
use crate::shared::delta::rebase_emphasis;
use crate::shared::fields::classify_rel_target;
use crate::shared::header::resolve_header_rows;
use crate::shared::list::{ListEntry, ListKey, MarkerKind, flush_list};
use crate::shared::text::clean_text;
use cascade::{Bullet, LevelStyle, Placeholder, TextProps, TitleClass};
use std::cell::{Cell as StdCell, RefCell};
use std::collections::HashMap;

const LAYOUT_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout";
const MASTER_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster";
const NOTES_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide";
const SLIDE_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";

/// Namespaces whose markup this frontend understands; `mc:Choice` branches
/// requiring anything else fall back to `mc:Fallback`.
const SUPPORTED_NS: &[&str] = &[ns::P, ns::A, ns::R, ns::MC];

struct LayoutInfo {
    placeholders: Vec<Placeholder>,
    master_path: Option<String>,
}

struct MasterInfo {
    title: LevelStyle,
    body: LevelStyle,
    other: LevelStyle,
    placeholders: Vec<Placeholder>,
}

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let pkg = match Package::open(bytes) {
        Ok(p) => p,
        Err(e) => return Err(probe_ole(bytes).unwrap_or(e)),
    };
    let pkg = RefCell::new(pkg);

    // OPC part discovery: the presentation part comes from the package-level
    // officeDocument relationship, with the conventional path as fallback.
    let root_rels = read_rels(&mut pkg.borrow_mut(), "_rels/.rels")?;
    let pres_part = root_rels
        .first_of_type(rel_type::OFFICE_DOCUMENT)
        .and_then(|rel| path::resolve("", &rel.target).ok())
        .map(|t| t.path)
        .unwrap_or_else(|| "ppt/presentation.xml".to_string());
    let pres = pkg.borrow_mut().required_xml_part(&pres_part)?;
    let pres_rels = read_rels(&mut pkg.borrow_mut(), &rels_part_for(&pres_part))?;

    let default_text =
        cascade::parse_level_styles(pres.first_descendant(ns::P, "defaultTextStyle"));

    let slide_paths: Vec<String> = pres
        .first_descendant(ns::P, "sldIdLst")
        .map(|list| {
            list.find_all(ns::P, "sldId")
                .filter_map(|s| s.attr_qualified(ns::R, "id"))
                .filter_map(|rid| pres_rels.internal_target(rid))
                .filter_map(|target| path::resolve(&pres_part, target).ok().map(|t| t.path))
                .collect()
        })
        .unwrap_or_default();
    if slide_paths.is_empty() {
        return Err(ConvertError::malformed_part(
            pres_part.clone(),
            "presentation has no slide list",
        ));
    }

    let assets = RefCell::new(AssetSink::new());
    let mut layouts: HashMap<String, LayoutInfo> = HashMap::new();
    let mut masters: HashMap<String, MasterInfo> = HashMap::new();
    let mut blocks: Vec<Block> = Vec::new();
    let mut failed = 0usize;
    let instance_counter = StdCell::new(0u64);
    // Every slide has a start anchor id so internal slide-to-slide links
    // resolve after concatenation; the anchor node is emitted only on
    // slides some link actually targets.
    let slide_anchors: HashMap<String, String> = slide_paths
        .iter()
        .enumerate()
        .map(|(i, p)| (p.clone(), format!("slide-{}", i + 1)))
        .collect();
    let mut all_rels: Vec<Relationships> = Vec::with_capacity(slide_paths.len());
    for p in &slide_paths {
        all_rels.push(read_rels(&mut pkg.borrow_mut(), &rels_part_for(p))?);
    }
    let targeted: std::collections::HashSet<String> = slide_paths
        .iter()
        .zip(&all_rels)
        .flat_map(|(p, rels)| {
            rels.iter()
                .filter(|(_, r)| r.rel_type == SLIDE_REL && r.mode == TargetMode::Internal)
                .filter_map(move |(_, r)| path::resolve(p, &r.target).ok().map(|t| t.path))
        })
        .filter(|t| slide_anchors.contains_key(t))
        .collect();

    for (slide_index, slide_path) in slide_paths.iter().enumerate() {
        let tree = match pkg.borrow_mut().optional_xml_part(slide_path)? {
            Some(t) => t,
            None => {
                log::warn!("skipping unusable slide {slide_path}");
                failed += 1;
                continue;
            }
        };
        let Some(sp_tree) = tree
            .find(ns::P, "sld")
            .and_then(|s| s.find(ns::P, "cSld"))
            .and_then(|c| c.find(ns::P, "spTree"))
        else {
            log::warn!("skipping slide {slide_path}: no shape tree");
            failed += 1;
            continue;
        };
        let slide_rels = &all_rels[slide_index];

        let layout_path = rel_target_of_type(slide_rels, slide_path, LAYOUT_REL);
        if let Some(lp) = &layout_path
            && !layouts.contains_key(lp)
        {
            let info = load_layout(&pkg, lp)?;
            if let Some(mp) = info.master_path.clone()
                && !masters.contains_key(&mp)
            {
                let m = load_master(&pkg, &mp)?;
                masters.insert(mp, m);
            }
            layouts.insert(lp.clone(), info);
        }
        let layout = layout_path.as_ref().and_then(|lp| layouts.get(lp));
        let master = layout.and_then(|l| l.master_path.as_ref()).and_then(|mp| masters.get(mp));

        let ctx = SlideCtx {
            pkg: &pkg,
            rels: slide_rels,
            base_part: slide_path,
            assets: &assets,
            default_text: &default_text,
            layout,
            master,
            instance_counter: &instance_counter,
            slide_anchors: &slide_anchors,
        };
        // A slide is a container, not a page: its content must not run into
        // the next slide's. A thematic break is the structural separator the
        // model already has, and it is emitted BETWEEN slides only — never
        // leading, never doubled by an empty slide.
        let mut slide_blocks: Vec<Block> = Vec::new();
        if targeted.contains(slide_path)
            && let Some(anchor) = slide_anchors.get(slide_path)
        {
            slide_blocks.push(Block::Paragraph(vec![Inline::Anchor(anchor.clone())]));
        }
        parse_shapes(sp_tree, &ctx, &mut slide_blocks)?;
        if !slide_blocks.is_empty() {
            if !blocks.is_empty() {
                blocks.push(Block::Rule);
            }
            blocks.append(&mut slide_blocks);
        }

        // Speaker notes, set off as a quote (fixed policy: included). The
        // tree is loaded before the `if let` so the package borrow is not
        // held across the body.
        let notes = match rel_target_of_type(slide_rels, slide_path, NOTES_REL) {
            Some(notes_path) => {
                let tree = pkg.borrow_mut().optional_xml_part(&notes_path)?;
                tree.map(|t| (notes_path, t))
            }
            None => None,
        };
        if let Some((notes_path, notes_tree)) = notes {
            let notes_rels = read_rels(&mut pkg.borrow_mut(), &rels_part_for(&notes_path))?;
            let notes_ctx = SlideCtx {
                rels: &notes_rels,
                base_part: &notes_path,
                layout: None,
                master: None,
                ..ctx
            };
            let mut notes_blocks = Vec::new();
            for sp in notes_tree.descendants(ns::P, "sp") {
                // Keep note text bodies (real producers use a body
                // placeholder; LibreOffice writes plain text boxes) but skip
                // the slide-image and chrome placeholders.
                if matches!(placeholder_type(sp), Some("sldImg" | "sldNum" | "hdr" | "ftr" | "dt"))
                {
                    continue;
                }
                if let Some(tx) = sp.find(ns::P, "txBody") {
                    parse_text_body(tx, &notes_ctx, None, &mut notes_blocks)?;
                }
            }
            if !notes_blocks.is_empty() {
                blocks.push(Block::BlockQuote(notes_blocks));
            }
        }
    }
    if failed == slide_paths.len() {
        return Err(ConvertError::malformed("no slide in the presentation could be read"));
    }

    let assets = std::mem::take(&mut assets.borrow_mut().assets);
    Ok(Document { blocks, notes: Vec::new(), assets })
}

fn rel_target_of_type(rels: &Relationships, base: &str, rel_type: &str) -> Option<String> {
    // first_of_type picks the lowest id, so duplicate relationships resolve
    // deterministically.
    rels.first_of_type(rel_type)
        .and_then(|rel| path::resolve(base, &rel.target).ok())
        .map(|t| t.path)
}

fn load_layout(pkg: &RefCell<Package>, layout_path: &str) -> Result<LayoutInfo, ConvertError> {
    let placeholders = match pkg.borrow_mut().optional_xml_part(layout_path)? {
        Some(tree) => tree
            .first_descendant(ns::P, "spTree")
            .map(cascade::collect_placeholders)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    let rels = read_rels(&mut pkg.borrow_mut(), &rels_part_for(layout_path))?;
    let master_path = rel_target_of_type(&rels, layout_path, MASTER_REL);
    Ok(LayoutInfo { placeholders, master_path })
}

fn load_master(pkg: &RefCell<Package>, master_path: &str) -> Result<MasterInfo, ConvertError> {
    let mut info = MasterInfo {
        title: LevelStyle::default(),
        body: LevelStyle::default(),
        other: LevelStyle::default(),
        placeholders: Vec::new(),
    };
    if let Some(tree) = pkg.borrow_mut().optional_xml_part(master_path)? {
        if let Some(tx_styles) = tree.first_descendant(ns::P, "txStyles") {
            info.title = cascade::parse_level_styles(tx_styles.find(ns::P, "titleStyle"));
            info.body = cascade::parse_level_styles(tx_styles.find(ns::P, "bodyStyle"));
            info.other = cascade::parse_level_styles(tx_styles.find(ns::P, "otherStyle"));
        }
        if let Some(sp_tree) = tree.first_descendant(ns::P, "spTree") {
            info.placeholders = cascade::collect_placeholders(sp_tree);
        }
    }
    Ok(info)
}

struct SlideCtx<'a, 'b> {
    pkg: &'b RefCell<Package<'a>>,
    rels: &'b Relationships,
    base_part: &'b str,
    assets: &'b RefCell<AssetSink>,
    default_text: &'b LevelStyle,
    layout: Option<&'b LayoutInfo>,
    master: Option<&'b MasterInfo>,
    /// Per-text-body list instance ids, unique document-wide.
    instance_counter: &'b StdCell<u64>,
    /// Slide part path -> the slide's start anchor id, for internal
    /// slide-to-slide links.
    slide_anchors: &'b HashMap<String, String>,
}

impl SlideCtx<'_, '_> {
    /// Slide-to-slide relationships become anchor links to the target
    /// slide's start anchor; everything else classifies as usual.
    fn link_target(&self, rel: &crate::package::relationships::Relationship) -> LinkTarget {
        if rel.mode == TargetMode::Internal
            && let Ok(t) = path::resolve(self.base_part, &rel.target)
            && let Some(anchor) = self.slide_anchors.get(&t.path)
        {
            return LinkTarget::Anchor(anchor.clone());
        }
        classify_rel_target(rel.mode == TargetMode::External, &rel.target)
    }

    /// Fold the cascade for a paragraph, outermost first.
    fn base_props(&self, ph: Option<&PhInfo>, shape_styles: &LevelStyle, lvl: usize) -> TextProps {
        let mut props = self.default_text.level(lvl);
        if let Some(master) = self.master {
            let class_style = match ph.map(|p| cascade::title_class(&p.ph_type)) {
                Some(TitleClass::Title) => &master.title,
                Some(TitleClass::Body) => &master.body,
                None => &master.other,
            };
            props = props.merge(class_style.level(lvl));
            if let Some(ph) = ph
                && let Some(hit) =
                    cascade::match_placeholder(&master.placeholders, &ph.ph_type, ph.idx.as_deref())
            {
                props = props.merge(hit.styles.level(lvl));
            }
        }
        if let Some(layout) = self.layout
            && let Some(ph) = ph
            && let Some(hit) =
                cascade::match_placeholder(&layout.placeholders, &ph.ph_type, ph.idx.as_deref())
        {
            props = props.merge(hit.styles.level(lvl));
        }
        props.merge(shape_styles.level(lvl))
    }

    /// Load an internal relationship target's bytes, resolved against this
    /// part. Failures degrade (log + `None`) per the unified policy;
    /// resource-limit errors always propagate.
    fn rel_part(&self, rel_id: &str) -> Result<Option<RelTarget>, ConvertError> {
        rel_target_bytes(self.pkg, self.rels, self.base_part, rel_id)
    }
}

struct PhInfo {
    ph_type: String,
    idx: Option<String>,
}

fn placeholder_type(sp: &Element) -> Option<&str> {
    sp.first_descendant(ns::P, "ph").map(|ph| ph.attr(ns::P, "type").unwrap_or("body"))
}

fn parse_shapes(
    parent: &Element,
    ctx: &SlideCtx,
    blocks: &mut Vec<Block>,
) -> Result<(), ConvertError> {
    for child in parent.child_elems() {
        if child.is(ns::MC, "AlternateContent") {
            if let Some(branch) = crate::shared::mc::alternate_branch(child, SUPPORTED_NS) {
                parse_shapes(branch, ctx, blocks)?;
            }
            continue;
        }
        if child.ns.as_deref().is_none_or(|n| n != ns::P) {
            continue;
        }
        match child.local.as_str() {
            // Connector shapes carry the same `p:txBody` as plain shapes.
            "sp" | "cxnSp" => parse_shape(child, ctx, blocks)?,
            "grpSp" => parse_shapes(child, ctx, blocks)?,
            "graphicFrame" => parse_graphic_frame(child, ctx, blocks)?,
            "pic" => {
                let descr = child
                    .first_descendant(ns::P, "cNvPr")
                    .and_then(|p| p.attr(ns::P, "descr"))
                    .map(clean_text)
                    .unwrap_or_default();
                let rid = child.first_descendant(ns::A, "blip").and_then(|b| {
                    b.attr_qualified(ns::R, "embed").or_else(|| b.attr_qualified(ns::R, "link"))
                });
                // External-mode targets (`r:link`) become external image
                // sources; embedded targets are retained as assets.
                let source = match rid {
                    Some(rid) => crate::shared::assets::rel_image_source(
                        ctx.pkg,
                        ctx.rels,
                        ctx.base_part,
                        ctx.assets,
                        rid,
                    )?,
                    None => None,
                };
                if source.is_some() || !descr.trim().is_empty() {
                    blocks.push(Block::Paragraph(vec![Inline::Image {
                        alt: descr,
                        source: source.unwrap_or(ImageSource::Unavailable),
                    }]));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_shape(sp: &Element, ctx: &SlideCtx, blocks: &mut Vec<Block>) -> Result<(), ConvertError> {
    let ph = sp.first_descendant(ns::P, "ph").map(|p| PhInfo {
        ph_type: p.attr(ns::P, "type").unwrap_or("body").to_string(),
        idx: p.attr(ns::P, "idx").map(str::to_string),
    });
    if let Some(ph) = &ph
        && matches!(ph.ph_type.as_str(), "sldNum" | "dt" | "ftr")
    {
        return Ok(());
    }
    let Some(tx) = sp.find(ns::P, "txBody") else {
        return Ok(());
    };
    // Titles get heading semantics but keep their shape-order position.
    if ph.as_ref().is_some_and(|p| cascade::title_class(&p.ph_type) == TitleClass::Title) {
        push_title_heading(tx, ctx, ph.as_ref(), blocks)?;
    } else {
        parse_text_body(tx, ctx, ph.as_ref(), blocks)?;
    }
    Ok(())
}

/// Collapse a title placeholder's paragraphs into one slide heading. Runs
/// resolve through the full cascade like body text.
fn push_title_heading(
    tx: &Element,
    ctx: &SlideCtx,
    ph: Option<&PhInfo>,
    blocks: &mut Vec<Block>,
) -> Result<(), ConvertError> {
    let shape_styles = cascade::parse_level_styles(tx.find(ns::A, "lstStyle"));
    let mut inlines: Vec<Inline> = Vec::new();
    for p in tx.find_all(ns::A, "p") {
        let ppr = p.find(ns::A, "pPr");
        let lvl: usize =
            ppr.and_then(|pr| pr.attr(ns::A, "lvl")).and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut props = ctx.base_props(ph, &shape_styles, lvl);
        if let Some(ppr) = ppr {
            props = props.merge(cascade::paragraph_props(ppr));
        }
        let base = props.delta.resolve();
        let mut para = parse_para_inlines(p, ctx, base);
        if inlines_are_empty(&para) {
            continue;
        }
        rebase_emphasis(&mut para, base);
        if !inlines.is_empty() {
            inlines.push(Inline::LineBreak);
        }
        inlines.extend(para);
    }
    if !inlines_are_empty(&inlines) {
        let anchor = Some(crate::model::inlines_to_plain_text(&inlines));
        blocks.push(Block::Heading { level: 2, anchor, content: inlines });
    }
    Ok(())
}

fn parse_text_body(
    tx: &Element,
    ctx: &SlideCtx,
    ph: Option<&PhInfo>,
    blocks: &mut Vec<Block>,
) -> Result<(), ConvertError> {
    let shape_styles = cascade::parse_level_styles(tx.find(ns::A, "lstStyle"));
    let instance = {
        let id = ctx.instance_counter.get() + 1;
        ctx.instance_counter.set(id);
        id
    };
    let mut counters = [0u64; cascade::LEVELS];
    let mut started = [false; cascade::LEVELS];
    let mut list_run: Vec<ListEntry> = Vec::new();

    for p in tx.find_all(ns::A, "p") {
        let ppr = p.find(ns::A, "pPr");
        let lvl: usize =
            ppr.and_then(|pr| pr.attr(ns::A, "lvl")).and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut props = ctx.base_props(ph, &shape_styles, lvl);
        if let Some(ppr) = ppr {
            props = props.merge(cascade::paragraph_props(ppr));
        }
        let base = props.delta.resolve();
        let inlines = parse_para_inlines(p, ctx, base);
        if inlines_are_empty(&inlines) {
            flush_list(blocks, &mut list_run);
            continue;
        }
        match props.bullet {
            Bullet::AutoNum { marker, start, wrap } => {
                let lvl_idx = lvl.min(cascade::LEVELS - 1);
                let number = if started[lvl_idx] {
                    counters[lvl_idx].saturating_add(1)
                } else {
                    started[lvl_idx] = true;
                    start
                };
                counters[lvl_idx] = number;
                for deeper in started.iter_mut().skip(lvl_idx + 1) {
                    *deeper = false;
                }
                list_run.push(ListEntry {
                    level: lvl,
                    key: ListKey { instance, marker },
                    number,
                    label: wrap.label(marker, number),
                    blocks: vec![Block::Paragraph(inlines)],
                });
            }
            Bullet::Char => list_run.push(ListEntry {
                level: lvl,
                key: ListKey { instance, marker: MarkerKind::Bullet },
                number: 0,
                label: None,
                blocks: vec![Block::Paragraph(inlines)],
            }),
            Bullet::None | Bullet::Inherit => {
                flush_list(blocks, &mut list_run);
                blocks.push(Block::Paragraph(inlines));
            }
        }
    }
    flush_list(blocks, &mut list_run);
    Ok(())
}

fn parse_para_inlines(p: &Element, ctx: &SlideCtx, base: Style) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    for child in p.child_elems() {
        if child.ns.as_deref().is_none_or(|n| n != ns::A) {
            continue;
        }
        match child.local.as_str() {
            "r" | "fld" => {
                let rpr = child.find(ns::A, "rPr");
                let text =
                    clean_text(&child.find(ns::A, "t").map(|t| t.text()).unwrap_or_default());
                if text.is_empty() {
                    continue;
                }
                let style = match rpr {
                    Some(rpr) => cascade::rpr_delta(rpr).apply(base),
                    None => base,
                };
                let inline = Inline::Text { text, style };
                let target = rpr
                    .and_then(|r| r.find(ns::A, "hlinkClick"))
                    .and_then(|h| h.attr_qualified(ns::R, "id"))
                    .and_then(|rid| ctx.rels.get(rid))
                    .map(|rel| ctx.link_target(rel));
                match target {
                    Some(target) => out.push(Inline::Link { content: vec![inline], target }),
                    None => out.push(inline),
                }
            }
            "br" => out.push(Inline::LineBreak),
            _ => {}
        }
    }
    out
}

fn parse_graphic_frame(
    frame: &Element,
    ctx: &SlideCtx,
    blocks: &mut Vec<Block>,
) -> Result<(), ConvertError> {
    if let Some(tbl) = frame.first_descendant(ns::A, "tbl") {
        parse_table(tbl, ctx, blocks)?;
        return Ok(());
    }
    // Embedded OLE objects: retain identity, media type, and payload.
    if let Some(ole) = frame.first_descendant(ns::P, "oleObj") {
        let prog_id = ole.attr(ns::P, "progId").unwrap_or("object");
        let name = ole.attr(ns::P, "name").unwrap_or("").trim();
        let alt =
            if name.is_empty() { format!("Embedded object: {prog_id}") } else { name.to_string() };
        let source = match ole.attr_qualified(ns::R, "id") {
            Some(rid) => match ctx.rel_part(rid)? {
                Some((part, bytes)) => Some(ImageSource::Asset(ctx.assets.borrow_mut().add(
                    "application/vnd.ms-ole-object".into(),
                    part,
                    &bytes,
                )?)),
                None => None,
            },
            None => None,
        };
        blocks.push(Block::Paragraph(vec![Inline::Image {
            alt,
            source: source.unwrap_or(ImageSource::Unavailable),
        }]));
        return Ok(());
    }
    if let Some(chart_ref) = frame.first_descendant(ns::CHART, "chart")
        && let Some(rid) = chart_ref.attr_qualified(ns::R, "id")
        && let Some((part, bytes)) = ctx.rel_part(rid)?
    {
        match parse_xml(&bytes) {
            Ok(root) => blocks.extend(crate::shared::drawingml::chart_blocks(&root)),
            Err(e) if e.is_fatal() => return Err(e),
            Err(e) => log::warn!("skipping corrupt chart part {part}: {e}"),
        }
        return Ok(());
    }
    if let Some(rel_ids) = frame.first_descendant(ns::DGM, "relIds")
        && let Some(rid) = rel_ids.attr_qualified(ns::R, "dm")
        && let Some((part, bytes)) = ctx.rel_part(rid)?
    {
        match parse_xml(&bytes) {
            Ok(root) => blocks.extend(crate::shared::drawingml::diagram_blocks(&root)),
            Err(e) if e.is_fatal() => return Err(e),
            Err(e) => log::warn!("skipping corrupt diagram part {part}: {e}"),
        }
    }
    Ok(())
}

/// DrawingML slide table: origins carry `gridSpan`/`rowSpan`; merged
/// continuation cells (`hMerge`/`vMerge`) consume covered positions.
fn parse_table(tbl: &Element, ctx: &SlideCtx, blocks: &mut Vec<Block>) -> Result<(), ConvertError> {
    let header_rows = usize::from(
        tbl.find(ns::A, "tblPr")
            .is_some_and(|p| matches!(p.attr(ns::A, "firstRow"), Some("1") | Some("true"))),
    );
    let mut builder = GridBuilder::new();
    for tr in tbl.find_all(ns::A, "tr") {
        builder.next_row();
        for tc in tr.find_all(ns::A, "tc") {
            let merged = matches!(tc.attr(ns::A, "hMerge"), Some("1") | Some("true"))
                || matches!(tc.attr(ns::A, "vMerge"), Some("1") | Some("true"));
            if merged {
                builder.covered();
                continue;
            }
            let col_span: u32 =
                tc.attr(ns::A, "gridSpan").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
            let row_span: u32 =
                tc.attr(ns::A, "rowSpan").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
            let mut cell_blocks = Vec::new();
            if let Some(tx) = tc.find(ns::A, "txBody") {
                parse_text_body(tx, ctx, None, &mut cell_blocks)?;
            }
            builder.place(Cell::spanning(cell_blocks, col_span, row_span))?;
        }
    }
    let mut table = builder.finish(TableKind::Data);
    if table.grid.is_empty() {
        return Ok(());
    }
    table.header_rows = resolve_header_rows(&table, header_rows);
    blocks.push(Block::Table(table));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const PRES_NS: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
    const DRAWING_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
    const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    const PKG_REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

    /// A slide holding a single text box with `text`, or no shapes at all when
    /// `text` is `None` — the "contributes nothing" case the separator must not
    /// leave a dangling break for.
    fn slide_xml(text: Option<&str>) -> String {
        let body = match text {
            Some(t) => format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="TextBox"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:txBody><a:bodyPr/><a:p><a:r><a:t>{t}</a:t></a:r></a:p></p:txBody></p:sp>"#
            ),
            None => String::new(),
        };
        format!(
            r#"<?xml version="1.0"?><p:sld xmlns:p="{PRES_NS}" xmlns:a="{DRAWING_NS}"><p:cSld><p:spTree>{body}</p:spTree></p:cSld></p:sld>"#
        )
    }

    /// Minimal .pptx whose slides carry the given text bodies, in order. No
    /// layout or master parts: every slide is title-less, which is exactly the
    /// shape whose boundary used to vanish.
    fn pptx_with_slides(slides: &[Option<&str>]) -> Vec<u8> {
        let ids: String = (0..slides.len())
            .map(|i| format!(r#"<p:sldId id="{}" r:id="rId{}"/>"#, 256 + i, i + 1))
            .collect();
        let rels: String = (0..slides.len())
            .map(|i| {
                format!(
                    r#"<Relationship Id="rId{}" Type="{REL_NS}/slide" Target="slides/slide{}.xml"/>"#,
                    i + 1,
                    i + 1
                )
            })
            .collect();

        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        let mut put = |name: &str, body: &str| {
            w.start_file(name, opts).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        };
        put(
            "_rels/.rels",
            &format!(
                r#"<?xml version="1.0"?><Relationships xmlns="{PKG_REL_NS}"><Relationship Id="rIdP" Type="{REL_NS}/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#
            ),
        );
        put(
            "ppt/presentation.xml",
            &format!(
                r#"<?xml version="1.0"?><p:presentation xmlns:p="{PRES_NS}" xmlns:r="{REL_NS}"><p:sldIdLst>{ids}</p:sldIdLst></p:presentation>"#
            ),
        );
        put(
            "ppt/_rels/presentation.xml.rels",
            &format!(
                r#"<?xml version="1.0"?><Relationships xmlns="{PKG_REL_NS}">{rels}</Relationships>"#
            ),
        );
        for (i, text) in slides.iter().enumerate() {
            put(&format!("ppt/slides/slide{}.xml", i + 1), &slide_xml(*text));
        }
        w.finish().unwrap().into_inner()
    }

    fn blocks(slides: &[Option<&str>]) -> Vec<Block> {
        parse(&pptx_with_slides(slides)).expect("minimal pptx should convert").blocks
    }

    #[test]
    fn untitled_slides_are_separated() {
        // The defect: with no title placeholder neither slide contributes a
        // heading, so the two paragraphs used to be indistinguishable from two
        // paragraphs of one slide.
        let out = blocks(&[Some("first"), Some("second")]);
        assert!(
            matches!(out.as_slice(), [Block::Paragraph(_), Block::Rule, Block::Paragraph(_)]),
            "expected a separator between the two slides, got {out:?}"
        );
    }

    #[test]
    fn separator_never_leads() {
        let out = blocks(&[Some("only")]);
        assert!(
            !matches!(out.first(), Some(Block::Rule)),
            "a deck must not open with a separator, got {out:?}"
        );
        assert_eq!(out.len(), 1, "single slide should yield exactly its own content: {out:?}");
    }

    #[test]
    fn empty_slides_leave_no_dangling_separator() {
        // An empty slide contributes no blocks, so it must not push a break of
        // its own — neither doubled between two real slides nor trailing.
        let out = blocks(&[Some("first"), None, Some("second")]);
        let rules = out.iter().filter(|b| matches!(b, Block::Rule)).count();
        assert_eq!(rules, 1, "empty slide must not add a separator, got {out:?}");

        let trailing = blocks(&[Some("first"), None]);
        assert!(
            !matches!(trailing.last(), Some(Block::Rule)),
            "a trailing empty slide must not leave a dangling separator, got {trailing:?}"
        );
    }
}
