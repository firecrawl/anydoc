//! DocLang (.dclx) export for the document-model formats (everything but
//! PDF, which has its own positioned exporter in `doclang.rs`).

mod common;

use common::fixture_root;
use std::collections::HashMap;
use std::io::{Cursor, Read};

/// Unzip a `.dclx` into a name → bytes map.
fn unzip(bytes: &[u8]) -> HashMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = HashMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        out.insert(f.name().unwrap().to_string(), buf);
    }
    out
}

fn document_xml(parts: &HashMap<String, Vec<u8>>) -> String {
    String::from_utf8(parts["document.xml"].clone()).unwrap()
}

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

fn assert_opc(parts: &HashMap<String, Vec<u8>>) {
    for required in ["[Content_Types].xml", "_rels/.rels", "document.xml", "source"] {
        assert!(parts.contains_key(required), "missing {required}");
    }
    let ct = String::from_utf8(parts["[Content_Types].xml"].clone()).unwrap();
    assert!(ct.contains("application/vnd.doclang.document+xml"));
    let rels = String::from_utf8(parts["_rels/.rels"].clone()).unwrap();
    assert!(rels.contains("http://doclang.ai/ns/package/2026/relationships/document"));
    assert!(rels.contains("http://doclang.ai/ns/package/2026/relationships/source"));
}

/// Every `<src uri="...">` that is a relative reference resolves to a
/// stored archive part.
fn assert_srcs_resolve(parts: &HashMap<String, Vec<u8>>, xml: &str) {
    for src in xml.split("<src uri=\"").skip(1) {
        let uri = src.split('"').next().unwrap();
        if uri.contains("://") {
            continue;
        }
        assert!(parts.contains_key(uri), "src {uri} has no archive part");
    }
}

#[test]
fn docx_exports_structure() {
    let bytes = std::fs::read(fixture_root().join("docx/text.docx")).unwrap();
    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Docx, None).unwrap();
    let parts = unzip(&dclx);
    assert_opc(&parts);

    let xml = document_xml(&parts);
    assert_well_formed(&xml);
    assert!(xml.contains("<heading level=\"1\">"), "{xml}");
    assert!(xml.contains("<bold>"), "inline styling lost: {xml}");
    assert!(xml.contains("<list"), "lists lost: {xml}");
    assert!(xml.contains("<table>"), "tables lost: {xml}");
    assert!(xml.contains("<lcel/>") || xml.contains("<ucel/"), "spans lost: {xml}");
    assert!(xml.contains("<href uri=\"https://example.com/page\"/>"), "links lost: {xml}");
    assert!(xml.contains("<footnote>"), "footnote lost: {xml}");
    assert!(xml.contains("<label value=\"endnote\"/>"), "endnote marker lost: {xml}");
    assert!(xml.contains("<thread thread_id="), "anchors lost: {xml}");
    assert!(xml.contains("<xref thread_id="), "cross-references lost: {xml}");
    assert!(xml.contains("<picture>"), "image lost: {xml}");
    assert_srcs_resolve(&parts, &xml);

    // The original .docx travels as /source with its own content type.
    assert_eq!(parts["source"], bytes);
    let ct = String::from_utf8(parts["[Content_Types].xml"].clone()).unwrap();
    assert!(ct.contains("wordprocessingml.document"), "{ct}");
}

#[test]
fn docx_assets_deduplicate_into_parts() {
    let bytes = std::fs::read(fixture_root().join("docx/handmade-manyrefs.docx")).unwrap();
    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Docx, None).unwrap();
    let parts = unzip(&dclx);
    let xml = document_xml(&parts);
    assert_well_formed(&xml);
    // Seventy references to one image → one assets/ part.
    let asset_parts: Vec<_> =
        parts.keys().filter(|k| k.starts_with("assets/")).collect();
    assert_eq!(asset_parts.len(), 1, "{asset_parts:?}");
    assert!(xml.matches("<picture>").count() >= 70);
}

