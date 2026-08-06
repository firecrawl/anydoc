//! Shared primitives for the legacy OLE2 binary formats (DOC, PPT):
//! checked little-endian integer readers and bounded compound-file stream
//! reading.

use crate::error::ConvertError;
use crate::package::limits;
use std::io::{Cursor, Read};

/// Little-endian `u16` at `off`; `None` when out of bounds.
pub fn get_u16(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(off..off + 2)?.try_into().ok()?))
}

/// Little-endian `u32` at `off`; `None` when out of bounds.
pub fn get_u32(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

/// Map an Office code-page number to the decoder used by RTF and OLE
/// property-set strings. Unknown pages degrade to Windows-1252.
pub fn codepage_encoding(cp: u32) -> &'static encoding_rs::Encoding {
    match cp {
        932 => encoding_rs::SHIFT_JIS,
        936 => encoding_rs::GBK,
        949 => encoding_rs::EUC_KR,
        950 => encoding_rs::BIG5,
        1250 => encoding_rs::WINDOWS_1250,
        1251 => encoding_rs::WINDOWS_1251,
        1253 => encoding_rs::WINDOWS_1253,
        1254 => encoding_rs::WINDOWS_1254,
        1255 => encoding_rs::WINDOWS_1255,
        1256 => encoding_rs::WINDOWS_1256,
        1257 => encoding_rs::WINDOWS_1257,
        1258 => encoding_rs::WINDOWS_1258,
        874 => encoding_rs::WINDOWS_874,
        _ => encoding_rs::WINDOWS_1252,
    }
}

/// Read a named stream from an OLE2 compound file. A missing stream is
/// `MissingPart`; the read is hard-capped at `MAX_ENTRY_BYTES` so a corrupt
/// sector chain cannot expand without bound.
pub fn read_ole_stream<R: Read + std::io::Seek>(
    ole: &mut cfb::CompoundFile<R>,
    name: &str,
) -> Result<Vec<u8>, ConvertError> {
    let stream =
        ole.open_stream(name).map_err(|_| ConvertError::MissingPart { part: name.to_string() })?;
    let mut bytes = Vec::new();
    let read = stream.take(limits::MAX_ENTRY_BYTES + 1).read_to_end(&mut bytes).map_err(|e| {
        ConvertError::Malformed {
            part: Some(name.to_string()),
            detail: format!("unreadable stream: {e}"),
        }
    })? as u64;
    if read > limits::MAX_ENTRY_BYTES {
        return Err(ConvertError::ResourceLimit {
            limit: "max_entry_bytes",
            detail: format!("{name} stream exceeds the read cap"),
        });
    }
    Ok(bytes)
}

/// Read PIDSI_TITLE from an OLE SummaryInformation property set. Property
/// offsets are untrusted, so malformed metadata returns `None` and leaves the
/// otherwise readable document on its normal conversion path.
pub fn ole_summary_title(bytes: &[u8]) -> Option<String> {
    let mut ole = cfb::CompoundFile::open(Cursor::new(bytes)).ok()?;
    let summary = match read_ole_stream(&mut ole, "\x05SummaryInformation") {
        Ok(summary) => summary,
        Err(error) => {
            log::debug!("OLE SummaryInformation unavailable: {error}");
            return None;
        }
    };
    parse_summary_title(&summary)
}

fn parse_summary_title(bytes: &[u8]) -> Option<String> {
    // Property-set byte order must be little endian (the only form used by
    // Office OLE files). The rest of the header is fixed-size.
    if get_u16(bytes, 0)? != 0xFFFE {
        return None;
    }
    let section_count = get_u32(bytes, 24)? as usize;
    let directory_end = 28usize.checked_add(section_count.checked_mul(20)?)?;
    if directory_end > bytes.len() {
        return None;
    }
    for index in 0..section_count {
        let entry = 28usize.checked_add(index.checked_mul(20)?)?;
        let section_offset = get_u32(bytes, entry + 16)? as usize;
        if let Some(title) = parse_summary_section(bytes, section_offset) {
            return Some(title);
        }
    }
    None
}

