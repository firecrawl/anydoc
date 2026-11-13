//! DocLang XML emission: laid-out pieces become `<doclang>` body markup.
//!
//! Locations are normalized to the document `default_resolution` (512×512)
//! with the DocLang top-left origin — PDF item coordinates are bottom-left,
//! so y is flipped against the page crop box. Pages carrying a non-zero
//! `/Rotate` skip locations entirely rather than emit wrong boxes.

use super::layout::{BBox, Piece, TextBlock};
use pdf_inspector::TextItem;
use std::collections::HashMap;
use std::fmt::Write;

/// Location axis resolution (`<default_resolution width height>`).
const RES: u32 = 512;

/// Crop-box geometry of a page in PDF space.
pub(crate) struct PageGeom {
    /// Lower-left corner of the crop box.
    pub x0: f32,
    pub y0: f32,
    /// Upper-right corner of the crop box.
    pub x1: f32,
    pub y1: f32,
    /// The page carries a non-zero `/Rotate`; locations are skipped.
    pub rotated: bool,
}

impl PageGeom {
    fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    fn height(&self) -> f32 {
        self.y1 - self.y0
    }
}

/// Emit one page's pieces as DocLang body markup. `srcs` maps each image
/// XObject name to its archive URI. `links` are the page's link-annotation
/// boxes used to attach hrefs to the runs (or pictures) they cover.
pub(crate) fn emit_page(
    out: &mut String,
    pieces: &[Piece],
    geom: &PageGeom,
    links: &[(BBox, String)],
    srcs: &HashMap<String, String>,
) {
    let mut in_fields = false;
    for piece in pieces {
        match piece {
            Piece::Field(item) => {
                if !in_fields {
                    out.push_str("<field_region>");
                    in_fields = true;
                }
                emit_field(out, item, geom);
            }
            Piece::Text(block) => {
                close_fields(out, &mut in_fields);
                emit_block(out, block, geom, links);
            }
            Piece::Picture { item, name, full_page } => {
                close_fields(out, &mut in_fields);
                emit_picture(out, item, name, *full_page, geom, links, srcs);
            }
        }
    }
    close_fields(out, &mut in_fields);
}

fn close_fields(out: &mut String, in_fields: &mut bool) {
    if *in_fields {
        out.push_str("</field_region>");
        *in_fields = false;
    }
}

/// Element head helpers, in schema order.
fn head_locations(out: &mut String, bbox: BBox, geom: &PageGeom) {
    let (w, h) = (geom.width(), geom.height());
    if geom.rotated || w <= 0.0 || h <= 0.0 {
        return;
    }
    let nx = |v: f32| (v / w * RES as f32).round().clamp(0.0, (RES - 1) as f32) as u32;
    let ny = |v: f32| (v / h * RES as f32).round().clamp(0.0, (RES - 1) as f32) as u32;
    // PDF y grows upward; DocLang y grows downward from the page top.
    let x0 = nx(bbox.x0 - geom.x0);
    let y0 = ny(geom.y1 - bbox.y1);
    let x1 = nx(bbox.x1 - geom.x0).max(x0);
    let y1 = ny(geom.y1 - bbox.y0).max(y0);
    for v in [x0, y0, x1, y1] {
        let _ = write!(out, "<location value=\"{v}\"/>");
    }
}

fn head_href(out: &mut String, bbox: BBox, links: &[(BBox, String)]) {
    let cx = (bbox.x0 + bbox.x1) / 2.0;
    let cy = (bbox.y0 + bbox.y1) / 2.0;
    if let Some((_, uri)) = links.iter().find(|(r, _)| r.contains(cx, cy)) {
        let _ = write!(out, "<href uri=\"{}\"/>", esc(uri));
    }
}

fn emit_block(out: &mut String, block: &TextBlock, geom: &PageGeom, links: &[(BBox, String)]) {
    let (tag, attr) = if block.footnote {
        ("footnote", String::new())
    } else if let Some(level) = block.level {
        ("heading", format!(" level=\"{level}\""))
    } else {
        ("text", String::new())
    };
    let _ = write!(out, "<{tag}{attr}>");
    if block.furniture {
        out.push_str("<layer value=\"furniture\"/>");
    }
    head_locations(out, block.bbox, geom);
    emit_runs(out, &block.items, links);
    let _ = write!(out, "</{tag}>");
}

