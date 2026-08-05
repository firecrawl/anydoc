//! Header-row detection for grids whose format designates no header row. CSV
//! and spreadsheets have no notion of one; Word, RTF, and DrawingML carry only
//! pagination and table-style flags (`w:tblHeader`, `\trhdr`,
//! `a:tblPr/@firstRow`), none of which means "this row labels the columns".
//! The first row is a header when the columns below it are consistently typed
//! and the row itself is not: a label sits above a column of numbers, dates or
//! booleans without being one of them.

use crate::model::{Block, CellSlot, Table, inlines_to_plain_text};
use std::collections::{HashMap, HashSet};

/// Body rows the column types are read from. A header shows in the first
/// screenful of data, so a full pass over a million-row export buys nothing.
const SAMPLE_ROWS: usize = 50;

/// Share of a column's non-empty body cells that must agree on one type
/// before the column votes, as a numerator over [`DOMINANCE_DEN`]. Stray
/// `N/A` and footnote markers are common enough that exact agreement would
/// silence otherwise obvious columns.
const DOMINANCE_NUM: usize = 9;
const DOMINANCE_DEN: usize = 10;

/// Longest a label can be when nothing else distinguishes the first row.
/// Beyond this the text reads as prose, which is data.
const MAX_LABEL: usize = 64;

/// What a cell value looks like, at the granularity the vote needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Number,
    Bool,
    Date,
    Text,
}

/// A grid's [`Table::header_rows`]: what the format `declared`, or - when it
/// declared none - 1 if the first row labels the columns and 0 otherwise. Only
/// the first row is ever considered, since that is where labels go.
pub fn resolve_header_rows(table: &Table, declared: usize) -> usize {
    if declared > 0 {
        return declared.min(table.grid.len());
    }
    let grid = &table.grid;
    if grid.len() < 2 {
        return 0;
    }
    // A span in the first row means grouped columns or a merged title, not
    // one label per column.
    if grid[0]
        .iter()
        .any(|s| !matches!(s, CellSlot::Origin(c) if c.col_span == 1 && c.row_span == 1))
    {
        return 0;
    }
    let sample = &grid[1..grid.len().min(SAMPLE_ROWS + 1)];
    // A first row that does not have the body's field count is a title line
    // or a stray fragment, not a header.
    if grid[0].len() != modal_width(sample) {
        return 0;
    }

    let head: Vec<String> = grid[0].iter().map(slot_text).collect();
    let body: Vec<Vec<String>> =
        sample.iter().map(|row| row.iter().map(slot_text).collect()).collect();
    // Columns past the last one holding content never render, so they take no
    // part in the checks below.
    let content_width =
        |row: &Vec<String>| row.iter().rposition(|v| !v.trim().is_empty()).map_or(0, |i| i + 1);
    let width = std::iter::once(&head).chain(body.iter()).map(content_width).max().unwrap_or(0);
    // Content past the last label is an unlabelled column, same as a blank
    // label between two filled ones.
    if width == 0 || width > head.len() {
        return 0;
    }

    let mut seen: HashSet<String> = HashSet::new();
    for (c, value) in head.iter().enumerate().take(width) {
        // Every column carries a label, with one exception: the unnamed index
        // column that pandas and R write as a leading empty field.
        if value.trim().is_empty() {
            if c == 0 {
                continue;
            }
            return 0;
        }
        // A label is one line; a cell holding several is data.
        if value.contains('\n') {
            return 0;
        }
        // Repeated values are what a data row looks like, not a header.
        if !seen.insert(fold(value)) {
            return 0;
        }
    }

    let (mut header_votes, mut data_votes) = (0usize, 0usize);
    for (c, label) in head.iter().enumerate().take(width) {
        let values: Vec<&str> = body
            .iter()
            .filter_map(|row| row.get(c))
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .collect();
        if values.is_empty() {
            continue;
        }
        let label = label.trim();
        match dominant_kind(&values) {
            // A column of numbers, dates or booleans under a label that is
            // none of those: the label does not belong to its own column.
            Some(kind) if kind != Kind::Text => {
                if classify(label) == Some(Kind::Text) {
                    header_votes += 1;
                } else {
                    data_votes += 1;
                }
            }
            // Text columns carry no type signal, but a first-row value that
            // recurs below is plainly a value of the column, not its label.
            _ => {
                if values.iter().any(|v| fold(v) == fold(label)) {
                    data_votes += 1;
                }
            }
        }
    }

    if header_votes == 0 && data_votes == 0 {
        // Text above text, with no value repeated: only the shape of the row
        // is left to go on. Short single-line entries read as labels, and the
        // alternative is the empty header row every such table renders today.
        let labelled = head.iter().take(width).all(|v| v.chars().count() <= MAX_LABEL);
        return usize::from(labelled);
    }
    usize::from(header_votes > data_votes)
}

