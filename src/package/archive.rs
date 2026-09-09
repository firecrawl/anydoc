//! ZIP archive access with decompression limits.

use crate::error::ConvertError;
use crate::package::limits;
use crate::package::xml::{Element, parse_xml};
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::rc::Rc;

/// A ZIP-based document package (OOXML, ODF, EPUB).
pub struct Package<'a> {
    zip: zip::ZipArchive<Cursor<&'a [u8]>>,
    total_read: u64,
    /// Decompressed parts by normalized name: repeated references are served
    /// from the cache instead of re-decompressing and re-charging the
    /// total-bytes budget (which would falsely trip on valid documents that
    /// reference one part many times). Bounded by `MAX_TOTAL_BYTES`. Buffers
    /// are shared (`Rc`), so a cache hit never copies the bytes.
    cache: HashMap<String, Rc<[u8]>>,
}

impl<'a> Package<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Self, ConvertError> {
        let zip = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|e| ConvertError::malformed(format!("not a readable zip archive: {e}")))?;
        if zip.len() > limits::MAX_ENTRY_COUNT {
            return Err(ConvertError::ResourceLimit {
                limit: "max_entry_count",
                detail: format!("archive contains {} entries", zip.len()),
            });
        }
        Ok(Package { zip, total_read: 0, cache: HashMap::new() })
    }

    /// Read a part's bytes. `Ok(None)` means the part is absent (a valid
    /// state for optional parts); `Err` means it exists but cannot be read.
    /// Callers apply the unified policy: skip + log when useful output
    /// remains, propagate when the part is the primary content.
    pub fn part(&mut self, name: &str) -> Result<Option<Rc<[u8]>>, ConvertError> {
        // OPC part URIs may carry a leading slash; entries never do.
        let name = name.trim_start_matches('/');
        if let Some(bytes) = self.cache.get(name) {
            return Ok(Some(Rc::clone(bytes)));
        }
        let mut file = match self.zip.by_name(name) {
            Ok(f) => f,
            Err(zip::result::ZipError::FileNotFound) => return Ok(None),
            Err(e) => {
                return Err(ConvertError::Malformed {
                    part: Some(name.to_string()),
                    detail: format!("unreadable archive entry: {e}"),
                });
            }
        };
        if file.size() > limits::MAX_ENTRY_BYTES {
            return Err(ConvertError::ResourceLimit {
                limit: "max_entry_bytes",
                detail: format!("{name} declares {} decompressed bytes", file.size()),
            });
        }
        // The declared size can lie; read through a hard-capped reader. The
        // cap is whichever budget has less room: the per-entry limit or what
        // remains of the whole-archive total.
        let remaining_total = limits::MAX_TOTAL_BYTES.saturating_sub(self.total_read);
        let cap = limits::MAX_ENTRY_BYTES.min(remaining_total);
        let bytes = read_part_bytes((&mut file).take(cap + 1), name)?;
        let read = bytes.len() as u64;
        if read > cap {
            return Err(if remaining_total < limits::MAX_ENTRY_BYTES {
                ConvertError::ResourceLimit {
                    limit: "max_total_bytes",
                    detail: format!("{name} exceeds the archive's remaining decompression budget"),
                }
            } else {
                ConvertError::ResourceLimit {
                    limit: "max_entry_bytes",
                    detail: format!("{name} exceeds the decompression cap"),
                }
            });
        }
        self.total_read += read;
        let bytes: Rc<[u8]> = Rc::from(bytes);
        self.cache.insert(name.to_string(), Rc::clone(&bytes));
        Ok(Some(bytes))
    }

    /// Read a part that must exist for any meaningful output.
    /// True when a part exists, without reading (or budget-charging) it.
    pub fn has_part(&self, name: &str) -> bool {
        self.zip.index_for_name(name.trim_start_matches('/')).is_some()
    }

    pub fn required_part(&mut self, name: &str) -> Result<Rc<[u8]>, ConvertError> {
        self.part(name)?.ok_or_else(|| ConvertError::MissingPart { part: name.to_string() })
    }

    /// Read an optional part under the unified recovery policy: absent is a
    /// valid state (`Ok(None)`, silent); an unreadable part is skipped with a
    /// log (`Ok(None)`); fatal resource-limit errors always propagate.
    pub fn optional_part(&mut self, name: &str) -> Result<Option<Rc<[u8]>>, ConvertError> {
        match self.part(name) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.is_fatal() => Err(e),
            Err(e) => {
                log::warn!("skipping unreadable part {name}: {e}");
                Ok(None)
            }
        }
    }

    /// Read and parse an optional XML part under the unified recovery policy:
    /// absent -> `Ok(None)`; unreadable or corrupt -> skipped with a log;
    /// fatal resource-limit errors always propagate.
    pub fn optional_xml_part(&mut self, name: &str) -> Result<Option<Element>, ConvertError> {
        let Some(bytes) = self.optional_part(name)? else {
            return Ok(None);
        };
        match parse_xml(&bytes) {
            Ok(tree) => Ok(Some(tree)),
            Err(e) if e.is_fatal() => Err(e),
            Err(e) => {
                log::warn!("skipping corrupt part {name}: {e}");
                Ok(None)
            }
        }
    }

    /// Read and parse an XML part that must exist and parse for any
    /// meaningful output.
    pub fn required_xml_part(&mut self, name: &str) -> Result<Element, ConvertError> {
        let bytes = self.required_part(name)?;
        parse_xml(&bytes)
    }
}