/// A maximal run of same-style, same-link items.
fn emit_runs(out: &mut String, items: &[TextItem], links: &[(BBox, String)]) {
    let mut open: Option<Key> = None;
    let mut prev: Option<&TextItem> = None;
    for item in items {
        if let Some(class) = checkbox_class(&item.text) {
            close_run(out, &mut open);
            let _ = write!(out, "<checkbox class=\"{class}\"/>");
            prev = Some(item);
            continue;
        }
        let key = Key::of(item, links);
        let sep = prev.map_or("", |p| separator(p, item));
        if open.as_ref() == Some(&key) {
            out.push_str(sep);
        } else {
            close_run(out, &mut open);
            out.push_str(sep);
            open_run(out, &key);
            open = Some(key);
        }
        out.push_str(&esc(&item.text));
        prev = Some(item);
    }
    close_run(out, &mut open);
}

/// Style+link identity of a run.
#[derive(PartialEq)]
struct Key {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    code: bool,
    href: Option<String>,
}

impl Key {
    fn of(item: &TextItem, links: &[(BBox, String)]) -> Key {
        let font = item.font.to_ascii_lowercase();
        let code =
            ["courier", "mono", "typewriter", "fixed", "console"].iter().any(|s| font.contains(s));
        let (cx, cy) = (item.x + item.width / 2.0, item.y + item.height / 2.0);
        let href = links.iter().find(|(r, _)| r.contains(cx, cy)).map(|(_, u)| u.clone());
        Key {
            bold: item.is_bold,
            italic: item.is_italic,
            underline: item.is_underline,
            strike: item.is_strikeout,
            code,
            href,
        }
    }
}

/// Whitespace between two consecutive items: a newline across baselines,
/// a space across a real horizontal gap, nothing for intra-word splits.
fn separator(prev: &TextItem, cur: &TextItem) -> &'static str {
    if (cur.y - prev.y).abs() > prev.height.max(cur.height) * 0.4 + 0.5 {
        "\n"
    } else if cur.x - (prev.x + prev.width) > prev.font_size.max(1.0) * 0.15 {
        " "
    } else {
        ""
    }
}

fn open_run(out: &mut String, key: &Key) {
    if let Some(uri) = &key.href {
        let _ = write!(out, "<text><href uri=\"{}\"/>", esc(uri));
    }
    for (flag, tag) in [
        (key.bold, "bold"),
        (key.italic, "italic"),
        (key.underline, "underline"),
        (key.strike, "strikethrough"),
        (key.code, "code"),
    ] {
        if flag {
            let _ = write!(out, "<{tag}>");
        }
    }
}

fn close_run(out: &mut String, open: &mut Option<Key>) {
    if let Some(key) = open.take() {
        for (flag, tag) in [
            (key.code, "code"),
            (key.strike, "strikethrough"),
            (key.underline, "underline"),
            (key.italic, "italic"),
            (key.bold, "bold"),
        ] {
            if flag {
                let _ = write!(out, "</{tag}>");
            }
        }
        if key.href.is_some() {
            out.push_str("</text>");
        }
    }
}

/// The DocLang checkbox class for a lone checkbox glyph, or `None` when the
/// text is ordinary content.
fn checkbox_class(text: &str) -> Option<&'static str> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let all_in = |set: &[char]| t.chars().all(|c| set.contains(&c));
    if t == "[ ]" || all_in(&['☐', '❑', '⬜', '◻', '□', '▢']) {
        Some("unselected")
    } else if t.eq_ignore_ascii_case("[x]")
        || all_in(&['☑', '☒', '❒', '◼', '■', '✓', '✔', '✗', '✘'])
    {
        Some("selected")
    } else {
        None
    }
}

fn emit_field(out: &mut String, item: &TextItem, geom: &PageGeom) {
    out.push_str("<field_item>");
    head_locations(out, BBox::of(item), geom);
    // pdf-inspector reports AcroForm widgets as `name: value`.
    match item.text.split_once(':') {
        Some((key, value)) => {
            let _ = write!(out, "<key>{}:</key>", esc(key.trim_end()));
            emit_value(out, value.trim());
        }
        None => emit_value(out, item.text.trim()),
    }
    out.push_str("</field_item>");
}

fn emit_value(out: &mut String, value: &str) {
    if value.is_empty() {
        // An unfilled widget is a fillable blank.
        out.push_str("<value class=\"fillable\"/>");
    } else {
        let _ = write!(out, "<value>{}</value>", esc(value));
    }
}

