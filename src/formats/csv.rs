//! CSV and friends (semicolon, tab, pipe delimited).
//!
//! Field content is preserved as written (RFC 4180: spaces are part of the
//! field); only control characters are cleaned. Encoding is detected from
//! the BOM (UTF-8, UTF-16LE/BE), then UTF-8, then Windows-1252. The
//! delimiter is chosen by trial-parsing candidates and scoring record
//! consistency, so delimiters inside quoted fields don't skew the choice.

use crate::error::ConvertError;
use crate::model::{Block, Cell, Document, Inline, Table, TableKind};
use crate::shared::text::clean_text;
use csv::ReaderBuilder;
use std::borrow::Cow;

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let text = decode(bytes);
    let delimiter = sniff_delimiter(&text);

    let mut reader = ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(text.as_bytes());

    let mut records: Vec<Vec<String>> = Vec::new();
    for record in reader.records() {
        let record = match record {
            Ok(r) => r,
            Err(e) => {
                log::warn!("skipping unreadable csv record: {e}");
                continue;
            }
        };
        records.push(record.iter().map(clean_text).collect());
    }

    let header_rows = if first_row_is_header(&records) { 1 } else { 0 };
    let rows: Vec<Vec<Cell>> = records
        .into_iter()
        .map(|r| r.into_iter().map(|f| Cell::from_inlines(vec![Inline::plain(f)])).collect())
        .collect();

    let mut doc = Document::default();
    let table = Table::from_rows(rows, header_rows, TableKind::Data);
    if !table.grid.is_empty() {
        doc.blocks.push(Block::Table(table));
    }
    Ok(doc)
}

/// Whether the first record reads as a header row. CSV carries no header
/// semantics, so this is a heuristic — but an asymmetric one: promoting a
/// data row only restyles it (the row still renders, in the header
/// position), while not promoting a real header leaves a blank header row
/// with the column names down in the table body. So the test asks only that
/// the first row *look like* labels: at least two columns, as wide as any
/// record, and every field non-empty, non-numeric, and distinct. Files that
/// plainly have no header — numeric first rows, ragged or repeated fields,
/// a single record — keep today's headerless output.
fn first_row_is_header(records: &[Vec<String>]) -> bool {
    let [first, rest @ ..] = records else { return false };
    if rest.is_empty() || first.len() < 2 {
        return false;
    }
    if rest.iter().any(|r| r.len() > first.len()) {
        return false; // a header names every column
    }
    let mut seen = std::collections::HashSet::new();
    first.iter().all(|f| {
        let t = f.trim();
        !t.is_empty() && !is_numeric_like(t) && seen.insert(t.to_lowercase())
    })
}

/// Numbers as data files spell them: plain floats plus a single
/// decimal-comma variant (`1,5`).
fn is_numeric_like(field: &str) -> bool {
    if field.parse::<f64>().is_ok() {
        return true;
    }
    let mut commas = field.split(',');
    match (commas.next(), commas.next(), commas.next()) {
        (Some(a), Some(b), None) => format!("{a}.{b}").parse::<f64>().is_ok(),
        _ => false,
    }
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

    fn header_rows_of(doc: &Document) -> usize {
        let Block::Table(t) = &doc.blocks[0] else { panic!() };
        t.header_rows
    }

    #[test]
    fn label_like_first_row_is_promoted_to_header() {
        // The issue's repro: all-text data, so header detection cannot rely
        // on the body containing numbers.
        let doc =
            parse(b"name,role,team\nAlice,Engineer,Platform\nBob,Designer,Product\n").unwrap();
        assert_eq!(header_rows_of(&doc), 1);
        // Promotion restyles the first row; nothing is dropped.
        let Block::Table(t) = &doc.blocks[0] else { panic!() };
        assert_eq!(t.grid.len(), 3);
    }

    #[test]
    fn numeric_first_row_stays_headerless() {
        let doc = parse(b"1,2,3\n4,5,6\n").unwrap();
        assert_eq!(header_rows_of(&doc), 0);
        // Decimal commas inside quoted fields are numbers too.
        let doc = parse(b"\"1,5\";\"2,5\";x\n\"3,0\";y;z\n").unwrap();
        assert_eq!(header_rows_of(&doc), 0);
    }

    #[test]
    fn repeated_empty_or_short_first_rows_stay_headerless() {
        // Duplicate labels.
        let doc = parse(b"a,b,a\n1,2,3\n").unwrap();
        assert_eq!(header_rows_of(&doc), 0);
        // An empty field in the first row.
        let doc = parse(b"a,,c\n1,2,3\n").unwrap();
        assert_eq!(header_rows_of(&doc), 0);
        // A first row narrower than the body names no header for column 3.
        let doc = parse(b"a,b\n1,2,3\n").unwrap();
        assert_eq!(header_rows_of(&doc), 0);
    }

    #[test]
    fn single_record_or_single_column_stays_headerless() {
        let doc = parse(b"name,role,team\n").unwrap();
        assert_eq!(header_rows_of(&doc), 0);
        let doc = parse(b"name\nAlice\nBob\n").unwrap();
        assert_eq!(header_rows_of(&doc), 0);
    }
}
