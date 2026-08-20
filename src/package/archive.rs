//! ZIP archive access with decompression limits.

use crate::error::ConvertError;
use crate::package::limits;
use crate::package::xml::{Element, parse_xml_counted};
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
    /// Parsed part trees by normalized name, shared (`Rc`) with callers: a
    /// part referenced many times parses once. Holds up to
    /// `MAX_CACHED_XML_NODES`; past that a part is returned uncached and
    /// re-parses on its next reference.
    trees: HashMap<String, Rc<Element>>,
    /// Nodes currently held in `trees`, against `MAX_CACHED_XML_NODES`.
    cached_nodes: u64,
    /// XML nodes parsed out of this package so far, counting each parse
    /// separately. The tree cache removes the repeats a document actually
    /// has; this bounds the ones it cannot hold, so a package pointing at an
    /// uncacheable part many times still terminates. Bounded by
    /// `MAX_DOCUMENT_XML_NODES`.
    parsed_nodes: u64,
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
        Ok(Package {
            zip,
            total_read: 0,
            cache: HashMap::new(),
            trees: HashMap::new(),
            cached_nodes: 0,
            parsed_nodes: 0,
        })
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
        let mut bytes = Vec::new();
        let read = (&mut file).take(cap + 1).read_to_end(&mut bytes).map_err(|e| {
            ConvertError::Malformed {
                part: Some(name.to_string()),
                detail: format!("corrupt archive entry: {e}"),
            }
        })? as u64;
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
    pub fn optional_xml_part(&mut self, name: &str) -> Result<Option<Rc<Element>>, ConvertError> {
        let Some(bytes) = self.optional_part(name)? else {
            return Ok(None);
        };
        match self.parse_part(name, &bytes) {
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
    pub fn required_xml_part(&mut self, name: &str) -> Result<Rc<Element>, ConvertError> {
        let bytes = self.required_part(name)?;
        self.parse_part(name, &bytes)
    }

    /// Parse one of this package's parts, serving a part already parsed from
    /// the tree cache. Parts reached by relationship (charts, diagrams) come
    /// through here with the bytes the relationship resolved to, so that they
    /// share the cache and the node total with the rest of the package.
    pub fn parse_part(&mut self, name: &str, bytes: &[u8]) -> Result<Rc<Element>, ConvertError> {
        let name = name.trim_start_matches('/');
        if let Some(tree) = self.trees.get(name) {
            return Ok(Rc::clone(tree));
        }
        let before = self.parsed_nodes;
        let tree = Rc::new(parse_xml_counted(bytes, &mut self.parsed_nodes)?);
        let nodes = self.parsed_nodes - before;
        if self.cached_nodes + nodes <= limits::MAX_CACHED_XML_NODES {
            self.cached_nodes += nodes;
            self.trees.insert(name.to_string(), Rc::clone(&tree));
        }
        Ok(tree)
    }
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
    fn a_part_referenced_repeatedly_is_parsed_once() {
        let data = one_part_zip("a.xml", b"<a><b/><b/><b/></a>");
        let mut pkg = Package::open(&data).unwrap();
        let first = pkg.required_xml_part("a.xml").unwrap();
        let per_parse = pkg.parsed_nodes;
        assert!(per_parse > 0);
        for _ in 0..64 {
            let again = pkg.required_xml_part("a.xml").unwrap();
            assert!(Rc::ptr_eq(&first, &again), "a repeat reference must share the parsed tree");
        }
        assert_eq!(pkg.parsed_nodes, per_parse, "a cached tree must not re-parse");
        // The leading slash OPC part URIs carry must not miss the cache.
        assert!(Rc::ptr_eq(&first, &pkg.required_xml_part("/a.xml").unwrap()));
    }

    #[test]
    fn a_part_too_large_to_cache_re_parses_and_hits_the_document_total() {
        let data = one_part_zip("a.xml", b"<a><b/><b/><b/></a>");
        let mut pkg = Package::open(&data).unwrap();
        // A package whose cache is full of earlier parts: this one is served
        // uncached, so every reference to it parses again. The per-part cap
        // resets on each of those parses and never fires; the running total
        // is what stops it.
        pkg.cached_nodes = limits::MAX_CACHED_XML_NODES;
        pkg.required_xml_part("a.xml").unwrap();
        assert!(pkg.trees.is_empty(), "a full cache must not grow past its budget");
        let per_parse = pkg.parsed_nodes;
        pkg.required_xml_part("a.xml").unwrap();
        assert_eq!(pkg.parsed_nodes, per_parse * 2, "an uncached repeat must be charged again");

        pkg.parsed_nodes = limits::MAX_DOCUMENT_XML_NODES;
        let err = pkg.required_xml_part("a.xml").unwrap_err();
        assert!(
            matches!(err, ConvertError::ResourceLimit { limit: "max_document_xml_nodes", .. }),
            "expected max_document_xml_nodes, got: {err}"
        );
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
