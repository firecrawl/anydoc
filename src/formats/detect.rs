//! Content-based format detection.
//!
//! Identifies the format from what each specification designates as the
//! container's identity, never from heuristics over document content:
//!
//! - PDF: the `%PDF-` header (ISO 32000; implementations accept leading
//!   junk, bounded here at 1024 bytes).
//! - RTF: the `{\rtf` group that must open every RTF file.
//! - OLE compound files: the [MS-CFB] signature, then the stream name each
//!   binary Office format mandates ([MS-DOC] `WordDocument`, [MS-PPT]
//!   `PowerPoint Document`, [MS-XLS] `Workbook`/`Book`).
//! - ZIP packages: the local-file-header signature, then the package's own
//!   identity: the `mimetype` part (ODF/EPUB OCF), or for OPC the content
//!   type of the part the package-level officeDocument relationship
//!   designates as the main document (with the main part's mandated root
//!   element as the authority when content types are stale or generic).
//! - Raster images: the signature each codec mandates (PNG, JPEG, WebP,
//!   TIFF, BMP). They are checked last, after every container, and all map
//!   to one format: the decoder identifies the codec itself.
//!
//! Plain-text formats (CSV) carry no signature and are never detected;
//! callers fall back to the file extension. Detection never errors: any
//! unreadable or ambiguous container yields `None` and the caller's
//! fallback (extension, then the frontend's own error) applies.

use crate::Format;
use crate::package::Package;
use crate::package::relationships::{read_rels, rel_type};
use crate::package::xml::{Element, ns};
use std::io::Cursor;

const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const CT_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const MANIFEST_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0";
const SPREADSHEET_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

pub(crate) fn from_bytes(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(b"{\\rtf") {
        return Some(Format::Rtf);
    }
    if bytes.starts_with(&OLE_MAGIC) {
        return detect_ole(bytes);
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return detect_zip(bytes);
    }
    if bytes[..bytes.len().min(1024)].windows(5).any(|w| w == b"%PDF-") {
        return Some(Format::Pdf);
    }
    detect_image(bytes)
}

/// Raster image signatures, checked after every container. They share one
/// format: which codec it is only matters to the decoder. Detection runs
/// ahead of the extension fallback, so a signature that also matches
/// ordinary text would take formats like CSV away from it.
fn detect_image(bytes: &[u8]) -> Option<Format> {
    let is_image = bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"))
        || bytes.starts_with(b"II*\0")
        || bytes.starts_with(b"MM\0*")
        || is_bmp(bytes);
    is_image.then_some(Format::Image)
}

/// `BM` alone is two ordinary letters, and text files do start with them
/// (`BMI,weight,height`), so the DIB header behind it has to be one of the
/// sizes the format specifies before this claims a bitmap.
fn is_bmp(bytes: &[u8]) -> bool {
    const DIB_HEADER_SIZES: [u32; 7] = [12, 40, 52, 56, 64, 108, 124];

    bytes.starts_with(b"BM")
        && bytes
            .get(14..18)
            .and_then(|size| size.try_into().ok())
            .map(u32::from_le_bytes)
            .is_some_and(|size| DIB_HEADER_SIZES.contains(&size))
}

/// Classify an OLE compound file by its mandated content stream. Encrypted
/// OOXML packages (`EncryptedPackage`) stay `None`: the inner format is
/// unknowable, and the frontend reports `Encrypted` precisely.
fn detect_ole(bytes: &[u8]) -> Option<Format> {
    let ole = cfb::CompoundFile::open(Cursor::new(bytes)).ok()?;
    // Stream-name comparison is case-insensitive, matching CFB's own
    // uppercase name comparisons; producers vary (`WORKBOOK`, `BOOK`).
    let mut found = None;
    for entry in ole.read_root_storage() {
        let name = entry.name();
        if name.eq_ignore_ascii_case("WordDocument") {
            found = Some(Format::Doc);
        } else if name.eq_ignore_ascii_case("PowerPoint Document") {
            found = Some(Format::Ppt);
        } else if name.eq_ignore_ascii_case("Workbook") || name.eq_ignore_ascii_case("Book") {
            found = Some(Format::Excel);
        }
        if found.is_some() {
            break;
        }
    }
    found
}

