//! Spatial layout for DocLang export: flat extracted items are grouped
//! into lines and merged into paragraph-like blocks in reading order, and
//! each block picks up its semantic kind from the PDF structure tree (or a
//! conservative font-size heuristic when the document is untagged).
//!
//! Item coordinates stay in PDF space (bottom-left origin) here; the
//! markup stage normalizes them into DocLang locations.

use pdf_inspector::TextItem;
use pdf_inspector::types::ItemType;
use std::collections::HashMap;

/// A bounding box in PDF space: `[x0, y0, x1, y1]`, bottom-left origin.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl BBox {
    /// The item's box (x/y are the PDF lower-left origin already).
    pub fn of(item: &TextItem) -> BBox {
        BBox { x0: item.x, y0: item.y, x1: item.x + item.width, y1: item.y + item.height }
    }

    fn union(&mut self, other: BBox) {
        self.x0 = self.x0.min(other.x0);
        self.y0 = self.y0.min(other.y0);
        self.x1 = self.x1.max(other.x1);
        self.y1 = self.y1.max(other.y1);
    }

    /// True when `point` falls inside the box.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }
}

/// One laid-out element of a page, in reading order.
pub(crate) enum Piece {
    /// A block of merged text lines, emitted as `<text>`, `<heading>`, or
    /// `<footnote>`.
    Text(TextBlock),
    /// An AcroForm field item (text carries `name: value`), emitted as a
    /// `<field_item>`; consecutive fields nest under one `<field_region>`.
    Field(TextItem),
    /// An image placeholder item, emitted as `<picture>`. `name` is the
    /// XObject resource name pdf-inspector records in `[Image: name]`.
    /// `full_page` is set by the caller once the page context is known
    /// (dominant image → `full_page_image` label + `pages/` part).
    Picture { item: TextItem, name: String, full_page: bool },
}

impl Piece {
    pub fn bbox(&self) -> BBox {
        match self {
            Piece::Text(b) => b.bbox,
            Piece::Field(i) | Piece::Picture { item: i, .. } => BBox::of(i),
        }
    }
}

/// A paragraph-like block of same-role lines.
pub(crate) struct TextBlock {
    /// Items in reading order (lines top-down, items left-right).
    pub items: Vec<TextItem>,
    /// Heading level when the block classifies as a heading.
    pub level: Option<u8>,
    /// Emitted as `<footnote>` instead of `<text>`.
    pub footnote: bool,
    /// Supplementary content (page numbers, artifacts): `<layer
    /// value="furniture"/>`.
    pub furniture: bool,
    /// The block's union box.
    pub bbox: BBox,
}

/// One extracted line (items sharing a baseline).
struct Line {
    items: Vec<TextItem>,
    bbox: BBox,
}

/// Structure-tree role lookup keyed by `(page, mcid)`.
pub(crate) type Roles = HashMap<(u32, i64), String>;

/// The link-annotation boxes of a page: `(bbox, uri)`.
pub(crate) fn link_rects(items: &[TextItem]) -> Vec<(BBox, String)> {
    items
        .iter()
        .filter_map(|i| match &i.item_type {
            ItemType::Link(uri) => Some((BBox::of(i), uri.clone())),
            _ => None,
        })
        .collect()
}

/// Lay out one page's items into pieces in reading order. `tagged` tells
/// whether the document carries a structure tree at all; untagged pages get
/// the font-size heading heuristic.
pub(crate) fn layout(items: Vec<TextItem>, roles: &Roles, tagged: bool) -> Vec<Piece> {
    let median = median_font(&items);
    let mut textish: Vec<TextItem> = Vec::new();
    let mut pieces: Vec<Piece> = Vec::new();
    for item in items {
        match &item.item_type {
            ItemType::Image => {
                let name = item
                    .text
                    .strip_prefix("[Image: ")
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or("")
                    .to_string();
                pieces.push(Piece::Picture { item, name, full_page: false });
            }
            ItemType::FormField => pieces.push(Piece::Field(item)),
            // Link items describe an annotation rectangle over existing
            // text; the markup stage uses them to attach hrefs.
            ItemType::Link(_) => {}
            ItemType::Text => textish.push(item),
        }
    }
    for line in lines(textish) {
        merge_line(&mut pieces, line, roles, tagged, median);
    }
    pieces.sort_by(|a, b| {
        let (a, b) = (a.bbox(), b.bbox());
        b.y1.partial_cmp(&a.y1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal))
    });
    pieces
}

