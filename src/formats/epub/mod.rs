//! EPUB: XHTML chapters in spine order, concatenated into one document with
//! chapter-scoped anchors so intra-book navigation survives.

use crate::error::ConvertError;
use crate::model::{AnchorId, Block, Document, ImageSource, Inline, LinkTarget};
use crate::package::limits;
use crate::package::xml::Element;
use crate::package::{Package, path};
use crate::shared::assets::{AssetSink, media_type_for};
use crate::shared::html::{HtmlCtx, Stylesheet};
use crate::shared::meta;
use crate::shared::uri::is_absolute_uri;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

#[derive(Debug)]
struct ManifestItem {
    href: String,
    properties: String,
}

#[derive(Debug)]
struct TocEntry {
    path: String,
    fragment: Option<String>,
    depth: usize,
    label: String,
}

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let pkg = RefCell::new(Package::open(bytes)?);

    let container = pkg.borrow_mut().required_xml_part("META-INF/container.xml")?;
    let opf_path = container
        .descendants_any("rootfile")
        .next()
        .and_then(|r| r.attr_any("full-path"))
        .ok_or_else(|| ConvertError::malformed_part("META-INF/container.xml", "no rootfile entry"))?
        .to_string();

    let opf = pkg.borrow_mut().required_xml_part(&opf_path)?;

    let document_title = opf.descendants_any("title").next().map(|t| t.text());
    let mut doc = Document::default();

    let mut manifest: HashMap<String, ManifestItem> = HashMap::new();
    for item in opf.descendants_any("item") {
        if let (Some(id), Some(href)) = (item.attr_any("id"), item.attr_any("href")) {
            let properties = item.attr_any("properties").unwrap_or("").to_string();
            manifest.insert(id.to_string(), ManifestItem { href: href.to_string(), properties });
        }
    }

    let toc = load_toc(&pkg, &opf, &opf_path, &manifest)?;

    // Every spine part in spine order: non-linear items are auxiliary but
    // still publication content, and unusable parts degrade at parse time.
    // Intra-book links target these; links to any other resource stay
    // Relative.
    let spine_hrefs: Vec<&str> = opf
        .descendants_any("itemref")
        .filter_map(|ir| ir.attr_any("idref"))
        .filter_map(|idref| manifest.get(idref))
        .map(|item| item.href.as_str())
        .collect();
    let spine_parts: HashSet<String> = spine_hrefs
        .iter()
        .filter_map(|href| path::resolve(&opf_path, href).ok().map(|t| t.path))
        .collect();

    let assets = RefCell::new(AssetSink::new());
    let mut css_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut failed = 0usize;
    for href in &spine_hrefs {
        let chapter_path = match path::resolve(&opf_path, href) {
            Ok(t) => t.path,
            Err(e) => {
                log::warn!("skipping chapter with unresolvable href {href:?}: {e}");
                failed += 1;
                continue;
            }
        };
        let Some(tree) = pkg.borrow_mut().optional_xml_part(&chapter_path)? else {
            log::warn!("skipping unusable chapter {chapter_path}");
            failed += 1;
            continue;
        };
        let Some(body) = tree
            .child_elems()
            .find(|e| e.local == "html")
            .and_then(|h| h.child_elems().find(|e| e.local == "body"))
        else {
            log::warn!("skipping chapter {chapter_path}: no body element");
            failed += 1;
            continue;
        };
        let css = chapter_stylesheet(&tree, &chapter_path, &pkg, &mut css_cache)?;
        let ctx = ChapterCtx {
            pkg: &pkg,
            assets: &assets,
            chapter_path: chapter_path.clone(),
            spine_parts: &spine_parts,
        };
        // Chapter-start anchor: renders only when a link targets this chapter.
        let mut chapter_blocks = crate::shared::html::to_blocks(body, &css, &ctx)?;
        if let Some(toc) = &toc {
            apply_toc(&mut chapter_blocks, &chapter_path, toc);
        }
        doc.blocks.push(Block::Paragraph(vec![Inline::Anchor(chapter_path.clone())]));
        doc.blocks.extend(chapter_blocks);
    }
    if !spine_hrefs.is_empty() && failed == spine_hrefs.len() {
        return Err(ConvertError::malformed("no chapter in the book could be read"));
    }

    meta::prepend_title(&mut doc, document_title.as_deref());
    doc.assets = std::mem::take(&mut assets.borrow_mut().assets);
    Ok(doc)
}

