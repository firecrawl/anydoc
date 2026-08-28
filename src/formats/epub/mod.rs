//! EPUB: XHTML chapters in spine order, concatenated into one document with
//! chapter-scoped anchors so intra-book navigation survives.

use crate::error::ConvertError;
use crate::model::{AnchorId, Block, Document, ImageSource, Inline, LinkTarget};
use crate::package::xml::Element;
use crate::package::{Package, path};
use crate::shared::assets::{AssetSink, media_type_for};
use crate::shared::html::{HtmlCtx, Stylesheet};
use crate::shared::uri::is_absolute_uri;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

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

    let mut doc = Document::default();
    if let Some(title) = opf.descendants_any("title").next().map(|t| t.text()) {
        let title = title.trim().to_string();
        if !title.is_empty() {
            doc.blocks.push(Block::heading(1, vec![Inline::plain(title)]));
        }
    }

    let mut manifest: HashMap<String, (String, String)> = HashMap::new();
    for item in opf.descendants_any("item") {
        if let (Some(id), Some(href)) = (item.attr_any("id"), item.attr_any("href")) {
            let media = item.attr_any("media-type").unwrap_or("").to_string();
            manifest.insert(id.to_string(), (href.to_string(), media));
        }
    }

    // Every spine part in spine order: non-linear items are auxiliary but
    // still publication content, and unusable parts degrade at parse time.
    // Intra-book links target these; links to any other resource stay
    // Relative.
    // A part holds one position in a reading order, and each repeat would
    // cost another parse of it and another copy of its anchor.
    let mut spine_entries = 0usize;
    let mut spine_paths: Vec<String> = Vec::new();
    let mut spine_parts: HashSet<String> = HashSet::new();
    for href in opf
        .descendants_any("itemref")
        .filter_map(|ir| ir.attr_any("idref"))
        .filter_map(|idref| manifest.get(idref))
        .map(|(href, _)| href.as_str())
    {
        spine_entries += 1;
        let target = match path::resolve(&opf_path, href) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("skipping chapter with unresolvable href {href:?}: {e}");
                continue;
            }
        };
        if spine_parts.insert(target.path.clone()) {
            spine_paths.push(target.path);
        }
    }

    let assets = RefCell::new(AssetSink::new());
    let mut css_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut converted = 0usize;
    for chapter_path in &spine_paths {
        let Some(tree) = pkg.borrow_mut().optional_xml_part(chapter_path)? else {
            log::warn!("skipping unusable chapter {chapter_path}");
            continue;
        };
        let Some(body) = tree
            .child_elems()
            .find(|e| e.local == "html")
            .and_then(|h| h.child_elems().find(|e| e.local == "body"))
        else {
            log::warn!("skipping chapter {chapter_path}: no body element");
            continue;
        };
        let css = chapter_stylesheet(&tree, chapter_path, &pkg, &mut css_cache)?;
        let ctx = ChapterCtx {
            pkg: &pkg,
            assets: &assets,
            chapter_path: chapter_path.clone(),
            spine_parts: &spine_parts,
        };
        // Chapter-start anchor: renders only when a link targets this chapter.
        doc.blocks.push(Block::Paragraph(vec![Inline::Anchor(chapter_path.clone())]));
        doc.blocks.extend(crate::shared::html::to_blocks(body, &css, &ctx)?);
        converted += 1;
    }
    if spine_entries > 0 && converted == 0 {
        return Err(ConvertError::malformed("no chapter in the book could be read"));
    }

    doc.assets = std::mem::take(&mut assets.borrow_mut().assets);
    Ok(doc)
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
            let fragment = path::decode_fragment(fragment);
            return Some(LinkTarget::Anchor(scoped(&self.chapter_path, Some(&fragment))));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    #[test]
    fn a_part_repeated_across_the_spine_is_read_once() {
        let items: String = (0..64)
            .map(|i| {
                format!(r#"<item id="i{i}" href="ch.xhtml" media-type="application/xhtml+xml"/>"#)
            })
            .collect();
        let refs: String = (0..64).map(|i| format!(r#"<itemref idref="i{i}"/>"#)).collect();
        let parts = [
            (
                "META-INF/container.xml",
                r#"<?xml version="1.0"?>
                <container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
                <rootfiles><rootfile full-path="c.opf"
                media-type="application/oebps-package+xml"/></rootfiles></container>"#
                    .to_string(),
            ),
            (
                "c.opf",
                format!(
                    r#"<?xml version="1.0"?>
                    <package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata/>
                    <manifest>{items}</manifest><spine>{refs}</spine></package>"#
                ),
            ),
            (
                "ch.xhtml",
                r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml">
                <body><p>chapter text</p></body></html>"#
                    .to_string(),
            ),
        ];
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in &parts {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }

        let doc = parse(&w.finish().unwrap().into_inner()).unwrap();
        let text = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(inlines) => Some(crate::model::inlines_to_plain_text(inlines)),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(text, "chapter text", "sixty-four itemrefs naming one part");
    }
}