fn detect_zip(bytes: &[u8]) -> Option<Format> {
    let mut pkg = Package::open(bytes).ok()?;

    // ODF and EPUB designate the `mimetype` part as the package identity.
    if let Ok(Some(mime)) = pkg.optional_part("mimetype") {
        let mime = std::str::from_utf8(&mime).ok()?.trim();
        return mimetype_format(mime);
    }

    // OPC: the package-level officeDocument relationship designates the
    // main part; its content type names the document kind.
    if let Ok(rels) = read_rels(&mut pkg, "_rels/.rels")
        && let Some(rel) = rels.first_of_type(rel_type::OFFICE_DOCUMENT)
        && let Ok(target) = crate::package::path::resolve("", &rel.target)
    {
        if let Ok(Some(types)) = pkg.optional_xml_part("[Content_Types].xml")
            && let Some(ct) = content_type_of(&types, &target.path)
            && let Some(format) = opc_format(&ct)
        {
            return Some(format);
        }
        // Content types can be stale or generic; the main part's mandated
        // root element (`w:document`, `p:presentation`, `workbook`) is the
        // next authority.
        if let Ok(Some(tree)) = pkg.optional_xml_part(&target.path)
            && let Some(format) = tree.child_elems().next().and_then(root_element_format)
        {
            return Some(format);
        }
        // Binary main parts (`xl/workbook.bin`) and unreadable ones: the
        // conventional locations the frontends also fall back to.
        return opc_format_by_path(&target.path);
    }

    // Packages with no usable rels: conventional main-part locations.
    for (part, format) in [
        ("word/document.xml", Format::Docx),
        ("ppt/presentation.xml", Format::Pptx),
        ("xl/workbook.xml", Format::Excel),
        ("xl/workbook.bin", Format::Excel),
    ] {
        if pkg.has_part(part) {
            return Some(format);
        }
    }

    // ODF without its mandatory `mimetype`: the manifest's root file entry
    // carries the same media type.
    if let Ok(Some(manifest)) = pkg.optional_xml_part("META-INF/manifest.xml")
        && let Some(root) = manifest
            .descendants(MANIFEST_NS, "file-entry")
            .find(|e| e.attr(MANIFEST_NS, "full-path") == Some("/"))
        && let Some(mime) = root.attr(MANIFEST_NS, "media-type")
    {
        return mimetype_format(mime.trim());
    }

    // EPUB without its mandatory `mimetype`: the OCF container descriptor.
    if pkg.has_part("META-INF/container.xml") {
        return Some(Format::Epub);
    }

    None
}

fn mimetype_format(mime: &str) -> Option<Format> {
    // `-template` variants share the base format's parser.
    let mime = mime.strip_suffix("-template").unwrap_or(mime);
    Some(match mime {
        "application/epub+zip" => Format::Epub,
        "application/vnd.oasis.opendocument.text" => Format::Odt,
        "application/vnd.oasis.opendocument.spreadsheet" => Format::Ods,
        "application/vnd.oasis.opendocument.presentation" => Format::Odp,
        _ => return None,
    })
}

/// Resolve a part's content type per OPC: an `Override` for the exact part
/// name wins, else the `Default` for its extension.
fn content_type_of(types: &Element, part: &str) -> Option<String> {
    let part_name = format!("/{part}");
    if let Some(ct) = types
        .descendants(CT_NS, "Override")
        .find(|e| e.attr_any("PartName").is_some_and(|p| p.eq_ignore_ascii_case(&part_name)))
        .and_then(|e| e.attr_any("ContentType"))
    {
        return Some(ct.to_string());
    }
    let ext = part.rsplit_once('.').map(|(_, e)| e)?;
    types
        .descendants(CT_NS, "Default")
        .find(|e| e.attr_any("Extension").is_some_and(|x| x.eq_ignore_ascii_case(ext)))
        .and_then(|e| e.attr_any("ContentType"))
        .map(str::to_string)
}

