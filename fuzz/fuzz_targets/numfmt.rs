#![no_main]

use libfuzzer_sys::fuzz_target;
use std::io::{Cursor, Write};

// A number format code, carried into the engine on a minimal workbook. The
// grammar is where the parsing complexity lives, and reaching it through a
// discovered styles part would cost the fuzzer most of its budget.
fuzz_target!(|data: &[u8]| {
    let code = String::from_utf8_lossy(data);
    let escaped: String = code
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            c => c.to_string(),
        })
        .collect();
    let styles = format!(
        r#"<?xml version="1.0"?><styleSheet xmlns="{SML}"><numFmts count="1"><numFmt numFmtId="164" formatCode="{escaped}"/></numFmts><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="164" applyNumberFormat="1"/></cellXfs></styleSheet>"#
    );

    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    let parts: [(&str, &str); 6] = [
        ("[Content_Types].xml", CONTENT_TYPES),
        ("_rels/.rels", RELS),
        ("xl/workbook.xml", WORKBOOK),
        ("xl/_rels/workbook.xml.rels", WORKBOOK_RELS),
        ("xl/styles.xml", &styles),
        ("xl/worksheets/sheet1.xml", SHEET),
    ];
    for (name, body) in parts {
        if zip.start_file(name, opts).is_err() || zip.write_all(body.as_bytes()).is_err() {
            return;
        }
    }
    let Ok(bytes) = zip.finish() else {
        return;
    };
    let _ = anydoc::to_markdown_bytes(bytes.into_inner().as_slice(), anydoc::Format::Excel);
});

const SML: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const CONTENT_TYPES: &str = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#;
const RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
const WORKBOOK: &str = r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="S" sheetId="1" r:id="rId1"/></sheets></workbook>"#;
const WORKBOOK_RELS: &str = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
const SHEET: &str = r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="1"><v>1234.5</v></c><c r="B1" s="1"><v>0.075</v></c><c r="C1" s="1" t="inlineStr"><is><t>text</t></is></c></row></sheetData></worksheet>"#;
