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
use crate::shared::meta;
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
    let document_title = meta::opc_title(&pkg, &root_rels)?;
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
    // Load the layout/master cascade before evaluating the deck-wide guard.
    // A title placeholder with an omitted slide-level type is still a real
    // title after the layout-by-idx resolution from A1.
    for (slide_path, slide_rels) in slide_paths.iter().zip(&all_rels) {
        let layout_path = rel_target_of_type(slide_rels, slide_path, LAYOUT_REL);
        if let Some(lp) = &layout_path
            && !layouts.contains_key(lp)
        {
            let info = load_layout(&pkg, lp)?;
            if let Some(mp) = info.master_path.clone()
                && !masters.contains_key(&mp)
            {
                let master = load_master(&pkg, &mp)?;
                masters.insert(mp, master);
            }
            layouts.insert(lp.clone(), info);
        }
    }
    let mut has_real_title_placeholder = false;
    for (slide_path, slide_rels) in slide_paths.iter().zip(&all_rels) {
        let Some(tree) = pkg.borrow_mut().optional_xml_part(slide_path)? else {
            continue;
        };
        let Some(sp_tree) = tree
            .find(ns::P, "sld")
            .and_then(|s| s.find(ns::P, "cSld"))
            .and_then(|c| c.find(ns::P, "spTree"))
        else {
            continue;
        };
        let layout_path = rel_target_of_type(slide_rels, slide_path, LAYOUT_REL);
        let layout = layout_path.as_ref().and_then(|lp| layouts.get(lp));
        if sp_tree.descendant_elems().filter(|sp| matches!(sp.local.as_str(), "sp" | "cxnSp")).any(
            |sp| {
                resolve_ph_type(sp, layout)
                    .is_some_and(|ph_type| cascade::title_class(&ph_type) == TitleClass::Title)
            },
        ) {
            has_real_title_placeholder = true;
            break;
        }
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
        let slide_block_start = blocks.len();
        if targeted.contains(slide_path)
            && let Some(anchor) = slide_anchors.get(slide_path)
        {
            blocks.push(Block::Paragraph(vec![Inline::Anchor(anchor.clone())]));
        }
        parse_shapes(sp_tree, &ctx, &mut blocks)?;
        let slide_has_heading =
            blocks[slide_block_start..].iter().any(|block| matches!(block, Block::Heading { .. }));
        if !has_real_title_placeholder
            && !slide_has_heading
            && let Some(title) = heuristic_title_text(sp_tree)
        {
            promote_heuristic_heading(&mut blocks[slide_block_start..], &title);
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
    let mut document = Document { blocks, notes: Vec::new(), assets };
    meta::prepend_title(&mut document, document_title.as_deref());
    Ok(document)
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
                Some(TitleClass::Subtitle | TitleClass::Body) => &master.body,
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

/// Resolve a slide shape's effective placeholder type.
///
/// PresentationML says that a slide placeholder without `@type` inherits the
/// type of the layout placeholder with the same `@idx` (ECMA-376 19.3.1.36).
/// A shape without a placeholder is deliberately kept distinct from the
/// default `body` type because it is an ordinary text box, not a body
/// placeholder.
fn resolve_ph_type(sp: &Element, layout: Option<&LayoutInfo>) -> Option<String> {
    let ph = sp.first_descendant(ns::P, "ph")?;
    let ph_type = ph.attr(ns::P, "type").map(str::to_string).or_else(|| {
        let idx = ph.attr(ns::P, "idx")?;
        layout?
            .placeholders
            .iter()
            .find(|placeholder| placeholder.idx.as_deref() == Some(idx))
            .map(|placeholder| placeholder.ph_type.clone())
    });
    Some(ph_type.unwrap_or_else(|| "body".to_string()))
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

/// Return the first text shape only when it is also the topmost text shape
/// and carries one short paragraph. Missing geometry is not guessed.
fn heuristic_title_text(sp_tree: &Element) -> Option<String> {
    let text_shapes: Vec<(&Element, String, Option<i64>)> = sp_tree
        .descendant_elems()
        .filter(|sp| matches!(sp.local.as_str(), "sp" | "cxnSp"))
        .filter_map(|sp| {
            let tx = sp.find(ns::P, "txBody")?;
            let paragraphs: Vec<&Element> = tx.find_all(ns::A, "p").collect();
            let text = clean_text(&paragraphs.iter().map(|p| p.text()).collect::<String>());
            if text.is_empty() {
                return None;
            }
            let y = sp
                .first_descendant(ns::A, "off")
                .and_then(|off| off.attr(ns::A, "y"))
                .and_then(|value| value.parse::<i64>().ok());
            Some((sp, text, y))
        })
        .collect();
    let first = text_shapes.first()?;
    if text_shapes.iter().any(|(_, _, y)| y.is_none()) {
        return None;
    }
    let min_y = text_shapes.iter().filter_map(|(_, _, y)| *y).min()?;
    if first.2 != Some(min_y) || first.1.chars().count() >= 120 {
        return None;
    }
    let tx = first.0.find(ns::P, "txBody")?;
    (tx.find_all(ns::A, "p").count() == 1).then(|| first.1.clone())
}

fn promote_heuristic_heading(blocks: &mut [Block], title: &str) {
    let Some(block) = blocks.iter_mut().find(|block| {
        matches!(block, Block::Paragraph(content) if crate::model::inlines_to_plain_text(content) == title)
    }) else {
        return;
    };
    let Block::Paragraph(content) = std::mem::replace(block, Block::Paragraph(Vec::new())) else {
        return;
    };
    *block = Block::Heading { level: 2, anchor: Some(title.to_string()), content };
}

fn parse_shape(sp: &Element, ctx: &SlideCtx, blocks: &mut Vec<Block>) -> Result<(), ConvertError> {
    let ph = resolve_ph_type(sp, ctx.layout).map(|ph_type| PhInfo {
        ph_type,
        idx: sp
            .first_descendant(ns::P, "ph")
            .and_then(|p| p.attr(ns::P, "idx"))
            .map(str::to_string),
    });
    if let Some(ph) = &ph
        && matches!(ph.ph_type.as_str(), "sldNum" | "dt" | "ftr")
    {
        return Ok(());
    }
    let Some(tx) = sp.find(ns::P, "txBody") else {
        return Ok(());
    };
    // Titles and subtitles get heading semantics but keep their shape-order
    // position. Subtitles use the body cascade, preserving their styling.
    match ph.as_ref().map(|p| cascade::title_class(&p.ph_type)) {
        Some(TitleClass::Title) => push_title_heading(tx, ctx, ph.as_ref(), blocks, 2)?,
        Some(TitleClass::Subtitle) => push_title_heading(tx, ctx, ph.as_ref(), blocks, 3)?,
        _ => parse_text_body(tx, ctx, ph.as_ref(), blocks)?,
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
    level: u8,
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
        blocks.push(Block::Heading { level, anchor, content: inlines });
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
    use crate::package::xml::parse_xml;

    fn element(xml: &str) -> Element {
        parse_xml(xml.as_bytes()).expect("valid test XML")
    }

    fn heuristic_tree(shapes: &str) -> Element {
        element(&format!(
            r#"<p:spTree xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
                xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">{shapes}</p:spTree>"#
        ))
    }

    fn heuristic_shape(id: &str, y: &str, paragraphs: &str) -> String {
        format!(
            r#"<p:sp><p:spPr><a:xfrm><a:off x="0" y="{y}"/></a:xfrm></p:spPr>
                <p:txBody><a:p>{paragraphs}</a:p></p:txBody></p:sp><!-- {id} -->"#
        )
    }

    fn layout(xml: &str) -> LayoutInfo {
        let tree = element(xml);
        LayoutInfo { placeholders: cascade::collect_placeholders(&tree), master_path: None }
    }

    #[test]
    fn placeholder_type_explicit_value_wins_over_layout() {
        let shape = element(
            r#"<p:sp xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
                <p:nvPr><p:ph type="body" idx="0"/></p:nvPr>
            </p:sp>"#,
        );
        let layout = layout(
            r#"<p:spTree xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
                <p:sp><p:nvPr><p:ph type="title" idx="0"/></p:nvPr></p:sp>
            </p:spTree>"#,
        );
        assert_eq!(resolve_ph_type(&shape, Some(&layout)).as_deref(), Some("body"));
    }

    #[test]
    fn placeholder_type_inherits_layout_type_by_idx() {
        let shape = element(
            r#"<p:sp xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
                <p:nvPr><p:ph idx="0"/></p:nvPr>
            </p:sp>"#,
        );
        let layout = layout(
            r#"<p:spTree xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
                <p:sp><p:nvPr><p:ph type="title" idx="0"/></p:nvPr></p:sp>
            </p:spTree>"#,
        );
        assert_eq!(resolve_ph_type(&shape, Some(&layout)).as_deref(), Some("title"));
    }

    #[test]
    fn placeholder_type_falls_back_to_body_for_unknown_idx() {
        let shape = element(
            r#"<p:sp xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
                <p:nvPr><p:ph idx="9"/></p:nvPr>
            </p:sp>"#,
        );
        let layout = layout(
            r#"<p:spTree xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
                <p:sp><p:nvPr><p:ph type="title" idx="0"/></p:nvPr></p:sp>
            </p:spTree>"#,
        );
        assert_eq!(resolve_ph_type(&shape, Some(&layout)).as_deref(), Some("body"));
    }

    #[test]
    fn shape_without_placeholder_stays_unclassified() {
        let shape = element(
            r#"<p:sp xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
                <p:nvPr/>
            </p:sp>"#,
        );
        assert_eq!(resolve_ph_type(&shape, None), None);
    }

    #[test]
    fn heuristic_title_requires_first_shape_to_be_topmost_and_single_short_paragraph() {
        let title = heuristic_shape("title", "100", "<a:r><a:t>Deck title</a:t></a:r>");
        let body = heuristic_shape("body", "500", "<a:r><a:t>Body</a:t></a:r>");
        assert_eq!(
            heuristic_title_text(&heuristic_tree(&format!("{title}{body}"))),
            Some("Deck title".into())
        );

        let lower_first = heuristic_shape("lower", "500", "<a:r><a:t>Deck title</a:t></a:r>");
        let upper_second = heuristic_shape("upper", "100", "<a:r><a:t>Topmost</a:t></a:r>");
        assert_eq!(
            heuristic_title_text(&heuristic_tree(&format!("{lower_first}{upper_second}"))),
            None
        );

        let two_paragraphs = heuristic_shape(
            "two",
            "100",
            "<a:r><a:t>First</a:t></a:r></a:p><a:p><a:r><a:t>Second</a:t></a:r>",
        );
        assert_eq!(heuristic_title_text(&heuristic_tree(&two_paragraphs)), None);

        let long =
            heuristic_shape("long", "100", &format!("<a:r><a:t>{}</a:t></a:r>", "X".repeat(120)));
        assert_eq!(heuristic_title_text(&heuristic_tree(&long)), None);
    }
}
