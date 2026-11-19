//! Textual extraction of DrawingML rich objects shared by DOCX and PPTX:
//! charts (title, axis titles, cached series data) and SmartArt diagram
//! data (text points).

use std::collections::{BTreeMap, HashMap};

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
        values: HashMap<usize, String>,
    }
    let mut categories = BTreeMap::new();
    let mut higher_labels = Vec::new();
    let mut has_category_cache = false;
    let mut series: Vec<Series> = Vec::new();
    for ser in root.descendants(ns::CHART, "ser") {
        // Cached display strings only; `c:f` formula references are not text.
        let name = ser
            .find(ns::CHART, "tx")
            .and_then(|t| t.first_descendant(ns::CHART, "v"))
            .map(|v| clean_text(&v.text()))
            .unwrap_or_default();
        if categories.is_empty()
            && higher_labels.is_empty()
            && let Some(cat) = ser.find(ns::CHART, "cat")
        {
            if let Some(cache) = cat.first_descendant(ns::CHART, "multiLvlStrCache") {
                let mut levels = cache.find_all(ns::CHART, "lvl");
                if let Some(leaf) = levels.next() {
                    categories = chart_points(leaf).collect();
                    // Keep higher-level labels in the existing flattened view,
                    // but only leaf indices identify numeric data rows.
                    higher_labels = levels.flat_map(chart_points).map(|(_, label)| label).collect();
                    has_category_cache = true;
                }
            } else if let Some(cache) = ["strCache", "numCache", "strLit", "numLit"]
                .into_iter()
                .find_map(|kind| cat.first_descendant(ns::CHART, kind))
            {
                categories = chart_points(cache).collect();
                has_category_cache = true;
            }
        }
        let values = ser.find(ns::CHART, "val").into_iter().flat_map(chart_points).collect();
        series.push(Series { name, values });
    }
    // An empty category label can be omitted while its numeric points remain.
    // Iterate observed indices, not the potentially much larger declared count.
    if has_category_cache {
        for s in &series {
            for &index in s.values.keys() {
                categories.entry(index).or_default();
            }
        }
    }
    if !series.is_empty() && (!categories.is_empty() || !higher_labels.is_empty()) {
        let cat_title = root
            .first_descendant(ns::CHART, "catAx")
            .and_then(|ax| ax.find(ns::CHART, "title"))
            .map(|t| clean_text(&drawing_text(t)))
            .unwrap_or_default();
        let mut header: Vec<Cell> = vec![Cell::from_inlines(vec![Inline::plain(cat_title)])];
        header
            .extend(series.iter().map(|s| Cell::from_inlines(vec![Inline::plain(s.name.clone())])));
        let mut rows = vec![header];
        let indexed_labels = categories.into_iter().map(|(index, label)| (Some(index), label));
        let structural_labels = higher_labels.into_iter().map(|label| (None, label));
        for (index, label) in indexed_labels.chain(structural_labels) {
            let mut row = vec![Cell::from_inlines(vec![Inline::plain(label)])];
            for s in &series {
                let v = index.and_then(|i| s.values.get(&i)).cloned().unwrap_or_default();
                row.push(Cell::from_inlines(vec![Inline::plain(v)]));
            }
            rows.push(row);
        }
        blocks.push(Block::Table(Table::from_rows(rows, 1, TableKind::Data)));
    }
    blocks
}

fn chart_points(container: &Element) -> impl Iterator<Item = (usize, String)> + '_ {
    container.descendants(ns::CHART, "pt").filter_map(|point| {
        let index = point.attr(ns::CHART, "idx")?.parse().ok()?;
        let value = point.find(ns::CHART, "v")?;
        Some((index, clean_text(&value.text())))
    })
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

#[cfg(test)]
mod tests {
    use super::chart_blocks;
    use crate::model::Document;
    use crate::package::xml::parse_xml;
    use crate::render::markdown::document_to_markdown;

    #[test]
    fn chart_keeps_sparse_values_with_their_indexed_category() {
        let chart = parse_xml(
            br#"<c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">
            <c:plotArea><c:barChart><c:ser>
              <c:tx><c:v>Actual</c:v></c:tx>
              <c:cat><c:strLit><c:ptCount val="3"/>
                <c:pt idx="0"><c:v>Week 1</c:v></c:pt>
                <c:pt idx="1"><c:v>Week 2</c:v></c:pt>
                <c:pt idx="2"><c:v>Week 3</c:v></c:pt>
              </c:strLit></c:cat>
              <c:val><c:numLit><c:ptCount val="3"/>
                <c:pt idx="0"><c:v>0</c:v></c:pt>
                <c:pt idx="2"><c:v>27</c:v></c:pt>
              </c:numLit></c:val>
            </c:ser><c:ser>
              <c:tx><c:v>Target</c:v></c:tx>
              <c:val><c:numLit><c:ptCount val="3"/>
                <c:pt idx="0"><c:v>10</c:v></c:pt>
                <c:pt idx="1"><c:v>20</c:v></c:pt>
                <c:pt idx="2"><c:v>30</c:v></c:pt>
              </c:numLit></c:val>
            </c:ser></c:barChart></c:plotArea>
            </c:chart>"#,
        )
        .unwrap();
        let doc = Document { blocks: chart_blocks(&chart), notes: Vec::new(), assets: Vec::new() };
        let markdown = document_to_markdown(&doc);
        assert!(markdown.contains("| Week 1 | 0 | 10 |"), "{markdown}");
        assert!(markdown.contains("| Week 2 |  | 20 |"), "{markdown}");
        assert!(markdown.contains("| Week 3 | 27 | 30 |"), "{markdown}");
    }

    #[test]
    fn chart_keeps_values_without_category_labels() {
        let points = r#"<c:pt idx="0"><c:v>Week 1</c:v></c:pt>
                        <c:pt idx="2"><c:v>Week 3</c:v></c:pt>"#;
        let flat = format!(r#"<c:strLit><c:ptCount val="3"/>{points}</c:strLit>"#);
        let multi = format!(
            r#"<c:multiLvlStrRef><c:multiLvlStrCache><c:ptCount val="3"/>
                        <c:lvl>{points}</c:lvl>
                        <c:lvl><c:pt idx="0"><c:v>Term 1</c:v></c:pt></c:lvl>
                        </c:multiLvlStrCache></c:multiLvlStrRef>"#
        );
        for (cache, higher_label) in [(flat, None), (multi, Some("Term 1"))] {
            let xml = format!(
                r#"<c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">
                <c:plotArea><c:barChart><c:ser>
                  <c:tx><c:v>Actual</c:v></c:tx>
                  <c:cat>{cache}</c:cat>
                  <c:val><c:numLit><c:ptCount val="3"/>
                    <c:pt idx="0"><c:v>12</c:v></c:pt>
                    <c:pt idx="1"><c:v>20</c:v></c:pt>
                    <c:pt idx="2"><c:v>27</c:v></c:pt>
                  </c:numLit></c:val>
                </c:ser></c:barChart></c:plotArea>
                </c:chart>"#,
            );
            let chart = parse_xml(xml.as_bytes()).unwrap();
            let doc =
                Document { blocks: chart_blocks(&chart), notes: Vec::new(), assets: Vec::new() };
            let markdown = document_to_markdown(&doc);
            assert!(markdown.contains("| Week 1 | 12 |"), "{markdown}");
            assert!(markdown.contains("|  | 20 |"), "{markdown}");
            assert!(markdown.contains("| Week 3 | 27 |"), "{markdown}");
            if let Some(label) = higher_label {
                assert!(markdown.contains(&format!("| {label} |  |")), "{markdown}");
            }
        }
    }
}
