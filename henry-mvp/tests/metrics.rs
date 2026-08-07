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
fn counts_arbitrary_ordered_list_markers() {
    let stats = summarize_markdown("2. second\n17. seventeenth\n3) third\n");

    assert_eq!(stats.list_items, 3);
}

#[test]
fn counts_plain_paragraph_blocks() {
    let stats = summarize_markdown(
        "# Heading\n\nFirst paragraph line one.\nLine two.\n\n- list item\n\nSecond paragraph.\n",
    );
    let json = serde_json::to_value(&stats).unwrap();

    assert_eq!(json["paragraphs"], 2);
}

#[test]
fn counts_contiguous_table_blocks() {
    let stats = summarize_markdown(
        "| A | B |\n| - | - |\n| 1 | 2 |\n\nText between tables.\n\n| C | D |\n| - | - |\n",
    );
    let json = serde_json::to_value(&stats).unwrap();

    assert_eq!(stats.table_rows, 5);
    assert_eq!(json["tables"], 2);
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

#[cfg(unix)]
#[test]
fn conversion_creates_private_directories_and_files() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let input =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/pdf/text.pdf");
    let private_directory = temporary.path().join("new-private-directory");
    let output = private_directory.join("converted.md");
    let report_path = private_directory.join("report.json");

    convert_file(&input, &output, &report_path, "permissions-fixture").unwrap();

    assert_eq!(fs::metadata(&private_directory).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(fs::metadata(&output).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(&report_path).unwrap().permissions().mode() & 0o777, 0o600);
}

#[cfg(unix)]
#[test]
fn conversion_does_not_repermission_an_existing_parent_directory() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir().unwrap();
    let input =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/pdf/text.pdf");
    fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o750)).unwrap();
    let output = temporary.path().join("converted.md");
    let report_path = temporary.path().join("report.json");

    convert_file(&input, &output, &report_path, "existing-parent-fixture").unwrap();

    assert_eq!(fs::metadata(temporary.path()).unwrap().permissions().mode() & 0o777, 0o750);
}

#[test]
fn second_publication_failure_restores_existing_markdown() {
    let temporary = tempfile::tempdir().unwrap();
    let input =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/pdf/text.pdf");
    let output = temporary.path().join("converted.md");
    let report_path = temporary.path().join("report.json");
    fs::write(&output, b"old markdown").unwrap();
    fs::create_dir(&report_path).unwrap();

    let result = convert_file(&input, &output, &report_path, "rollback-fixture");

    assert!(result.is_err());
    assert_eq!(fs::read(&output).unwrap(), b"old markdown");
    assert!(report_path.is_dir());
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 2);
}

#[test]
fn second_publication_failure_removes_new_markdown_without_a_previous_file() {
    let temporary = tempfile::tempdir().unwrap();
    let input =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/pdf/text.pdf");
    let output = temporary.path().join("converted.md");
    let report_path = temporary.path().join("report.json");
    fs::create_dir(&report_path).unwrap();

    let result = convert_file(&input, &output, &report_path, "rollback-fixture");

    assert!(result.is_err());
    assert!(!output.exists());
    assert!(report_path.is_dir());
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 1);
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

#[test]
fn mixed_pdf_surfaces_ocr_warning_in_report() {
    let temporary = tempfile::tempdir().unwrap();
    let input = temporary.path().join("mixed.pdf");
    let output = temporary.path().join("converted.md");
    let report_path = temporary.path().join("report.json");
    fs::write(&input, mixed_pdf()).unwrap();

    let report = convert_file(&input, &output, &report_path, "mixed-fixture").unwrap();

    assert!(report.warnings.iter().any(|warning| {
        warning == "pdf_ocr_required: 1 of 2 pages need OCR and were not extracted"
    }));
}

#[test]
fn encoding_issue_pdf_surfaces_encoding_warning_in_report() {
    let temporary = tempfile::tempdir().unwrap();
    let input = temporary.path().join("encoding-issue.pdf");
    let output = temporary.path().join("converted.md");
    let report_path = temporary.path().join("report.json");
    fs::write(&input, encoding_issue_pdf()).unwrap();

    let report = convert_file(&input, &output, &report_path, "encoding-fixture").unwrap();

    assert!(report.warnings.iter().any(|warning| {
        warning
            == "pdf_encoding_issues: broken font encodings detected; extracted text may be garbled"
    }));
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
    build_pdf(&objects)
}

fn mixed_pdf() -> Vec<u8> {
    let text_content = b"BT /F1 12 Tf 10 90 Td (Readable native text appears here) Tj 0 -20 Td (A second extractable line appears here) Tj 0 -20 Td (A third extractable line appears here) Tj ET";
    let image_content = b"q 100 0 0 100 0 0 cm /Im0 Do Q";
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /Im0 7 0 R >> >> /Contents 8 0 R >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        stream_object(text_content),
        image_object(),
        stream_object(image_content),
    ];
    build_pdf(&objects)
}

fn encoding_issue_pdf() -> Vec<u8> {
    let garbled = "8VceZWZTReVW9VdZXReZdhZeYcVdaVTeeHVcZVd8EcVWVccVUHeT:iYZSZe-(,e2'WZ]V \
        ;VScfRcj2&,*,*$ .'R CZdecfVehYZTYUVWZVdeYVcZXYedWY]UVcdW]X'eVcUVSeYVcVXZdecReRUR]]ed \
        TCBGC@ZUReVUdfSdZUZRcZVdZdWZ]VUYVcVhZeYafcdfReeVXf]ReZV0*S$.$ZZZ$6$&ViTVae \
        WceYVZdecfVedcVWVccVUe.'S&.'T&.'U&.'V&.'WSV]h(EfcdfReeYZdcVXf]ReZYV \
        cVXZdecReYVcVSjRXcVVdeWfcZdYRTajWRjdfTYZdecfVeWZ]VUYVcVhZeYeYVH:8 \
        cVbfVde( .'S fRcRejWTVceRZZXReZTZWZT7V]]IV]VaYV8(RUHfeYhVdeVc7V]]IV]VaYV8 \
        :iYZSZe.'TeYVaVcZUVUZX9VTVSVc-&,*$";
    let hex: String = garbled.as_bytes().iter().map(|byte| format!("{byte:02X}")).collect();
    let content = format!("BT /F1 12 Tf 10 50 Td <{hex}> Tj ET");
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        stream_object(content.as_bytes()),
    ];
    build_pdf(&objects)
}

fn image_object() -> Vec<u8> {
    [
        b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n"
            .to_vec(),
        vec![0],
        b"\nendstream".to_vec(),
    ]
    .concat()
}

fn stream_object(content: &[u8]) -> Vec<u8> {
    [
        format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(),
        content.to_vec(),
        b"\nendstream".to_vec(),
    ]
    .concat()
}

fn build_pdf(objects: &[Vec<u8>]) -> Vec<u8> {
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