/// Map a main-part content type onto its parser. The family segment covers
/// every variant (document, template, macro-enabled, slideshow); `ms-excel`
/// covers the binary `.xlsb` main part.
fn opc_format(content_type: &str) -> Option<Format> {
    let ct = content_type.to_ascii_lowercase();
    Some(if ct.contains("wordprocessingml") {
        Format::Docx
    } else if ct.contains("presentationml") {
        Format::Pptx
    } else if ct.contains("spreadsheetml") || ct.contains("ms-excel") {
        Format::Excel
    } else {
        return None;
    })
}

/// Each OOXML main part has a mandated root element; its namespace (already
/// normalized from Strict to Transitional at parse time) names the format.
fn root_element_format(root: &Element) -> Option<Format> {
    match root.ns.as_deref() {
        Some(ns::W) => Some(Format::Docx),
        Some(ns::P) => Some(Format::Pptx),
        Some(SPREADSHEET_NS) => Some(Format::Excel),
        _ => None,
    }
}

fn opc_format_by_path(part: &str) -> Option<Format> {
    Some(if part.starts_with("word/") {
        Format::Docx
    } else if part.starts_with("ppt/") {
        Format::Pptx
    } else if part.starts_with("xl/") {
        Format::Excel
    } else {
        return None;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in parts {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn ole_of(streams: &[&str]) -> Vec<u8> {
        let mut ole = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        for name in streams {
            ole.create_stream(name).unwrap().write_all(b"x").unwrap();
        }
        ole.into_inner().into_inner()
    }

    #[test]
    fn signatures() {
        assert_eq!(from_bytes(b"%PDF-1.7\n"), Some(Format::Pdf));
        // Leading junk before the header is accepted, bounded.
        let mut junk = vec![b' '; 500];
        junk.extend_from_slice(b"%PDF-1.4");
        assert_eq!(from_bytes(&junk), Some(Format::Pdf));
        assert_eq!(from_bytes(b"{\\rtf1\\ansi hi}"), Some(Format::Rtf));
        assert_eq!(from_bytes(b"a,b,c\n1,2,3\n"), None);
        assert_eq!(from_bytes(b""), None);
    }

    /// `BM`, then a file header and a DIB header of `dib_size` bytes.
    fn bmp_of(dib_size: u32) -> Vec<u8> {
        let mut bytes = vec![0_u8; 14];
        bytes[..2].copy_from_slice(b"BM");
        bytes.extend_from_slice(&dib_size.to_le_bytes());
        bytes.resize(14 + dib_size as usize, 0);
        bytes
    }

    #[test]
    fn image_codecs_are_identified_by_their_signature() {
        for signature in [
            b"\x89PNG\r\n\x1a\n".as_slice(),
            &[0xFF, 0xD8, 0xFF, 0xE0],
            b"RIFF\x24\0\0\0WEBPVP8 ",
            b"II*\0\x08\0\0\0",
            b"MM\0*\0\0\0\x08",
            &bmp_of(40),
            &bmp_of(124),
        ] {
            assert_eq!(from_bytes(signature), Some(Format::Image), "{signature:?}");
        }
    }

    #[test]
    fn near_misses_are_not_images() {
        // `BM` needs a DIB header of a specified size behind it. A CSV whose
        // first column is BMI used to convert and has to keep converting.
        assert_eq!(from_bytes(b"BMI,weight,height\n22.5,70,1.76\n21.0,65,1.76\n"), None);
        assert_eq!(from_bytes(&bmp_of(41)), None);
        assert_eq!(from_bytes(b"BM"), None);
        // RIFF containers that are not WebP (this one is a WAV).
        assert_eq!(from_bytes(b"RIFF\x24\0\0\0WAVEfmt "), None);
        assert_eq!(from_bytes(b"RIFF"), None);
        // One byte off each remaining signature.
        assert_eq!(from_bytes(b"\x89PNG\r\n\x1a\x00"), None);
        assert_eq!(from_bytes(&[0xFF, 0xD8, 0xFE, 0xE0]), None);
        assert_eq!(from_bytes(b"II\0*\x08\0\0\0"), None);
        assert_eq!(from_bytes(b"MM*\0\0\0\0\x08"), None);
    }

    #[test]
    fn container_signature_wins_over_an_early_embedded_pdf() {
        let pkg = zip_of(&[("embedded.pdf", b"%PDF-1.7\n"), ("word/document.xml", b"<document/>")]);
        assert_eq!(from_bytes(&pkg), Some(Format::Docx));
    }

    #[test]
    fn ole_streams_designate_the_binary_format() {
        assert_eq!(from_bytes(&ole_of(&["WordDocument"])), Some(Format::Doc));
        assert_eq!(from_bytes(&ole_of(&["PowerPoint Document"])), Some(Format::Ppt));
        assert_eq!(from_bytes(&ole_of(&["Workbook"])), Some(Format::Excel));
        assert_eq!(from_bytes(&ole_of(&["BOOK"])), Some(Format::Excel));
        // Encrypted OOXML: inner format unknowable, extension decides.
        assert_eq!(from_bytes(&ole_of(&["EncryptedPackage", "EncryptionInfo"])), None);
    }

    #[test]
    fn mimetype_identifies_odf_and_epub() {
        for (mime, format) in [
            ("application/vnd.oasis.opendocument.text", Format::Odt),
            ("application/vnd.oasis.opendocument.text-template", Format::Odt),
            ("application/vnd.oasis.opendocument.spreadsheet", Format::Ods),
            ("application/vnd.oasis.opendocument.presentation", Format::Odp),
            ("application/epub+zip", Format::Epub),
        ] {
            assert_eq!(from_bytes(&zip_of(&[("mimetype", mime.as_bytes())])), Some(format));
        }
        assert_eq!(from_bytes(&zip_of(&[("mimetype", b"application/zip")])), None);
    }

    #[test]
    fn opc_main_part_content_type_wins_over_its_path() {
        // Main part at a nonconventional path; only the content type says
        // this is a presentation.
        let rels = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="/custom/main.xml"/>
        </Relationships>"#;
        let types = br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
            <Override PartName="/custom/main.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
        </Types>"#;
        let pkg = zip_of(&[
            ("_rels/.rels", rels),
            ("[Content_Types].xml", types),
            ("custom/main.xml", b"<p/>"),
        ]);
        assert_eq!(from_bytes(&pkg), Some(Format::Pptx));
    }

    #[test]
    fn opc_stale_content_types_defer_to_the_root_element() {
        // The Override names a part that does not exist; the real main part
        // only gets the generic Default. Its w:document root decides.
        let rels = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="content/main.xml"/>
        </Relationships>"#;
        let types = br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
            <Default Extension="xml" ContentType="application/xml"/>
            <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
        </Types>"#;
        let main = br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body/></w:document>"#;
        let pkg = zip_of(&[
            ("_rels/.rels", rels),
            ("[Content_Types].xml", types),
            ("content/main.xml", main),
        ]);
        assert_eq!(from_bytes(&pkg), Some(Format::Docx));
    }

    #[test]
    fn opc_falls_back_to_conventional_paths() {
        // No content types: the main-part path decides.
        let rels = br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
        </Relationships>"#;
        let pkg = zip_of(&[("_rels/.rels", rels), ("word/document.xml", b"<d/>")]);
        assert_eq!(from_bytes(&pkg), Some(Format::Docx));

        // No rels at all: conventional part existence decides.
        let pkg = zip_of(&[("xl/workbook.bin", b"\0")]);
        assert_eq!(from_bytes(&pkg), Some(Format::Excel));
    }

    #[test]
    fn broken_odf_and_epub_fall_back_to_manifest_and_container() {
        let manifest = br#"<manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0">
            <manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/>
        </manifest:manifest>"#;
        let pkg = zip_of(&[("META-INF/manifest.xml", manifest), ("content.xml", b"<c/>")]);
        assert_eq!(from_bytes(&pkg), Some(Format::Ods));

        let pkg = zip_of(&[("META-INF/container.xml", b"<container/>")]);
        assert_eq!(from_bytes(&pkg), Some(Format::Epub));

        // A plain zip is not a document.
        assert_eq!(from_bytes(&zip_of(&[("readme.txt", b"hi")])), None);
    }
}