/// The field count most sampled body rows agree on. Frequency ties break
/// toward the wider shape so the result never depends on hash-map iteration
/// order.
fn modal_width(rows: &[Vec<CellSlot>]) -> usize {
    let mut tally: HashMap<usize, usize> = HashMap::new();
    for row in rows {
        *tally.entry(row.len()).or_default() += 1;
    }
    tally.into_iter().max_by_key(|&(width, n)| (n, width)).map_or(0, |(width, _)| width)
}

/// A slot's text. Covered positions and cells holding anything but paragraphs
/// yield nothing: neither is a plain value, and both then read as empty.
fn slot_text(slot: &CellSlot) -> String {
    let CellSlot::Origin(cell) = slot else { return String::new() };
    let mut out = String::new();
    for block in &cell.blocks {
        let Block::Paragraph(inlines) = block else { return String::new() };
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&inlines_to_plain_text(inlines));
    }
    out
}

/// Case- and padding-insensitive form used to compare a label against the
/// values below it.
fn fold(value: &str) -> String {
    value.trim().to_lowercase()
}

/// The kind at least [`DOMINANCE_NUM`]/[`DOMINANCE_DEN`] of the values share,
/// if any. Values are non-empty, so they all classify.
fn dominant_kind(values: &[&str]) -> Option<Kind> {
    [Kind::Number, Kind::Bool, Kind::Date, Kind::Text].into_iter().find(|&kind| {
        let n = values.iter().filter(|v| classify(v) == Some(kind)).count();
        n * DOMINANCE_DEN >= values.len() * DOMINANCE_NUM
    })
}

/// Classify one value; `None` when it is blank and carries no type at all.
fn classify(value: &str) -> Option<Kind> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if is_number(value) {
        return Some(Kind::Number);
    }
    if matches!(fold(value).as_str(), "true" | "false" | "yes" | "no") {
        return Some(Kind::Bool);
    }
    if is_temporal(value) {
        return Some(Kind::Date);
    }
    Some(Kind::Text)
}

/// A numeric literal as data files write them: sign, digit grouping, either
/// decimal convention, exponent, and a percent suffix. Which convention a
/// value uses does not matter here, only that it is a number.
fn is_number(value: &str) -> bool {
    let value = value.strip_suffix('%').unwrap_or(value);
    let cleaned: String =
        value.chars().filter(|c| !matches!(c, ',' | ' ' | '_' | '\u{a0}')).collect();
    // `f64` also parses `inf` and `NaN`; a number here has digits in it.
    cleaned.chars().any(|c| c.is_ascii_digit()) && cleaned.parse::<f64>().is_ok()
}

/// A numeric date triple in any field order, optionally followed by a time,
/// or a bare clock time. Month names stay `Text`: a column of them is a
/// column of labels, and it would cast the same vote either way.
fn is_temporal(value: &str) -> bool {
    let (date, time) = value.split_once(['T', ' ']).unwrap_or((value, ""));
    // Fractional seconds and a trailing zone marker are not part of the shape.
    let time = time.trim_end_matches('Z').split('.').next().unwrap_or_default();
    let is_clock = |t: &str| matches!(digit_groups(t, &[':']), 2 | 3);
    if digit_groups(date, &['-', '/', '.']) == 3 {
        return time.is_empty() || is_clock(time);
    }
    time.is_empty() && is_clock(date)
}