#[test]
fn csv_exports_a_table() {
    let bytes = std::fs::read(fixture_root().join("csv/sheet.csv")).unwrap();
    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Csv, None).unwrap();
    let parts = unzip(&dclx);
    assert_opc(&parts);
    let xml = document_xml(&parts);
    assert_well_formed(&xml);
    assert!(xml.contains("<table>"), "{xml}");
    assert!(xml.contains("<fcel/>"), "{xml}");
    assert!(xml.contains("<nl/>"), "{xml}");
    let ct = String::from_utf8(parts["[Content_Types].xml"].clone()).unwrap();
    assert!(ct.contains("text/csv"), "{ct}");
}

#[test]
fn spreadsheet_exports_sheets_as_headings_and_tables() {
    let bytes = std::fs::read(fixture_root().join("xlsx/sheet.xlsx")).unwrap();
    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Excel, None).unwrap();
    let parts = unzip(&dclx);
    assert_opc(&parts);
    let xml = document_xml(&parts);
    assert_well_formed(&xml);
    assert!(xml.contains("<heading level=\"2\">"), "sheet names lost: {xml}");
    assert!(xml.contains("<table>"), "{xml}");
}

#[test]
fn math_exports_formula_elements() {
    let bytes = std::fs::read(fixture_root().join("docx/handmade-math.docx")).unwrap();
    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Docx, None).unwrap();
    let xml = document_xml(&unzip(&dclx));
    assert_well_formed(&xml);
    assert!(xml.contains("<formula>"), "{xml}");
    // Delimiters must be stripped from math, but literal $ signs in plain
    // text are legitimate.
    for f in xml.split("<formula>").skip(1) {
        let inner = f.split("</formula>").next().unwrap();
        assert!(!inner.contains('$'), "delimiter leaked into {inner}");
    }
}

/// Smoke test: every remaining document-model format produces a valid
/// archive. Presentation and text formats carry the same model, so a
/// well-formed document with body content is the contract.
#[test]
fn all_model_formats_export_valid_archives() {
    let cases: &[(&str, anydoc::Format)] = &[
        ("doc/text.doc", anydoc::Format::Doc),
        ("odt/text.odt", anydoc::Format::Odt),
        ("odt/handmade-lists.odt", anydoc::Format::Odt),
        ("rtf/text.rtf", anydoc::Format::Rtf),
        ("epub/book.epub", anydoc::Format::Epub),
        ("ppt/pres.ppt", anydoc::Format::Ppt),
        ("pptx/pres.pptx", anydoc::Format::Pptx),
        ("xls/sheet.xls", anydoc::Format::Excel),
        ("xlsb/handmade-sheet.xlsb", anydoc::Format::Excel),
        ("ods/sheet.ods", anydoc::Format::Ods),
        ("odp/pres.odp", anydoc::Format::Odp),
    ];
    for (path, format) in cases {
        let bytes = std::fs::read(fixture_root().join(path)).unwrap();
        let dclx = anydoc::to_doclang_bytes(&bytes, *format, None)
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        let parts = unzip(&dclx);
        assert_opc(&parts);
        let xml = document_xml(&parts);
        assert_well_formed(&xml);
        assert!(xml.contains("<text>") || xml.contains("<heading") || xml.contains("<table"),
            "{path} produced an empty body: {xml}");
        assert_srcs_resolve(&parts, &xml);
    }
}

#[test]
fn path_api_exports_doclang() {
    let path = fixture_root().join("docx/text.docx");
    let dclx = anydoc::to_doclang(&path, None).unwrap();
    let xml = document_xml(&unzip(&dclx));
    assert_well_formed(&xml);
    assert!(xml.contains("<doclang"));
}

#[test]
fn unrecognized_content_still_rejects_doclang_export() {
    assert!(matches!(
        anydoc::to_doclang_bytes(b"definitely not a document", None, None),
        Err(anydoc::ConvertError::Unsupported(_))
    ));
}
