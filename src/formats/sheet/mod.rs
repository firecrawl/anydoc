//! Excel spreadsheets (xlsx, xlsm, xlsb, xls). Every container is read
//! in-house: SpreadsheetML as XML (xlsx, xlsm) or binary (xlsb), and
//! OLE-based BIFF (xls). All three share the number format engine and grid
//! assembly, so one workbook saved in any of them converts identically.

mod controls;
mod numfmt;
mod xls;
mod xlsb;
mod xlsx;

use crate::error::ConvertError;
use crate::model::Document;
use crate::package::archive::probe_ole;
use crate::package::relationships::{read_rels, rel_type};
use crate::package::{Package, path};

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    if bytes.starts_with(&OLE_MAGIC) {
        // An encrypted OOXML package is an OLE container carrying no BIFF
        // workbook stream, so it has to be named before the reader looks
        // for one.
        return match probe_ole(bytes) {
            Some(e @ ConvertError::Encrypted) => Err(e),
            _ => xls::parse(bytes),
        };
    }
    // Failing to open as a ZIP means not a workbook; a resource limit
    // tripped by a valid archive still propagates.
    let mut pkg = match Package::open(bytes) {
        Ok(pkg) => pkg,
        Err(ConvertError::Malformed { .. }) => return Err(not_a_workbook()),
        Err(e) => return Err(e),
    };
    let Some(wb_part) = main_part(&mut pkg)? else {
        return Err(not_a_workbook());
    };
    match classify(&mut pkg, &wb_part)? {
        Some(Container::Xml) => xlsx::parse(&mut pkg, &wb_part),
        Some(Container::Bin) => xlsb::parse(&mut pkg, &wb_part),
        None => Err(not_a_workbook()),
    }
}

fn not_a_workbook() -> ConvertError {
    ConvertError::malformed("not a readable workbook container")
}

enum Container {
    Xml,
    Bin,
}

/// The package's main part: the root relationship names it wherever it
/// lives, so it decides ahead of the conventional locations, which a
/// package may also contain as leftovers.
fn main_part(pkg: &mut Package) -> Result<Option<String>, ConvertError> {
    if let Some(target) = read_rels(pkg, "_rels/.rels")?
        .first_of_type(rel_type::OFFICE_DOCUMENT)
        .and_then(|rel| path::resolve("", &rel.target).ok())
    {
        return Ok(Some(target.path));
    }
    Ok(["xl/workbook.xml", "xl/workbook.bin"]
        .into_iter()
        .find(|name| pkg.has_part(name))
        .map(str::to_string))
}

/// The container a main part belongs to, from its own bytes rather than its
/// name: a SpreadsheetML workbook is XML, an xlsb one is a record stream.
fn classify(pkg: &mut Package, part: &str) -> Result<Option<Container>, ConvertError> {
    let Some(bytes) = pkg.part(part)? else {
        return Ok(None);
    };
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    // A UTF-16 part opens on its byte order mark rather than the tag.
    let is_xml = matches!(body.iter().find(|b| !b.is_ascii_whitespace()), Some(b'<' | 0xFF | 0xFE));
    if is_xml {
        return Ok(Some(Container::Xml));
    }
    Ok((!body.is_empty()).then_some(Container::Bin))
}

/// Float formatting at the 15 significant decimal digits a spreadsheet
/// stores and displays. Shortest round-trip formatting past that surfaces the
/// binary representation (`3554.7000000000003`); 15 digits still keeps small
/// values like 0.0000004 exact.
fn format_float(f: f64) -> String {
    match format!("{f:.14e}").parse::<f64>() {
        Ok(rounded) => format!("{rounded}"),
        Err(_) => format!("{f}"),
    }
}

/// Render a time-of-day serial (a fraction of a day) as `hh:mm:ss`.
fn format_time_of_day(days: f64) -> String {
    let total_secs = (days.abs() * 86_400.0).round() as u64 % 86_400;
    let (h, m, s) = (total_secs / 3600, (total_secs % 3600) / 60, total_secs % 60);
    format!("{h:02}:{m:02}:{s:02}")
}