/// How many components `value` has when split on the first of `seps` it
/// contains, or 0 unless every component is a short digit run.
fn digit_groups(value: &str, seps: &[char]) -> usize {
    let Some(sep) = seps.iter().copied().find(|&s| value.contains(s)) else { return 0 };
    let parts: Vec<&str> = value.split(sep).collect();
    let short_digits =
        |p: &&str| (1..=4).contains(&p.len()) && p.chars().all(|c| c.is_ascii_digit());
    if parts.iter().all(short_digits) { parts.len() } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Cell, Inline, TableKind};

    fn detect(rows: &[&[&str]]) -> usize {
        let rows: Vec<Vec<Cell>> = rows
            .iter()
            .map(|r| r.iter().map(|t| Cell::from_inlines(vec![Inline::plain(*t)])).collect())
            .collect();
        resolve_header_rows(&Table::from_rows(rows, 0, TableKind::Data), 0)
    }

    #[test]
    fn labels_over_typed_columns_are_a_header() {
        assert_eq!(detect(&[&["name", "qty"], &["a", "1"], &["b", "2"]]), 1);
        assert_eq!(
            detect(&[&["when", "ok"], &["2026-03-15", "TRUE"], &["2026-03-16", "FALSE"]]),
            1
        );
    }

    #[test]
    fn a_first_data_row_is_not_a_header() {
        // Numeric columns with a numeric first row: sanctions-list shape.
        assert_eq!(
            detect(&[&["36", "12", "aka"], &["173", "57", "aka"], &["306", "220", "aka"]]),
            0
        );
    }

    #[test]
    fn a_label_repeated_below_is_data() {
        assert_eq!(detect(&[&["x", "aka"], &["y", "aka"], &["z", "aka"]]), 0);
    }

    #[test]
    fn all_text_falls_back_to_row_shape() {
        assert_eq!(detect(&[&["col1", "col2"], &["naïve", "café"], &["Αθήνα", "数据"]]), 1);
        let long = "l".repeat(MAX_LABEL + 1);
        assert_eq!(detect(&[&[&long, "col2"], &["a", "b"], &["c", "d"]]), 0);
    }

    #[test]
    fn structural_vetoes() {
        // Fewer fields than the body: a title line, not a header.
        assert_eq!(detect(&[&["Q1 report"], &["a", "1"], &["b", "2"]]), 0);
        // An unlabelled column.
        assert_eq!(detect(&[&["name", ""], &["a", "1"], &["b", "2"]]), 0);
        // Duplicate labels.
        assert_eq!(detect(&[&["name", "name"], &["a", "1"], &["b", "2"]]), 0);
        // Nothing to compare against.
        assert_eq!(detect(&[&["name", "qty"]]), 0);
    }

    #[test]
    fn a_leading_index_column_may_be_unlabelled() {
        assert_eq!(detect(&[&["", "qty"], &["a", "1"], &["b", "2"]]), 1);
    }

    #[test]
    fn a_mixed_column_abstains_rather_than_voting_against() {
        // The first row is text over a column of mixed literals; the type
        // signal is absent, so the row shape decides.
        assert_eq!(
            detect(&[&["kind", "value"], &["percent", "15.5%"], &["date", "2026-03-15"]]),
            1
        );
    }

    #[test]
    fn value_kinds() {
        assert_eq!(classify("1,234.56"), Some(Kind::Number));
        assert_eq!(classify("-1.5e3"), Some(Kind::Number));
        assert_eq!(classify("15.5%"), Some(Kind::Number));
        assert_eq!(classify("-0-"), Some(Kind::Text));
        assert_eq!(classify("NaN"), Some(Kind::Text));
        assert_eq!(classify("2026-03-15"), Some(Kind::Date));
        assert_eq!(classify("15/03/2026 09:04"), Some(Kind::Date));
        assert_eq!(classify("2026-03-15T09:04:54.123Z"), Some(Kind::Date));
        assert_eq!(classify("26:30:15"), Some(Kind::Date));
        assert_eq!(classify("yes"), Some(Kind::Bool));
        assert_eq!(classify("  "), None);
    }
}
