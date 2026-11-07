use anydoc::{ConvertError, Format, to_markdown_bytes};

fn inline_image_pdf(properties: &[u8]) -> Vec<u8> {
    let mut content = b"q 8 0 0 8 0 0 cm BI /W 8 /H 8 /BPC 8 ".to_vec();
    content.extend_from_slice(properties);
    content.extend_from_slice(b" ID ");
    content.extend_from_slice(&[0; 64]);
    content.extend_from_slice(b" EI Q\n");
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R >>"
            .to_vec(),
        [
            format!("<< /Length {} >>\nstream\n", content.len()).into_bytes(),
            content,
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
    let xref = pdf.len();
    pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    pdf
}

#[test]
fn inline_image_parser_failure_returns_a_conversion_error() {
    let result = to_markdown_bytes(&inline_image_pdf(b"/G true"), Format::Pdf);
    assert!(matches!(result, Err(ConvertError::Malformed { .. })));
    assert!(to_markdown_bytes(b"first,second\n1,2\n", Format::Csv).is_ok());
}

#[test]
fn inline_image_with_color_space_still_requests_ocr() {
    let result = to_markdown_bytes(&inline_image_pdf(b"/CS /DeviceGray"), Format::Pdf);
    assert!(matches!(result, Err(ConvertError::NeedsOcr { .. })));
}
