//! Textual extraction of DrawingML rich objects shared by DOCX and PPTX:
//! charts (title, axis titles, cached series data) and SmartArt diagram
//! data (text points).

use crate::model::{Block, Cell, Inline, List, ListItem, Style, Table, TableKind};
use crate::package::xml::{Element, ns};
use crate::shared::text::clean_text;

/// A chart part as blocks: bold title paragraph plus a categories x series
/// table built from the cached display strings.
pub fn chart_blocks(root: &Element) -> Vec<Block> {
    let mut blocks = Vec::new();
    let title = root
        .first_descendant(ns::CHART, "title")
        .map(|t| clean_text(&drawing_text(t)))
        .filter(|t| !t.trim().is_empty());
    if let Some(title) = title {
        blocks.push(Block::Paragraph(vec![Inline::Text {
            text: title,
            style: Style { bold: true, ..Style::PLAIN },
        }]));
    }
    struct Series {
        name: String,
        values: Vec<String>,
    }
    let mut categories: Vec<String> = Vec::new();
    let mut series: Vec<Series> = Vec::new();
    for ser in root.descendants(ns::CHART, "ser") {
        // Cached display strings only; `c:f` formula references are not text.
        let name = ser
            .find(ns::CHART, "tx")
            .and_then(|t| t.first_descendant(ns::CHART, "v"))
            .map(|v| clean_text(&v.text()))
            .unwrap_or_default();
        let cats: Vec<String> = ser
            .find(ns::CHART, "cat")
            .map(|c| c.descendants(ns::CHART, "v").map(|v| clean_text(&v.text())).collect())
            .unwrap_or_default();
        if categories.is_empty() {
            categories = cats;
        }
        let values: Vec<String> = ser
            .find(ns::CHART, "val")
            .map(|v| v.descendants(ns::CHART, "v").map(|p| clean_text(&p.text())).collect())
            .unwrap_or_default();
        series.push(Series { name, values });
    }
    if !series.is_empty() && !categories.is_empty() {
        let cat_title = root
            .first_descendant(ns::CHART, "catAx")
            .and_then(|ax| ax.find(ns::CHART, "title"))
            .map(|t| clean_text(&drawing_text(t)))
            .unwrap_or_default();
        let mut header: Vec<Cell> = vec![Cell::from_inlines(vec![Inline::plain(cat_title)])];
        header
            .extend(series.iter().map(|s| Cell::from_inlines(vec![Inline::plain(s.name.clone())])));
        let mut rows = vec![header];
        for (i, cat) in categories.iter().enumerate() {
            let mut row = vec![Cell::from_inlines(vec![Inline::plain(cat.clone())])];
            for s in &series {
                let v = s.values.get(i).cloned().unwrap_or_default();
                row.push(Cell::from_inlines(vec![Inline::plain(v)]));
            }
            rows.push(row);
        }
        blocks.push(Block::Table(Table::from_rows(rows, 1, TableKind::Data)));
    }
    blocks
}

/// A SmartArt data part as a bullet list of its text points in order.
pub fn diagram_blocks(root: &Element) -> Vec<Block> {
    let items: Vec<ListItem> = root
        .descendants(ns::DGM, "pt")
        .filter_map(|pt| {
            let text = clean_text(&pt.find(ns::DGM, "t")?.text());
            if text.trim().is_empty() {
                return None;
            }
            Some(ListItem {
                blocks: vec![Block::Paragraph(vec![Inline::plain(text)])],
                marker_label: None,
            })
        })
        .collect();
    if items.is_empty() {
        return Vec::new();
    }
    vec![Block::List(List { marker: crate::model::MarkerKind::Bullet, start: 1, items })]
}

/// Text runs inside DrawingML rich text (`a:p`/`a:r`/`a:t`), joined.
pub fn drawing_text(elem: &Element) -> String {
    let mut parts: Vec<String> = Vec::new();
    for p in elem.descendants(ns::A, "p") {
        let text = p.text();
        if !text.trim().is_empty() {
            parts.push(text);
        }
    }
    if parts.is_empty() { elem.text() } else { parts.join(" ") }
}