/// Render an Excel duration (stored in days) as `[h]:mm:ss`.
fn format_duration_days(days: f64) -> String {
    let sign = if days < 0.0 { "-" } else { "" };
    let total_secs = (days.abs() * 86_400.0).round() as u64;
    let (h, m, s) = (total_secs / 3600, (total_secs % 3600) / 60, total_secs % 60);
    format!("{sign}{h}:{m:02}:{s:02}")
}

/// Decode an RK value, the packed number both binary containers use. Bit 0
/// asks for a hundredth of the result; bit 1 selects an integer over the
/// high 30 bits of a double.
pub(super) fn rk_number(rk: u32) -> f64 {
    let value = if rk & 0x02 != 0 {
        f64::from((rk as i32) >> 2)
    } else {
        f64::from_bits(u64::from(rk & 0xFFFF_FFFC) << 32)
    };
    if rk & 0x01 != 0 { value / 100.0 } else { value }
}

/// The literal an error code displays as, shared by both binary containers.
pub(super) fn error_literal(code: u8) -> Option<&'static str> {
    Some(match code {
        0x00 => "#NULL!",
        0x07 => "#DIV/0!",
        0x0F => "#VALUE!",
        0x17 => "#REF!",
        0x1D => "#NAME?",
        0x24 => "#NUM!",
        0x2A => "#N/A",
        0x2B => "#GETTING_DATA",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn tiny_floats_survive() {
        assert_eq!(format_float(0.0000004), "0.0000004");
        assert_eq!(format_float(12.0), "12");
        assert_eq!(format_float(1.5), "1.5");
    }

    #[test]
    fn time_of_day_serials_carry_no_date() {
        // 09:04:54 as a fraction of a day, with the float noise a serial
        // carries in practice.
        assert_eq!(format_time_of_day(32_694.184 / 86_400.0), "09:04:54");
        assert_eq!(format_time_of_day(0.0), "00:00:00");
    }

    #[test]
    fn floats_render_at_spreadsheet_precision() {
        assert_eq!(format_float(3554.7000000000003), "3554.7");
        assert_eq!(format_float(5649.5599999999995), "5649.56");
        assert_eq!(format_float(346_289_529.491_800_1), "346289529.4918");
        // Small values stay exact: 15 significant digits reaches far below 1.
        assert_eq!(format_float(0.0000004), "0.0000004");
        assert_eq!(format_float(1.0), "1");
    }

    #[test]
    fn durations_render_as_clock_time() {
        // 26h30m15s = 1.104340277... days
        let days = (26.0 * 3600.0 + 30.0 * 60.0 + 15.0) / 86_400.0;
        assert_eq!(format_duration_days(days), "26:30:15");
        assert_eq!(format_duration_days(-0.5), "-12:00:00");
    }

    #[test]
    fn an_encrypted_ooxml_package_is_not_read_as_biff() {
        // An encrypted workbook is an OLE container carrying no BIFF
        // workbook stream, so the container check alone would send it to
        // the wrong reader.
        let mut ole = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        ole.create_stream("EncryptionInfo").unwrap();
        ole.create_stream("EncryptedPackage").unwrap();
        let bytes = ole.into_inner().into_inner();

        assert!(matches!(parse(&bytes), Err(ConvertError::Encrypted)));
    }

    #[test]
    fn a_package_that_is_not_a_workbook_does_not_convert_as_one() {
        use std::io::Write as _;

        // Resolving the root relationship reaches any OOXML main part, so
        // without a workbook check a document would convert to nothing.
        const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
        let rels = format!(
            r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/officeDocument" Target="word/document.xml"/></Relationships>"#
        );
        let doc = r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body/></w:document>"#;

        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in [("_rels/.rels", rels.as_str()), ("word/document.xml", doc)] {
            zip.start_file(name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();

        assert!(matches!(parse(&bytes), Err(ConvertError::Malformed { .. })));
    }
}
