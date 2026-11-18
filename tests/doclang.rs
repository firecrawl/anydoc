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
        out.insert(f.name().unwrap().to_string(), buf);
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

    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Pdf, None).unwrap();
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
    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Pdf, None).unwrap();
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
    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Pdf, None).unwrap();
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
fn text_pdf_exports_list_structure() {
    let bytes = std::fs::read(fixture_root().join("pdf/text.pdf")).unwrap();
    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Pdf, None).unwrap();
    let xml = document_xml(&unzip(&dclx));
    assert_well_formed(&xml);
    // The fixture's list section: `1.`–`3.` decimal, `a)`/`b)` alpha, `i.`
    // roman, `4.`/`IV.`/`V.` after a paragraph break, and `•`/`◦` bullets.
    assert!(xml.contains("<list"), "list region flattened to text: {xml}");
    assert!(xml.contains("class=\"ordered\""), "numbered list lost its class");
    assert!(xml.contains("<ldiv>"), "list items missing ldiv markers");
    assert!(xml.contains("1.</marker>"));
    assert!(xml.contains("•</marker>"));
    // The interrupting paragraph must not be absorbed into a list.
    let para = xml.find("Interrupting paragraph").unwrap();
    let lists: Vec<usize> = xml.match_indices("<list").map(|(i, _)| i).collect();
    let ends: Vec<usize> = xml.match_indices("</list>").map(|(i, _)| i).collect();
    assert!(lists.iter().zip(&ends).all(|(s, e)| !(para > *s && para < *e + 7)));
}

#[test]
fn lists_hint_off_flattens_lists_to_text() {
    let bytes = std::fs::read(fixture_root().join("pdf/text.pdf")).unwrap();
    let hints = anydoc::DoclangHints { lists: false, ..Default::default() };
    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Pdf, Some(hints)).unwrap();
    let xml = document_xml(&unzip(&dclx));
    assert_well_formed(&xml);
    assert!(!xml.contains("<list"), "lists hint off still emitted <list>");
    assert!(xml.contains("1.First numbered"), "list text lost entirely");
}

/// A one-page PDF with a ruled 2×2 table (header row in Helvetica-Bold)
/// plus a paragraph, built with lopdf.
fn ruled_table_pdf() -> Vec<u8> {
    use lopdf::content::{Content, Operation};
    use lopdf::{Document, Object, Stream, dictionary};

    fn text(ops: &mut Vec<Operation>, font: &str, x: i64, y: i64, s: &str) {
        ops.extend([
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec![font.into(), 10.into()]),
            Operation::new("Tm", vec![1.into(), 0.into(), 0.into(), 1.into(), x.into(), y.into()]),
            Operation::new("Tj", vec![Object::string_literal(s)]),
            Operation::new("ET", vec![]),
        ]);
    }
    fn rule(ops: &mut Vec<Operation>, x0: i64, y0: i64, x1: i64, y1: i64) {
        ops.extend([
            Operation::new("m", vec![x0.into(), y0.into()]),
            Operation::new("l", vec![x1.into(), y1.into()]),
            Operation::new("S", vec![]),
        ]);
    }

    let mut ops = Vec::new();
    text(&mut ops, "F1", 60, 760, "Report");
    // Header cells (bold), then body cells.
    text(&mut ops, "F2", 70, 715, "Name");
    text(&mut ops, "F2", 200, 715, "Age");
    text(&mut ops, "F1", 70, 685, "Ada");
    text(&mut ops, "F1", 200, 685, "36");
    text(&mut ops, "F1", 60, 620, "After the table.");
    // Grid: x boundaries 60/180/300, y 740/710/680.
    for x in [60, 180, 300] {
        rule(&mut ops, x, 680, x, 740);
    }
    for y in [740, 710, 680] {
        rule(&mut ops, 60, y, 300, y);
    }
    let content = Content { operations: ops };

    let mut doc = Document::with_version("1.5");
    // Reserve all ids first — `new_object_id` bumps the counter without
    // inserting, and `add_object` would otherwise collide with a
    // not-yet-inserted forward reference.
    let catalog_id = doc.new_object_id();
    let pages_id = doc.new_object_id();
    let regular = doc.new_object_id();
    let bold = doc.new_object_id();
    let contents_id = doc.new_object_id();
    let page_id = doc.new_object_id();
    doc.objects.insert(
        catalog_id,
        Object::Dictionary(dictionary! { "Type" => "Catalog", "Pages" => pages_id }),
    );
    doc.objects.insert(
        regular,
        Object::Dictionary(
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" },
        ),
    );
    doc.objects.insert(
        bold,
        Object::Dictionary(
            dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica-Bold" },
        ),
    );
    doc.objects.insert(
        contents_id,
        Object::Stream(Stream::new(dictionary! {}, content.encode().unwrap())),
    );
    doc.objects.insert(
        page_id,
        Object::Dictionary(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => contents_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => regular, "F2" => bold } },
        }),
    );
    doc.objects.insert(
        pages_id,
        Object::Dictionary(
            dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 },
        ),
    );
    doc.trailer.set("Root", catalog_id);
    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();
    buf
}

#[test]
fn ruled_table_exports_table_structure() {
    let bytes = ruled_table_pdf();
    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Pdf, None).unwrap();
    let xml = document_xml(&unzip(&dclx));
    assert_well_formed(&xml);
    assert!(xml.contains("<table>"), "ruled grid not detected: {xml}");
    assert!(xml.contains("<ched/>"), "bold header row not marked: {xml}");
    assert!(xml.contains("<fcel/>"));
    assert!(xml.contains("<nl/>"), "rows not separated: {xml}");
    assert!(xml.contains("Ada"), "cell text lost: {xml}");
    assert!(xml.contains("After the table."), "prose lost: {xml}");
}

#[test]
fn pages_with_tables_scopes_ruled_detection() {
    let bytes = ruled_table_pdf();
    // Restrict detection to page 2 — the table is on page 1, so the grid
    // is ignored and its text stays in <text> blocks.
    let hints = anydoc::DoclangHints { pages_with_tables: Some(vec![2]), ..Default::default() };
    let dclx = anydoc::to_doclang_bytes(&bytes, anydoc::Format::Pdf, Some(hints)).unwrap();
    let xml = document_xml(&unzip(&dclx));
    assert_well_formed(&xml);
    assert!(!xml.contains("<table>"), "out-of-scope page still detected: {xml}");
    assert!(xml.contains("Ada"));
}

#[test]
fn non_pdf_formats_reject_doclang_export() {
    let bytes = b"{\\rtf1 plain}".to_vec();
    assert!(matches!(
        anydoc::to_doclang_bytes(&bytes, anydoc::Format::Rtf, None),
        Err(anydoc::ConvertError::Unsupported(_))
    ));
}