fn parse_summary_section(bytes: &[u8], offset: usize) -> Option<String> {
    let size = get_u32(bytes, offset)? as usize;
    let end = offset.checked_add(size)?;
    let section = bytes.get(offset..end)?;
    if size < 8 {
        return None;
    }
    let property_count = get_u32(section, 4)? as usize;
    let entries_bytes = property_count.checked_mul(8)?;
    let entries_end = 8usize.checked_add(entries_bytes)?;
    if entries_end > section.len() {
        return None;
    }

    let mut codepage = 1252u32;
    let mut title = None;
    for index in 0..property_count {
        let entry = 8usize.checked_add(index.checked_mul(8)?)?;
        let property_id = get_u32(section, entry)?;
        let property_offset = get_u32(section, entry + 4)? as usize;
        match property_id {
            1 => {
                if let Some(cp) = property_codepage(section, property_offset) {
                    codepage = cp;
                }
            }
            2 => {
                title = Some(property_offset);
            }
            _ => {}
        }
    }

    let property_offset = title?;
    let property_type = get_u16(section, property_offset)?;
    match property_type {
        // VT_LPSTR: byte count includes the terminating NUL.
        0x001E => {
            let count = get_u32(section, property_offset + 4)? as usize;
            let start = property_offset.checked_add(8)?;
            let bytes = section.get(start..start.checked_add(count)?)?;
            let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
            let (text, _, _) = codepage_encoding(codepage).decode(bytes);
            Some(text.into_owned())
        }
        // VT_LPWSTR: character count includes the terminating UTF-16 NUL.
        0x001F => {
            let count = get_u32(section, property_offset + 4)? as usize;
            let units_start = property_offset.checked_add(8)?;
            let units_end = units_start.checked_add(count.checked_mul(2)?)?;
            let units = section.get(units_start..units_end)?;
            let mut chars = Vec::with_capacity(count);
            for pair in units.chunks_exact(2) {
                chars.push(u16::from_le_bytes([pair[0], pair[1]]));
            }
            while chars.last() == Some(&0) {
                chars.pop();
            }
            Some(String::from_utf16_lossy(&chars))
        }
        _ => None,
    }
}

fn property_codepage(section: &[u8], offset: usize) -> Option<u32> {
    match get_u16(section, offset)? {
        0x0002 | 0x0012 => Some(get_u16(section, offset + 4)? as u32),
        0x0003 | 0x0013 => Some(get_u32(section, offset + 4)?),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary_property_set(title: &[u8], codepage: u32, utf16: bool) -> Vec<u8> {
        let codepage_value = {
            let mut value = vec![0x02, 0x00, 0x00, 0x00];
            value.extend_from_slice(&(codepage as u16).to_le_bytes());
            value.extend_from_slice(&[0, 0]);
            value
        };
        let title_value = if utf16 {
            let mut value = vec![0x1F, 0x00, 0x00, 0x00];
            let mut units: Vec<u16> = String::from_utf8_lossy(title).encode_utf16().collect();
            units.push(0);
            value.extend_from_slice(&(units.len() as u32).to_le_bytes());
            for unit in units {
                value.extend_from_slice(&unit.to_le_bytes());
            }
            value
        } else {
            let mut value = vec![0x1E, 0x00, 0x00, 0x00];
            value.extend_from_slice(&((title.len() + 1) as u32).to_le_bytes());
            value.extend_from_slice(title);
            value.push(0);
            value
        };
        let title_offset = 8 + 16 + codepage_value.len();
        let size = title_offset + title_value.len();
        let mut section = Vec::new();
        section.extend_from_slice(&(size as u32).to_le_bytes());
        section.extend_from_slice(&2u32.to_le_bytes());
        section.extend_from_slice(&1u32.to_le_bytes());
        section.extend_from_slice(&24u32.to_le_bytes());
        section.extend_from_slice(&2u32.to_le_bytes());
        section.extend_from_slice(&(title_offset as u32).to_le_bytes());
        section.extend_from_slice(&codepage_value);
        section.extend_from_slice(&title_value);

        let mut stream = vec![0xFE, 0xFF, 0, 0];
        stream.extend_from_slice(&[0; 20]);
        stream.extend_from_slice(&1u32.to_le_bytes());
        stream.extend_from_slice(&[0; 16]);
        stream.extend_from_slice(&48u32.to_le_bytes());
        stream.extend_from_slice(&section);
        stream
    }

    #[test]
    fn summary_title_decodes_code_page_and_utf16_variants() {
        assert_eq!(
            parse_summary_title(&summary_property_set(&[0xC7, 0xE0, 0xE0], 1251, false)),
            Some("Заа".into())
        );
        assert_eq!(
            parse_summary_title(&summary_property_set("Title".as_bytes(), 1252, true)),
            Some("Title".into())
        );
    }

    #[test]
    fn malformed_summary_offsets_degrade_to_no_title() {
        assert_eq!(parse_summary_title(&[0xFE, 0xFF, 0, 0, 0, 0]), None);
    }
}
