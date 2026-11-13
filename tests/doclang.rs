//! DocLang (.dclx) export for PDFs, including scanned and mixed pages.

mod common;

use common::fixture_root;
use std::collections::BTreeSet;
use std::io::{Cursor, Read};

/// Unzip a `.dclx` into a name → bytes map.
fn unzip(bytes: &[u8]) -> std::collections::HashMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = std::collections::HashMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        out.insert(f.name().to_string(), buf);
    }
    out
}

fn document_xml(parts: &std::collections::HashMap<String, Vec<u8>>) -> String {
    String::from_utf8(parts["document.xml"].clone()).unwrap()
}

/// Well-formed XML and no unescaped markup bleed (quick-xml parses a full
/// event stream without error).
fn assert_well_formed(xml: &str) {
    let mut reader = quick_xml::Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Eof) => return,
            Ok(_) => {}
            Err(e) => panic!("malformed XML: {e}"),
        }
    }
}

fn assert_opc(parts: &std::collections::HashMap<String, Vec<u8>>) {
    for required in ["[Content_Types].xml", "_rels/.rels", "document.xml"] {
        assert!(parts.contains_key(required), "missing {required}");
    }
    let ct = String::from_utf8(parts["[Content_Types].xml"].clone()).unwrap();
    assert!(ct.contains("application/vnd.doclang.document+xml"));
    let rels = String::from_utf8(parts["_rels/.rels"].clone()).unwrap();
    assert!(rels.contains("http://doclang.ai/ns/package/2026/relationships/document"));
    assert!(rels.contains("Target=\"document.xml\""));
}

#[test]
fn scanned_pdf_exports_pages_as_pictures() {
    let bytes = std::fs::read(fixture_root().join("pdf/handmade-scanned.pdf")).unwrap();
    // The Markdown path refuses this fixture with NeedsOcr; DocLang must not.
    assert!(matches!(
        anydoc::to_markdown_bytes(&bytes, anydoc::Format::Pdf),
        Err(anydoc::ConvertError::NeedsOcr { .. })
    ));

    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Pdf).unwrap();
    let parts = unzip(&dclx);
    assert_opc(&parts);
    assert!(parts.contains_key("source"), "original bytes archived");

    let xml = document_xml(&parts);
    assert_well_formed(&xml);
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(xml.contains("<doclang"));
    assert!(xml.contains("version=\"0.9\""));
    // Two image-only pages → one page_break between them.
    assert_eq!(xml.matches("<page_break/>").count(), 1);
    // Both pages are pictures carrying raster parts.
    assert!(xml.matches("<picture").count() >= 2);
    assert!(xml.contains("full_page_image"));

    // Every <src uri="..."/> resolves to a stored part.
    let srcs: BTreeSet<String> = xml
        .split("<src uri=\"")
        .skip(1)
        .map(|s| s.split('"').next().unwrap().to_string())
        .collect();
    assert!(!srcs.is_empty(), "scanned pages must reference raster parts");
    for uri in &srcs {
        assert!(parts.contains_key(uri), "src {uri} has no archive part");
    }
}

#[test]
fn mixed_pdf_keeps_text_and_pictures() {
    let bytes = std::fs::read(fixture_root().join("pdf/handmade-mixed.pdf")).unwrap();
    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Pdf).unwrap();
    let parts = unzip(&dclx);
    assert_opc(&parts);
    let xml = document_xml(&parts);
    assert_well_formed(&xml);
    assert!(xml.contains("<text>") || xml.contains("<heading"), "mixed page lost its text");
    assert!(xml.contains("<picture"), "mixed page lost its picture");
}

#[test]
fn text_pdf_exports_text_with_locations() {
    let bytes = std::fs::read(fixture_root().join("pdf/text.pdf")).unwrap();
    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Pdf).unwrap();
    let parts = unzip(&dclx);
    assert_opc(&parts);
    let xml = document_xml(&parts);
    assert_well_formed(&xml);
    assert!(xml.contains("<text>"));
    assert!(xml.contains("<location value="), "text runs lost their geometry");
    assert!(xml.contains("<default_resolution"));
    assert!(xml.contains("<page_size"));
    assert!(xml.contains("hash_function=\"sha256sum\""));
}

#[test]
fn non_pdf_formats_reject_doclang_export() {
    let bytes = b"{\\rtf1 plain}".to_vec();
    assert!(matches!(
        anydoc::to_doclang_bytes(&bytes, anydoc::Format::Rtf),
        Err(anydoc::ConvertError::Unsupported(_))
    ));
}