fn load_toc(
    pkg: &RefCell<Package<'_>>,
    opf: &Element,
    opf_path: &str,
    manifest: &HashMap<String, ManifestItem>,
) -> Result<Option<Vec<TocEntry>>, ConvertError> {
    if let Some(nav_item) = manifest.values().find(|item| has_token(&item.properties, "nav")) {
        match path::resolve(opf_path, &nav_item.href) {
            Ok(target) => {
                if let Some(tree) = pkg.borrow_mut().optional_xml_part(&target.path)?
                    && let Some(nav) = tree.descendants_any("nav").find(|nav| {
                        has_token(nav.attr_any("type").unwrap_or(""), "toc")
                            || has_token(nav.attr_any("epub:type").unwrap_or(""), "toc")
                    })
                    && let Some(list) = nav.descendants_any("ol").next()
                {
                    let mut entries = Vec::new();
                    walk_nav_list(list, 1, &mut entries, &target.path)?;
                    if !entries.is_empty() {
                        return Ok(Some(entries));
                    }
                }
            }
            Err(error) => log::debug!(
                "ignoring unresolvable EPUB navigation href {:?}: {error}",
                nav_item.href
            ),
        }
    }

    let Some(spine) = opf.descendants_any("spine").next() else {
        return Ok(None);
    };
    let Some(ncx_id) = spine.attr_any("toc") else {
        return Ok(None);
    };
    let Some(ncx_item) = manifest.get(ncx_id) else {
        log::debug!("EPUB spine references missing NCX manifest item {ncx_id:?}");
        return Ok(None);
    };
    let Ok(target) = path::resolve(opf_path, &ncx_item.href) else {
        log::debug!("ignoring unresolvable EPUB NCX href {:?}", ncx_item.href);
        return Ok(None);
    };
    let Some(tree) = pkg.borrow_mut().optional_xml_part(&target.path)? else {
        return Ok(None);
    };
    let Some(nav_map) = tree.descendants_any("navMap").next() else {
        return Ok(None);
    };
    let mut entries = Vec::new();
    walk_ncx_points(nav_map, 1, &mut entries, &target.path)?;
    Ok((!entries.is_empty()).then_some(entries))
}

fn has_token(value: &str, token: &str) -> bool {
    value.split_whitespace().any(|part| part.eq_ignore_ascii_case(token))
}

fn walk_nav_list(
    list: &Element,
    depth: usize,
    entries: &mut Vec<TocEntry>,
    base_path: &str,
) -> Result<(), ConvertError> {
    check_toc_depth(depth)?;
    for li in list.child_elems().filter(|element| element.local == "li") {
        if let Some(link) = li.child_elems().find(|element| element.local == "a")
            && let Some(href) = link.attr_any("href")
        {
            add_toc_entry(entries, base_path, href, depth, link.text());
        }
        for nested in li.child_elems().filter(|element| element.local == "ol") {
            walk_nav_list(nested, depth + 1, entries, base_path)?;
        }
    }
    Ok(())
}

fn walk_ncx_points(
    nav_map: &Element,
    depth: usize,
    entries: &mut Vec<TocEntry>,
    base_path: &str,
) -> Result<(), ConvertError> {
    check_toc_depth(depth)?;
    for point in nav_map.child_elems().filter(|element| element.local == "navPoint") {
        let href = point
            .child_elems()
            .find(|element| element.local == "content")
            .and_then(|content| content.attr_any("src"));
        if let Some(href) = href {
            let label = point
                .child_elems()
                .find(|element| element.local == "navLabel")
                .map(Element::text)
                .unwrap_or_default();
            add_toc_entry(entries, base_path, href, depth, label);
        }
        walk_ncx_points(point, depth + 1, entries, base_path)?;
    }
    Ok(())
}

fn check_toc_depth(depth: usize) -> Result<(), ConvertError> {
    if depth > limits::MAX_XML_DEPTH {
        return Err(ConvertError::ResourceLimit {
            limit: "max_xml_depth",
            detail: format!("EPUB table of contents nesting exceeds {}", limits::MAX_XML_DEPTH),
        });
    }
    Ok(())
}

fn add_toc_entry(
    entries: &mut Vec<TocEntry>,
    base_path: &str,
    href: &str,
    depth: usize,
    label: String,
) {
    let Ok(target) = path::resolve(base_path, href.trim()) else {
        log::debug!("ignoring unresolvable EPUB table-of-contents target {href:?}");
        return;
    };
    entries.push(TocEntry {
        path: target.path,
        fragment: target.fragment,
        depth,
        label: label.trim().to_string(),
    });
}

fn apply_toc(blocks: &mut Vec<Block>, chapter_path: &str, entries: &[TocEntry]) {
    let covered: Vec<&TocEntry> =
        entries.iter().filter(|entry| entry.path == chapter_path).collect();
    if covered.is_empty() {
        return;
    }

    let base = covered.iter().find(|entry| entry.fragment.is_none()).copied();
    let mut pending_anchors = Vec::new();
    let mut has_heading = false;
    for block in blocks.iter_mut() {
        match block {
            Block::Paragraph(inlines) => {
                pending_anchors.extend(inlines.iter().filter_map(|inline| match inline {
                    Inline::Anchor(anchor) => Some(anchor.as_str()),
                    _ => None,
                }));
            }
            Block::Heading { level, anchor, .. } => {
                has_heading = true;
                let entry = anchor
                    .as_deref()
                    .and_then(|heading_anchor| exact_toc_entry(covered.as_slice(), heading_anchor))
                    .or_else(|| {
                        pending_anchors.iter().find_map(|heading_anchor| {
                            exact_toc_entry(covered.as_slice(), heading_anchor)
                        })
                    })
                    .or(base);
                if let Some(entry) = entry {
                    *level = shifted_heading_level(*level, entry.depth);
                }
                pending_anchors.clear();
            }
            _ => pending_anchors.clear(),
        }
    }

    if !has_heading && let Some(entry) = covered.iter().find(|entry| !entry.label.is_empty()) {
        blocks
            .insert(0, Block::heading(entry.depth.min(6) as u8, vec![Inline::plain(&entry.label)]));
    }
}

