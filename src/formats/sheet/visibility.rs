//! Row/column visibility from SpreadsheetML worksheet parts.
//!
//! calamine exposes cell values and merges but not `<row hidden>` /
//! `<col hidden>`. For xlsx-like OOXML packages this module opens the zip,
//! maps sheet names to worksheet parts via workbook relationships, and
//! streams only those visibility attributes.
//!
//! Scope: OOXML SpreadsheetML (`.xlsx` / `.xlsm`). Binary `.xls` / `.xlsb`
//! and ODS are not covered here. Collapsed outline groups and zero
//! width/height columns or rows are out of scope.

use crate::package::limits;
use crate::package::path;
use crate::package::xml::ns;
use quick_xml::NsReader;
use quick_xml::XmlVersion;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read};
use zip::ZipArchive;

/// Hidden axes for one worksheet, in calamine's 0-indexed coordinates.
#[derive(Debug, Default)]
pub(super) struct SheetHidden {
    rows: HashSet<u32>,
    /// Inclusive column ranges, 0-indexed.
    col_ranges: Vec<(u32, u32)>,
}

impl SheetHidden {
    pub(super) fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.col_ranges.is_empty()
    }

    pub(super) fn row_hidden(&self, abs_row: u32) -> bool {
        self.rows.contains(&abs_row)
    }

    pub(super) fn col_hidden(&self, abs_col: u32) -> bool {
        self.col_ranges.iter().any(|&(min, max)| abs_col >= min && abs_col <= max)
    }
}

/// Sheet name → hidden rows/columns. Empty when the bytes are not OOXML or
/// carry no hidden axes. Resource-limit failures on individual parts are
/// logged and that sheet is omitted from the map (conversion continues
/// without filtering that sheet).
pub(super) fn load_hidden_axes(bytes: &[u8]) -> HashMap<String, SheetHidden> {
    let mut zip = match ZipArchive::new(Cursor::new(bytes)) {
        Ok(z) => z,
        Err(_) => return HashMap::new(),
    };
    if zip.by_name("xl/workbook.xml").is_err() {
        return HashMap::new();
    }

    let workbook = match read_capped(&mut zip, "xl/workbook.xml") {
        Some(b) => b,
        None => return HashMap::new(),
    };
    let sheets = parse_sheet_rids(&workbook);
    if sheets.is_empty() {
        return HashMap::new();
    }

    let rels = match read_capped(&mut zip, "xl/_rels/workbook.xml.rels") {
        Some(b) => b,
        None => return HashMap::new(),
    };
    let rid_to_target = parse_worksheet_targets(&rels);

    let mut out = HashMap::new();
    for (name, rid) in sheets {
        let Some(target) = rid_to_target.get(&rid) else {
            continue;
        };
        let part = match path::resolve("xl/workbook.xml", target) {
            Ok(t) => t.path,
            Err(e) => {
                log::warn!("skipping unresolvable worksheet target {target:?}: {e}");
                continue;
            }
        };
        let Some(sheet_bytes) = read_capped(&mut zip, &part) else {
            continue;
        };
        let hidden = parse_sheet_hidden(&sheet_bytes);
        if !hidden.is_empty() {
            out.insert(name, hidden);
        }
    }
    out
}

fn read_capped(zip: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let mut file = match zip.by_name(name) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return None,
        Err(e) => {
            log::warn!("skipping unreadable archive entry {name}: {e}");
            return None;
        }
    };
    if file.size() > limits::MAX_ENTRY_BYTES {
        log::warn!("skipping oversized archive entry {name}");
        return None;
    }
    let mut buf = Vec::new();
    let cap = limits::MAX_ENTRY_BYTES;
    match (&mut file).take(cap + 1).read_to_end(&mut buf) {
        Ok(n) if (n as u64) > cap => {
            log::warn!("skipping archive entry {name}: exceeds decompression cap");
            None
        }
        Ok(_) => Some(buf),
        Err(e) => {
            log::warn!("skipping corrupt archive entry {name}: {e}");
            None
        }
    }
}

