use anydoc_henry_mvp::{ConversionError, convert_file, summarize_markdown};
use std::fs;

#[test]
fn counts_markdown_structure_without_copying_content() {
    let stats = summarize_markdown("# Title\n\n- one\n- two\n\n| A | B |\n| - | - |\n| 1 | 2 |\n");
    assert_eq!(stats.headings, 1);
    assert_eq!(stats.list_items, 2);
    assert_eq!(stats.table_rows, 3);
    assert!(stats.markdown_chars > 20);
}

#[test]
fn report_json_omits_paths_and_markdown() {
    let report = anydoc_henry_mvp::ConversionReport::fixture_for_test();
    let json = serde_json::to_string(&report).unwrap();
    assert!(!json.contains("/Users/"));
    assert!(!json.contains("# Confidential"));
}

#[test]
fn conversion_report_contains_only_metrics_and_provenance() {
    let temporary = tempfile::tempdir().unwrap();
    let input =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/pdf/text.pdf");
    let output = temporary.path().join("converted.md");
    let report_path = temporary.path().join("report.json");

    let report = convert_file(&input, &output, &report_path, "public-test-pdf").unwrap();
    let markdown = fs::read_to_string(&output).unwrap();
    let json = fs::read_to_string(&report_path).unwrap();
    let serialized: anydoc_henry_mvp::ConversionReport = serde_json::from_str(&json).unwrap();

    assert!(!markdown.trim().is_empty());
    assert_eq!(serialized, report);
    assert_eq!(serialized.markdown_chars, markdown.chars().count() as u64);
    assert!(!json.contains(input.to_string_lossy().as_ref()));
    assert!(!json.contains(markdown.trim()));
}

#[test]
fn unsupported_input_returns_a_typed_error_and_writes_nothing() {
    let temporary = tempfile::tempdir().unwrap();
    let input = temporary.path().join("unsupported.bin");
    let output = temporary.path().join("converted.md");
    let report = temporary.path().join("report.json");
    fs::write(&input, b"not a supported document").unwrap();

    let error = convert_file(&input, &output, &report, "unsupported-fixture").unwrap_err();
    let typed = error.downcast_ref::<ConversionError>().unwrap();

    assert_eq!(typed.code(), "format_detection");
    assert!(!output.exists());
    assert!(!report.exists());
}

#[test]
fn image_only_pdf_returns_a_typed_ocr_error_and_writes_nothing() {
    let temporary = tempfile::tempdir().unwrap();
    let input = temporary.path().join("image-only.pdf");
    let output = temporary.path().join("converted.md");
    let report = temporary.path().join("report.json");
    fs::write(&input, image_only_pdf()).unwrap();

    let error = convert_file(&input, &output, &report, "image-only-fixture").unwrap_err();
    let typed = error.downcast_ref::<ConversionError>().unwrap();

    assert_eq!(typed.code(), "unsupported");
    assert!(typed.to_string().contains("OCR"));
    assert!(!output.exists());
    assert!(!report.exists());
}

fn image_only_pdf() -> Vec<u8> {
    let content = b"q 1 0 0 1 0 0 cm /Im0 Do Q";
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        [
            format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(),
            content.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat(),
        [
            b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n"
                .to_vec(),
            vec![0],
            b"\nendstream".to_vec(),
        ]
        .concat(),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}