fn exact_toc_entry<'a>(entries: &[&'a TocEntry], anchor: &str) -> Option<&'a TocEntry> {
    entries
        .iter()
        .find(|entry| {
            entry
                .fragment
                .as_deref()
                .is_some_and(|fragment| anchor == format!("{}#{}", entry.path, fragment))
        })
        .copied()
}

fn shifted_heading_level(level: u8, depth: usize) -> u8 {
    level.saturating_add(depth.saturating_sub(1).min(6) as u8).min(6)
}

/// A chapter's CSS cascade: its linked stylesheets and inline `<style>`
/// blocks, in document order. Stylesheet parts are cached across chapters.
fn chapter_stylesheet(
    tree: &Element,
    chapter_path: &str,
    pkg: &RefCell<Package>,
    cache: &mut HashMap<String, Option<String>>,
) -> Result<Stylesheet, ConvertError> {
    let mut css = Stylesheet::default();
    let mut stack: Vec<&Element> = tree.child_elems().collect();
    stack.reverse();
    while let Some(elem) = stack.pop() {
        match elem.local.as_str() {
            "link" => {
                let rel = elem.attr_any("rel").unwrap_or("");
                let is_sheet = rel.split_whitespace().any(|r| r.eq_ignore_ascii_case("stylesheet"));
                if is_sheet && let Some(href) = elem.attr_any("href") {
                    let Ok(target) = path::resolve(chapter_path, href) else {
                        continue;
                    };
                    if !cache.contains_key(&target.path) {
                        let text = pkg
                            .borrow_mut()
                            .optional_part(&target.path)?
                            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
                        cache.insert(target.path.clone(), text);
                    }
                    if let Some(Some(text)) = cache.get(&target.path) {
                        css.add(text);
                    }
                }
            }
            "style" => css.add(&elem.text()),
            _ => {
                let start = stack.len();
                stack.extend(elem.child_elems());
                stack[start..].reverse();
            }
        }
    }
    Ok(css)
}

struct ChapterCtx<'a, 'b> {
    pkg: &'b RefCell<Package<'a>>,
    assets: &'b RefCell<AssetSink>,
    chapter_path: String,
    spine_parts: &'b HashSet<String>,
}

impl HtmlCtx for ChapterCtx<'_, '_> {
    fn link_target(&self, href: &str) -> Option<LinkTarget> {
        if href.is_empty() {
            return None;
        }
        if let Some(fragment) = href.strip_prefix('#') {
            return Some(LinkTarget::Anchor(scoped(&self.chapter_path, Some(fragment))));
        }
        if is_absolute_uri(href) {
            return Some(LinkTarget::External(href.to_string()));
        }
        // Anchors only for converted spine documents; links to any other
        // package resource (images, downloads, non-linear content) keep
        // their relative form.
        match path::resolve(&self.chapter_path, href) {
            Ok(target) if self.spine_parts.contains(&target.path) => {
                Some(LinkTarget::Anchor(scoped(&target.path, target.fragment.as_deref())))
            }
            _ => Some(LinkTarget::Relative(href.to_string())),
        }
    }

    fn image_source(&self, src: &str) -> Result<Option<ImageSource>, ConvertError> {
        if src.is_empty() {
            return Ok(None);
        }
        if is_absolute_uri(src) {
            return Ok(Some(ImageSource::External(src.to_string())));
        }
        let Ok(target) = path::resolve(&self.chapter_path, src) else {
            return Ok(None);
        };
        match self.pkg.borrow_mut().optional_part(&target.path)? {
            Some(bytes) => {
                let media = media_type_for(&target.path);
                let id = self.assets.borrow_mut().add(media, target.path, &bytes)?;
                Ok(Some(ImageSource::Asset(id)))
            }
            None => Ok(None),
        }
    }

    fn anchor_id(&self, raw: &str) -> AnchorId {
        scoped(&self.chapter_path, Some(raw))
    }
}

/// Chapter-scoped anchor id: the chapter path itself targets the chapter
/// start; `path#fragment` targets an element inside it.
fn scoped(chapter_path: &str, fragment: Option<&str>) -> AnchorId {
    match fragment {
        Some(f) if !f.is_empty() => format!("{chapter_path}#{f}"),
        _ => chapter_path.to_string(),
    }
}
