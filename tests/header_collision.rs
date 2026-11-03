//! Header detection when a body cell repeats its own column's label. One
//! value equal to the header (`region` under `region`) must not outweigh a
//! typed column under a text label, in any grid format that has no declared
//! header row.

use anydoc::{Format, to_markdown_bytes};
use std::io::{Cursor, Write};

const HEADER: [&str; 3] = ["city", "region", "pop"];
const HEADER_LINE: &str = "| city | region | pop |";

type Row = [&'static str; 3];
type Build = fn(&Row, &[Row]) -> Vec<u8>;

/// (name, header, body rows). Every case has a text label over a numeric
/// `pop` column, so the first row is the header.
const CASES: &[(&str, Row, &[Row])] = &[
    (
        "collision in col A",
        HEADER,
        &[["Paris", "north", "100"], ["city", "south", "200"], ["Nice", "east", "300"]],
    ),
    (
        "collision in col B",
        HEADER,
        &[["Paris", "north", "100"], ["Lyon", "region", "200"], ["Nice", "east", "300"]],
    ),
    (
        "upper case",
        HEADER,
        &[["Paris", "north", "100"], ["Lyon", "REGION", "200"], ["Nice", "east", "300"]],
    ),
    (
        "mixed case",
        HEADER,
        &[["Paris", "north", "100"], ["Lyon", "ReGiOn", "200"], ["Nice", "east", "300"]],
    ),
    (
        "padded",
        HEADER,
        &[["Paris", "north", "100"], ["Lyon", "  region  ", "200"], ["Nice", "east", "300"]],
    ),
    (
        "collision on the first data row",
        HEADER,
        &[["Paris", "region", "100"], ["Lyon", "south", "200"], ["Nice", "east", "300"]],
    ),
    (
        "Status column containing status",
        ["city", "Status", "pop"],
        &[["Paris", "active", "100"], ["Lyon", "status", "200"], ["Nice", "closed", "300"]],
    ),
    (
        "no collision",
        HEADER,
        &[["Paris", "north", "100"], ["Lyon", "south", "200"], ["Nice", "east", "300"]],
    ),
];

fn rows<'a>(header: &'a [&'a str; 3], body: &'a [[&'a str; 3]]) -> Vec<&'a [&'a str; 3]> {
    std::iter::once(header).chain(body.iter()).collect()
}

fn csv(header: &[&str; 3], body: &[[&str; 3]]) -> Vec<u8> {
    let mut out = String::new();
    for row in rows(header, body) {
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out.into_bytes()
}

fn zip(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for (name, body) in parts {
        zip.start_file(*name, opts).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn xlsx(header: &[&str; 3], body: &[[&str; 3]]) -> Vec<u8> {
    const SML: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    let mut data = String::new();
    for (r, row) in rows(header, body).into_iter().enumerate() {
        data.push_str(&format!(r#"<row r="{}">"#, r + 1));
        for (c, value) in row.iter().enumerate() {
            let at = format!("{}{}", (b'A' + c as u8) as char, r + 1);
            if value.parse::<f64>().is_ok() {
                data.push_str(&format!(r#"<c r="{at}"><v>{value}</v></c>"#));
            } else {
                data.push_str(&format!(
                    r#"<c r="{at}" t="inlineStr"><is><t xml:space="preserve">{value}</t></is></c>"#
                ));
            }
        }
        data.push_str("</row>");
    }
    let workbook = format!(
        r#"<?xml version="1.0"?><workbook xmlns="{SML}" xmlns:r="{R}"><sheets><sheet name="S" sheetId="1" r:id="rId1"/></sheets></workbook>"#
    );
    let rels = format!(
        r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#
    );
    let sheet = format!(
        r#"<?xml version="1.0"?><worksheet xmlns="{SML}"><sheetData>{data}</sheetData></worksheet>"#
    );
    zip(&[
        ("xl/workbook.xml", &workbook),
        ("xl/_rels/workbook.xml.rels", &rels),
        ("xl/worksheets/sheet1.xml", &sheet),
    ])
}

fn ods(header: &[&str; 3], body: &[[&str; 3]]) -> Vec<u8> {
    let mut data = String::new();
    for row in rows(header, body) {
        data.push_str("<table:table-row>");
        for value in row {
            if value.parse::<f64>().is_ok() {
                data.push_str(&format!(
                    r#"<table:table-cell office:value-type="float" office:value="{value}"><text:p>{value}</text:p></table:table-cell>"#
                ));
            } else {
                data.push_str(&format!(
                    r#"<table:table-cell office:value-type="string"><text:p>{value}</text:p></table:table-cell>"#
                ));
            }
        }
        data.push_str("</table:table-row>");
    }
    let content = format!(
        r#"<?xml version="1.0"?><office:document-content
        xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
        xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
        xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0">
        <office:body><office:spreadsheet><table:table table:name="S">{data}</table:table></office:spreadsheet></office:body>
        </office:document-content>"#
    );
    zip(&[
        ("mimetype", "application/vnd.oasis.opendocument.spreadsheet"),
        ("content.xml", &content),
    ])
}

/// Every case in every format: the real labels are the header line, and the
/// table holds the header, the separator and each body row exactly once.
#[test]
fn a_label_repeated_in_its_column_keeps_the_header() {
    let builders: [(Format, Build); 3] =
        [(Format::Csv, csv), (Format::Excel, xlsx), (Format::Ods, ods)];
    for (format, build) in builders {
        for (name, header, body) in CASES {
            let md = to_markdown_bytes(&build(header, body), format)
                .unwrap_or_else(|e| panic!("{format:?} {name}: {e}"));
            let expected = format!("| {} |", header.join(" | "));
            if *header == HEADER {
                assert_eq!(expected, HEADER_LINE);
            }
            let table: Vec<&str> = md.lines().filter(|l| l.starts_with('|')).collect();
            assert_eq!(md.lines().next(), Some(expected.as_str()), "{format:?} {name}:\n{md}");
            assert_eq!(table.len(), body.len() + 2, "{format:?} {name}:\n{md}");
        }
    }
}

/// The table without any collision renders exactly as it did before the fix.
#[test]
fn no_collision_is_unchanged() {
    let (_, header, body) = CASES.last().unwrap();
    let md = to_markdown_bytes(&csv(header, body), Format::Csv).unwrap();
    assert_eq!(
        md.trim_end(),
        "| city | region | pop |\n| --- | --- | --- |\n| Paris | north | 100 |\n| Lyon | south | 200 |\n| Nice | east | 300 |"
    );
}