fn emit_picture(
    out: &mut String,
    item: &TextItem,
    name: &str,
    full_page: bool,
    geom: &PageGeom,
    links: &[(BBox, String)],
    srcs: &HashMap<String, String>,
) {
    let bbox = BBox::of(item);
    out.push_str("<picture>");
    if full_page {
        out.push_str("<label value=\"full_page_image\"/>");
    }
    head_href(out, bbox, links);
    head_locations(out, bbox, geom);
    if let Some(uri) = srcs.get(name) {
        let _ = write!(out, "<src uri=\"{}\"/>", esc(uri));
    }
    out.push_str("</picture>");
}

/// Document-level inputs for the `<head>` metadata block.
pub(crate) struct DocMeta {
    /// PDF title metadata, when present.
    pub title: Option<String>,
    /// Lowercase hex SHA-256 of the source bytes.
    pub sha256: String,
    /// Crop-box size of each page in PDF points `(width, height)`.
    pub page_sizes: Vec<(f32, f32)>,
}

/// Wrap per-page body markup in the `<doclang>` root and `<head>`.
/// `bodies[i]` is the markup of page `i + 1`; `page_break` goes between.
pub(crate) fn emit_document(meta: &DocMeta, bodies: &[String]) -> String {
    let mut out = String::with_capacity(512 + bodies.iter().map(String::len).sum::<usize>());
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>");
    out.push_str("<doclang xmlns=\"https://www.doclang.ai/ns/v0\" version=\"0.9\">");
    out.push_str("<head>");
    if let Some(title) = meta.title.as_deref().filter(|t| !t.trim().is_empty()) {
        let _ = write!(out, "<title>{}</title>", esc(title.trim()));
    }
    let _ =
        write!(out, "<document_hash hash_function=\"sha256sum\">{}</document_hash>", meta.sha256);
    let _ = write!(out, "<generated_by>anydoc {}</generated_by>", env!("CARGO_PKG_VERSION"));
    let _ = write!(out, "<default_resolution width=\"{RES}\" height=\"{RES}\"/>");
    let uniform =
        meta.page_sizes.first().is_some_and(|first| meta.page_sizes.iter().all(|s| s == first));
    for (i, &(w, h)) in meta.page_sizes.iter().enumerate() {
        if uniform && i > 0 {
            break;
        }
        let page_no = if uniform { String::new() } else { format!(" page_no=\"{}\"", i + 1) };
        let _ =
            write!(out, "<page_size{page_no} width=\"{}\" height=\"{}\"/>", fmt_num(w), fmt_num(h));
    }
    out.push_str("</head>");
    for (i, body) in bodies.iter().enumerate() {
        if i > 0 {
            out.push_str("<page_break/>");
        }
        out.push_str(body);
    }
    out.push_str("</doclang>");
    out
}

/// Render a coordinate compactly (drop a redundant `.0`).
fn fmt_num(v: f32) -> String {
    if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{v:.2}") }
}

/// XML-escape text or an attribute value (quotes escaped unconditionally;
/// harmless in text content).
pub(crate) fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom() -> PageGeom {
        PageGeom { x0: 0.0, y0: 0.0, x1: 612.0, y1: 792.0, rotated: false }
    }

    #[test]
    fn escaping_covers_text_and_attributes() {
        assert_eq!(esc("a<b>&\"c\""), "a&lt;b&gt;&amp;&quot;c&quot;");
    }

    #[test]
    fn locations_flip_to_top_left_origin() {
        // A box at the page's lower-left maps to small x, large y.
        let mut out = String::new();
        head_locations(&mut out, BBox { x0: 0.0, y0: 0.0, x1: 100.0, y1: 100.0 }, &geom());
        assert_eq!(
            out,
            "<location value=\"0\"/><location value=\"447\"/>\
             <location value=\"84\"/><location value=\"511\"/>"
        );
    }

    #[test]
    fn rotated_pages_skip_locations() {
        let mut out = String::new();
        let g = PageGeom { rotated: true, ..geom() };
        head_locations(&mut out, BBox { x0: 0.0, y0: 0.0, x1: 10.0, y1: 10.0 }, &g);
        assert!(out.is_empty());
    }

    #[test]
    fn checkbox_glyphs_classify() {
        assert_eq!(checkbox_class("☐"), Some("unselected"));
        assert_eq!(checkbox_class("[ ]"), Some("unselected"));
        assert_eq!(checkbox_class("☒"), Some("selected"));
        assert_eq!(checkbox_class("[x]"), Some("selected"));
        assert_eq!(checkbox_class("plain text"), None);
    }
}