/// `(sheet name, rId)` in workbook order.
fn parse_sheet_rids(workbook: &[u8]) -> Vec<(String, String)> {
    let mut reader = NsReader::from_reader(workbook);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                let local = local_name(reader.decoder(), e.name());
                if local != "sheet" {
                    buf.clear();
                    continue;
                }
                let mut name = None;
                let mut rid = None;
                for attr in e.attributes().flatten() {
                    let key = attr_local(reader.decoder(), attr.key);
                    match key.as_str() {
                        "name" => {
                            name = attr_value(reader.decoder(), &attr);
                        }
                        // Prefer r:id (relationships ns); bare id is rare.
                        "id" if rid.is_none() || attr_ns_is_rels(&reader, attr.key) => {
                            rid = attr_value(reader.decoder(), &attr);
                        }
                        _ => {}
                    }
                }
                if let (Some(name), Some(rid)) = (name, rid) {
                    out.push((name, rid));
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn parse_worksheet_targets(rels: &[u8]) -> HashMap<String, String> {
    let mut reader = NsReader::from_reader(rels);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut out = HashMap::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                let local = local_name(reader.decoder(), e.name());
                if local != "Relationship" {
                    buf.clear();
                    continue;
                }
                let mut id = None;
                let mut target = None;
                let mut rel_type = None;
                for attr in e.attributes().flatten() {
                    let key = attr_local(reader.decoder(), attr.key);
                    let val = attr_value(reader.decoder(), &attr);
                    match key.as_str() {
                        "Id" => id = val,
                        "Target" => target = val,
                        "Type" => rel_type = val,
                        _ => {}
                    }
                }
                if let (Some(id), Some(target), Some(rel_type)) = (id, target, rel_type)
                    && rel_type.contains("/worksheet")
                {
                    out.insert(id, target);
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

fn parse_sheet_hidden(sheet: &[u8]) -> SheetHidden {
    let mut reader = NsReader::from_reader(sheet);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut hidden = SheetHidden::default();
    let mut next_row = 1u32;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                let local = local_name(reader.decoder(), e.name());
                match local.as_str() {
                    "col" => {
                        let mut min = None;
                        let mut max = None;
                        let mut is_hidden = false;
                        for attr in e.attributes().flatten() {
                            let key = attr_local(reader.decoder(), attr.key);
                            let val = attr_value(reader.decoder(), &attr);
                            match key.as_str() {
                                "min" => min = val.as_deref().and_then(|v| v.parse::<u32>().ok()),
                                "max" => max = val.as_deref().and_then(|v| v.parse::<u32>().ok()),
                                "hidden" => {
                                    is_hidden = val.as_deref().is_some_and(ooxml_bool);
                                }
                                _ => {}
                            }
                        }
                        if is_hidden {
                            let Some(min) = min.filter(|&m| m >= 1) else {
                                buf.clear();
                                continue;
                            };
                            let max = max.unwrap_or(min).max(min);
                            // OOXML cols are 1-indexed; calamine uses 0-indexed.
                            hidden.col_ranges.push((min - 1, max - 1));
                        }
                    }
                    "row" => {
                        let mut r = None;
                        let mut is_hidden = false;
                        for attr in e.attributes().flatten() {
                            let key = attr_local(reader.decoder(), attr.key);
                            let val = attr_value(reader.decoder(), &attr);
                            match key.as_str() {
                                "r" => r = val.as_deref().and_then(|v| v.parse::<u32>().ok()),
                                "hidden" => {
                                    is_hidden = val.as_deref().is_some_and(ooxml_bool);
                                }
                                _ => {}
                            }
                        }
                        let row_num = r.unwrap_or(next_row).max(1);
                        next_row = row_num.saturating_add(1);
                        if is_hidden {
                            hidden.rows.insert(row_num - 1);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    hidden
}

fn ooxml_bool(v: &str) -> bool {
    v == "1" || v.eq_ignore_ascii_case("true")
}

fn local_name(decoder: quick_xml::Decoder, name: quick_xml::name::QName<'_>) -> String {
    let raw = name.local_name().into_inner();
    decoder.decode(raw).map(|s| s.into_owned()).unwrap_or_default()
}

fn attr_local(decoder: quick_xml::Decoder, key: quick_xml::name::QName<'_>) -> String {
    local_name(decoder, key)
}

fn attr_value(
    decoder: quick_xml::Decoder,
    attr: &quick_xml::events::attributes::Attribute<'_>,
) -> Option<String> {
    attr.decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder).ok().map(|s| s.into_owned())
}

fn attr_ns_is_rels(reader: &NsReader<&[u8]>, key: quick_xml::name::QName<'_>) -> bool {
    matches!(
        reader.resolver().resolve_attribute(key),
        (ResolveResult::Bound(ns), _) if ns.as_ref() == ns::R.as_bytes()
            || ns.as_ref() == b"http://purl.oclc.org/ooxml/officeDocument/relationships"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hidden_row_and_col_attrs() {
        let xml = br#"<?xml version="1.0"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
  <cols>
    <col min="2" max="2" hidden="1" width="10"/>
    <col min="4" max="5" hidden="true" width="8"/>
  </cols>
  <sheetData>
    <row r="1"><c r="A1"/></row>
    <row r="2" hidden="1"><c r="A2"/></row>
    <row r="4" hidden="true"><c r="A4"/></row>
  </sheetData>
</worksheet>"#;
        let h = parse_sheet_hidden(xml);
        assert!(h.row_hidden(1));
        assert!(h.row_hidden(3));
        assert!(!h.row_hidden(0));
        assert!(h.col_hidden(1));
        assert!(h.col_hidden(3));
        assert!(h.col_hidden(4));
        assert!(!h.col_hidden(0));
        assert!(!h.col_hidden(2));
    }
}
