#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::{Cursor, Write};

// The binary workbook part, wrapped in a valid OPC package so mutation
// reaches the record reader instead of dying at the container gate.
fuzz_target!(|data: &[u8]| {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    let parts: [(&str, &[u8]); 3] = [
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", RELS.as_bytes()),
        ("xl/workbook.bin", data),
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

const CONTENT_TYPES: &str = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="bin" ContentType="application/vnd.ms-excel.sheet.binary.macroEnabled.main"/></Types>"#;

const RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.bin"/></Relationships>"#;
