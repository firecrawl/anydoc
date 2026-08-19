//! CSV and friends (semicolon, tab, pipe delimited).
//!
//! Field content is preserved as written (RFC 4180: spaces are part of the
//! field); only control characters are cleaned. Encoding is detected from
//! the BOM (UTF-8, UTF-16LE/BE), then UTF-8, then Windows-1252. The
//! delimiter is chosen by trial-parsing candidates and scoring record
//! consistency, so delimiters inside quoted fields don't skew the choice. The
//! format marks no header row, so the shape of the data decides whether the
//! first record is one.

use crate::error::ConvertError;
use crate::model::{Block, Cell, Document, Inline, Table, TableKind};
use crate::package::materialization::MaterializationBudget;
use crate::shared::header::resolve_header_rows;
use crate::shared::text::clean_text;
use csv::ReaderBuilder;
use std::borrow::Cow;

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    parse_with_budget(bytes, MaterializationBudget::default())
}

fn parse_with_budget(
    bytes: &[u8],
    mut budget: MaterializationBudget,
) -> Result<Document, ConvertError> {
    budget.check_input(bytes.len())?;
    let text = decode(bytes);
    let delimiter = sniff_delimiter(&text);

    let mut reader = ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes());

    let mut rows: Vec<Vec<Cell>> = Vec::new();
    for record in reader.records() {
        let record = match record {
            Ok(r) => r,
            Err(e) => {
                log::warn!("skipping unreadable csv record: {e}");
                continue;
            }
        };
        let mut cells = Vec::new();
        for field in &record {
            budget.charge_cell()?;
            budget.charge_text(field.len())?;
            budget.charge_text_run()?;
            cells.push(Cell::from_inlines(vec![Inline::plain(clean_text(field))]));
        }
        rows.push(cells);
    }

    let mut doc = Document::default();
    let mut table = Table::from_rows(rows, 0, TableKind::Data);
    table.header_rows = resolve_header_rows(&table, 0);
    if !table.grid.is_empty() {
        doc.blocks.push(Block::Table(table));
    }
    Ok(doc)
}

fn decode(bytes: &[u8]) -> Cow<'_, str> {
    match bytes {
        [0xFF, 0xFE, ..] => {
            let (s, _, _) = encoding_rs::UTF_16LE.decode(bytes);
            Cow::Owned(s.into_owned())
        }
        [0xFE, 0xFF, ..] => {
            let (s, _, _) = encoding_rs::UTF_16BE.decode(bytes);
            Cow::Owned(s.into_owned())
        }
        _ => {
            let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
            match std::str::from_utf8(bytes) {
                Ok(s) => Cow::Borrowed(s),
                Err(_) => {
                    let (s, _, _) = encoding_rs::WINDOWS_1252.decode(bytes);
                    Cow::Owned(s.into_owned())
                }
            }
        }
    }
}

/// Trial-parse each candidate over the leading records and score by field
/// consistency; comma wins ties.
fn sniff_delimiter(text: &str) -> u8 {
    const CANDIDATES: [u8; 4] = *b",;\t|";
    let mut best = (b',', 0u32);
    for d in CANDIDATES {
        let mut reader = ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .delimiter(d)
            .from_reader(text.as_bytes());
        // Sample complete records (the reader is streaming, so this reads
        // only as much input as the records span): sampling physical lines
        // would cut a quoted multiline field in half.
        let counts: Vec<usize> =
            reader.records().map_while(Result::ok).take(20).map(|r| r.len()).collect();
        if counts.is_empty() {
            continue;
        }
        // Modal field count and how dominant it is.
        let mut tally: std::collections::HashMap<usize, u32> = std::collections::HashMap::new();
        for &c in &counts {
            *tally.entry(c).or_default() += 1;
        }
        // Frequency ties break toward the wider record shape so the choice
        // never depends on hash-map iteration order.
        let (&modal, &freq) = tally.iter().max_by_key(|&(&count, &f)| (f, count)).unwrap();
        if modal < 2 {
            continue; // a delimiter that never splits carries no signal
        }
        // Consistency first, then wider records break the tie.
        let score = freq * 1000 + modal.min(500) as u32;
        if score > best.1 {
            best = (d, score);
        }
    }
    best.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_fields_keep_padding() {
        let doc = parse(b"a,b\n\"  padded  \",x\n").unwrap();
        let Block::Table(t) = &doc.blocks[0] else { panic!() };
        let crate::model::CellSlot::Origin(cell) = &t.grid[1][0] else { panic!() };
        let Block::Paragraph(inlines) = &cell.blocks[0] else { panic!() };
        let crate::model::Inline::Text { text, .. } = &inlines[0] else { panic!() };
        assert_eq!(text, "  padded  ");
    }

    #[test]
    fn quoted_delimiters_do_not_skew_sniffing() {
        // Semicolon-delimited with commas inside quoted fields.
        assert_eq!(sniff_delimiter("a;b;c\n\"1,5\";\"2,5\";x\n\"3,0\";y;z\n"), b';');
        assert_eq!(sniff_delimiter("a,b,c\n1,2,3\n"), b',');
        assert_eq!(sniff_delimiter("a\tb\n1\t2\n"), b'\t');
    }

    #[test]
    fn multiline_quoted_field_does_not_break_sniffing() {
        // A quoted field spanning more physical lines than the old 20-line
        // sample window; the record-based sample must still see semicolons.
        let long_field = format!("\"{}\"", vec!["line"; 30].join("\n"));
        let text = format!("a;b;c\n{long_field};2;3\nx;y;z\n");
        assert_eq!(sniff_delimiter(&text), b';');
    }

    #[test]
    fn utf16_bom_decodes() {
        let mut bytes = vec![0xFF, 0xFE];
        for u in "x,y\ncafé,90\n".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let doc = parse(&bytes).unwrap();
        assert!(!doc.blocks.is_empty());
    }

    fn resource_limit_name(error: ConvertError) -> &'static str {
        match error {
            ConvertError::ResourceLimit { limit, .. } => limit,
            other => panic!("expected resource limit, got {other}"),
        }
    }

    #[test]
    fn cell_materialization_is_bounded() {
        let budget = MaterializationBudget::with_limits(1024, 1024, 3, 10);
        let error = parse_with_budget(b"a,b\nc,d\n", budget).unwrap_err();
        assert_eq!(resource_limit_name(error), "max_materialized_cells");
    }

    #[test]
    fn copied_text_materialization_is_bounded() {
        let budget = MaterializationBudget::with_limits(1024, 5, 10, 10);
        let error = parse_with_budget(b"abcdef\n", budget).unwrap_err();
        assert_eq!(resource_limit_name(error), "max_materialized_text_bytes");
    }
}