/// Read an already capped entry before committing it to the package cache.
fn read_part_bytes(mut reader: impl Read, name: &str) -> Result<Vec<u8>, ConvertError> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).map_err(|e| match e.kind() {
        std::io::ErrorKind::OutOfMemory => ConvertError::ResourceLimit {
            limit: "memory_allocation",
            detail: format!("allocation failed while reading {name}: {e}"),
        },
        _ => ConvertError::Malformed {
            part: Some(name.to_string()),
            detail: format!("corrupt archive entry: {e}"),
        },
    })?;
    Ok(bytes)
}

/// A zip-open failure on OOXML input may actually be an OLE compound file:
/// an encrypted package, or a legacy binary document with the wrong
/// extension.
pub fn probe_ole(bytes: &[u8]) -> Option<ConvertError> {
    const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
    if !bytes.starts_with(&OLE_MAGIC) {
        return None;
    }
    let cursor = Cursor::new(bytes);
    if let Ok(file) = cfb::CompoundFile::open(cursor)
        && (file.exists("EncryptionInfo") || file.exists("EncryptedPackage"))
    {
        return Some(ConvertError::Encrypted);
    }
    Some(ConvertError::malformed(
        "OLE compound document where an OOXML package was expected (legacy binary format?)",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn one_part_zip(name: &str, bytes: &[u8]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(bytes).unwrap();
        w.finish().unwrap().into_inner()
    }

    struct FailingReader(std::io::ErrorKind);

    impl Read for FailingReader {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(self.0.into())
        }
    }

    #[test]
    fn allocation_read_errors_are_fatal_resource_limits() {
        for prefix in [b"".as_slice(), b"partial content".as_slice()] {
            let reader = Cursor::new(prefix).chain(FailingReader(std::io::ErrorKind::OutOfMemory));
            let err = read_part_bytes(reader, "word/document.xml").unwrap_err();
            assert!(
                matches!(
                    &err,
                    ConvertError::ResourceLimit { limit: "memory_allocation", detail }
                        if detail.contains("word/document.xml")
                ),
                "expected an allocation resource limit, got: {err}"
            );
            assert_eq!(err.code(), "resourceLimit");
            assert!(err.is_fatal(), "optional parts must not swallow allocation failures");
        }
    }

    #[test]
    fn invalid_data_read_errors_remain_malformed() {
        for prefix in [b"".as_slice(), b"partial content".as_slice()] {
            let reader = Cursor::new(prefix).chain(FailingReader(std::io::ErrorKind::InvalidData));
            let err = read_part_bytes(reader, "word/document.xml").unwrap_err();
            assert!(matches!(
                &err,
                ConvertError::Malformed { part: Some(part), detail }
                    if part == "word/document.xml"
                        && detail == &format!("corrupt archive entry: {}",
                            std::io::Error::from(std::io::ErrorKind::InvalidData))
            ));
            assert_eq!(err.code(), "malformed");
            assert!(!err.is_fatal());
        }
    }

    #[test]
    fn corrupt_entry_is_not_cached_or_charged() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(b"<document/>").unwrap();
        let mut data = writer.finish().unwrap().into_inner();
        let start = zip::ZipArchive::new(Cursor::new(data.as_slice()))
            .unwrap()
            .by_name("word/document.xml")
            .unwrap()
            .data_start()
            .unwrap() as usize;
        data[start] ^= 1; // Change the payload without updating its CRC.
        let mut pkg = Package::open(&data).unwrap();
        let err = pkg.part("word/document.xml").unwrap_err();
        assert!(matches!(&err, ConvertError::Malformed { part: Some(part), detail }
            if part == "word/document.xml" && detail.starts_with("corrupt archive entry: ")));
        assert!(!err.is_fatal());
        assert!(pkg.cache.is_empty());
        assert_eq!(pkg.total_read, 0);
        assert!(pkg.optional_part("word/document.xml").unwrap().is_none());
        assert!(pkg.cache.is_empty());
        assert_eq!(pkg.total_read, 0);
    }

    #[test]
    fn repeated_reads_are_cached_and_charged_once() {
        let data = one_part_zip("media/a.bin", &[7u8; 4096]);
        let mut pkg = Package::open(&data).unwrap();
        for _ in 0..5 {
            assert_eq!(pkg.part("media/a.bin").unwrap().unwrap().len(), 4096);
        }
        assert_eq!(pkg.total_read, 4096, "repeated reads must not re-charge the budget");
    }

    #[test]
    fn total_budget_exhaustion_reports_max_total_bytes() {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for name in ["a.bin", "b.bin"] {
            w.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(&[7u8; 4096]).unwrap();
        }
        let data = w.finish().unwrap().into_inner();
        let mut pkg = Package::open(&data).unwrap();
        assert!(pkg.part("a.bin").unwrap().is_some());
        // Simulate a large archive having consumed almost the whole total
        // budget across earlier entries; the next entry no longer fits.
        pkg.total_read = limits::MAX_TOTAL_BYTES - 100;
        let err = pkg.part("b.bin").unwrap_err();
        assert!(
            matches!(err, ConvertError::ResourceLimit { limit: "max_total_bytes", .. }),
            "expected max_total_bytes, got: {err}"
        );
    }

    #[test]
    fn leading_slash_part_names_normalize() {
        let data = one_part_zip("word/document.xml", b"<x/>");
        let mut pkg = Package::open(&data).unwrap();
        assert!(pkg.part("/word/document.xml").unwrap().is_some());
    }
}