/// Group items into lines by baseline proximity, each line sorted
/// left-to-right, lines produced top-down.
fn lines(mut items: Vec<TextItem>) -> Vec<Line> {
    items.sort_by(|a, b| {
        b.y.partial_cmp(&a.y)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut lines: Vec<Line> = Vec::new();
    for item in items {
        let bbox = BBox::of(&item);
        let fits = lines.last_mut().is_some_and(|line| {
            let base = line.items.first().map(|i| i.y).unwrap_or(item.y);
            (item.y - base).abs() <= 0.5 * item.height.max(line_height(line)).max(1.0)
        });
        if fits {
            let line = lines.last_mut().unwrap();
            line.bbox.union(bbox);
            line.items.push(item);
        } else {
            lines.push(Line { items: vec![item], bbox });
        }
    }
    for line in &mut lines {
        line.items.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
    }
    lines
}

fn line_height(line: &Line) -> f32 {
    line.items.iter().map(|i| i.height).fold(0.0_f32, f32::max)
}

/// Merge a line into the trailing text block when it reads as a
/// continuation, else start a new piece.
fn merge_line(pieces: &mut Vec<Piece>, line: Line, roles: &Roles, tagged: bool, median: f32) {
    let cls = classify(&line.items, roles, tagged, median);
    if let Some(Piece::Text(block)) = pieces.last_mut() {
        let gap = block.bbox.y0 - line.bbox.y1;
        let same_kind =
            block.level == cls.level && block.footnote == cls.footnote && !cls.furniture;
        if same_kind && gap >= -line_height(&line) && gap <= 1.5 * line_height(&line) {
            block.bbox.union(line.bbox);
            block.items.extend(line.items);
            return;
        }
    }
    pieces.push(Piece::Text(TextBlock {
        items: line.items,
        level: cls.level,
        footnote: cls.footnote,
        furniture: cls.furniture,
        bbox: line.bbox,
    }));
}

struct Classified {
    level: Option<u8>,
    footnote: bool,
    furniture: bool,
}

/// Semantic kind for a line: structure-tree roles win; when the document
/// is untagged a font-size heuristic catches display headings.
fn classify(items: &[TextItem], roles: &Roles, tagged: bool, median: f32) -> Classified {
    let mut level = None;
    let mut footnote = false;
    let mut furniture = false;
    for item in items {
        let Some(role) = item.mcid.and_then(|m| roles.get(&(item.page, m))) else {
            continue;
        };
        match role.as_str() {
            "Title" => level = Some(1),
            r if r.starts_with('H') => {
                if let Ok(n) = r[1..].parse::<u8>() {
                    let n = n.clamp(1, 6);
                    level = Some(level.map_or(n, |l: u8| l.min(n)));
                }
            }
            "Note" => footnote = true,
            "Artifact" => furniture = true,
            _ => {}
        }
    }
    if level.is_none() && !footnote && !furniture && !tagged {
        level = heuristic_heading(items, median);
    }
    Classified { level, footnote, furniture }
}

/// Untagged-document fallback: a line set markedly larger than the page's
/// median text is probably a heading.
fn heuristic_heading(items: &[TextItem], median: f32) -> Option<u8> {
    let size = items.iter().map(|i| i.font_size).fold(0.0_f32, f32::max);
    let ratio = size / median.max(1.0);
    if ratio >= 1.8 {
        Some(1)
    } else if ratio >= 1.4 {
        Some(2)
    } else {
        None
    }
}

/// Median font size of real text items on a page (heading heuristic base).
fn median_font(items: &[TextItem]) -> f32 {
    let mut sizes: Vec<f32> = items
        .iter()
        .filter(|i| i.font_size > 0.5 && matches!(i.item_type, ItemType::Text))
        .map(|i| i.font_size)
        .collect();
    if sizes.is_empty() {
        return 12.0;
    }
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sizes[sizes.len() / 2]
}
