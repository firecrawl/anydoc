#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::{Cursor, Write};

// The worksheet, styles and shared-string parts, wrapped in a package whose
// workbook part and relationships are already valid. Fuzzing the workbook
// part instead would strand the input there: without a resolvable worksheet
// relationship every sheet is skipped, and the cell, format and string
// readers never run. The workbook part is parsed by the same record reader
// this does reach.
fuzz_target!(|data: &[u8]| {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    let parts: [(&str, &[u8]); 8] = [
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", RELS.as_bytes()),
        ("xl/workbook.bin", WORKBOOK),
        ("xl/_rels/workbook.bin.rels", WORKBOOK_RELS.as_bytes()),
        ("xl/worksheets/sheet1.bin", data),
        ("xl/worksheets/sheet2.bin", data),
        ("xl/styles.bin", data),
        ("xl/sharedStrings.bin", data),
    ];
    for (name, body) in parts {
        if zip.start_file(name, opts).is_err() || zip.write_all(body).is_err() {
            return;
        }
    }
    let Ok(bytes) = zip.finish() else {
        return;
    };
    let _ = anydoc::to_markdown_bytes(bytes.into_inner().as_slice(), anydoc::Format::Excel);
});

const WORKBOOK: &[u8] = include_bytes!("../seeds/xlsb/workbook-bin");

const CONTENT_TYPES: &str = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="bin" ContentType="application/vnd.ms-excel.sheet.binary.macroEnabled.main"/></Types>"#;
const RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.bin"/></Relationships>"#;
const WORKBOOK_RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.bin"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.bin"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.bin"/><Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.bin"/></Relationships>"#;
