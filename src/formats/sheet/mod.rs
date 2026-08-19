//! Excel spreadsheets (xlsx, xlsm, xlsb, xls). Every container is read
//! in-house: SpreadsheetML as XML (xlsx, xlsm) or binary (xlsb), and
//! OLE-based BIFF (xls). All three share the number format engine and grid
//! assembly, so one workbook saved in any of them converts identically.

mod numfmt;
mod xls;
mod xlsb;
mod xlsx;

use crate::error::ConvertError;
use crate::model::Document;
use crate::package::archive::probe_ole;
use std::io::Cursor;

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    match container(bytes) {
        Some(Container::Ole) => {
            // An encrypted OOXML package is an OLE container carrying no
            // BIFF workbook stream, so it has to be named before the
            // reader looks for one.
            match probe_ole(bytes) {
                Some(e @ ConvertError::Encrypted) => Err(e),
                _ => xls::parse(bytes),
            }
        }
        Some(Container::Xml) => xlsx::parse(bytes),
        Some(Container::Bin) => xlsb::parse(bytes),
        None => Err(ConvertError::malformed("not a readable workbook container")),
    }
}

enum Container {
    Ole,
    Xml,
    Bin,
}

/// Which workbook container the bytes hold: an OLE compound file (xls), or a
/// ZIP holding `xl/workbook.xml` (xlsx, xlsm) or `xl/workbook.bin` (xlsb).
fn container(bytes: &[u8]) -> Option<Container> {
    const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    if bytes.starts_with(&OLE_MAGIC) {
        return Some(Container::Ole);
    }
    let zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    if zip.index_for_name("xl/workbook.xml").is_some() {
        return Some(Container::Xml);
    }
    if zip.index_for_name("xl/workbook.bin").is_some() {
        return Some(Container::Bin);
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
