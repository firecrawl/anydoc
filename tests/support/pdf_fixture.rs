//! Deterministic in-memory PDF fixtures for the selective OCR pipeline.
//!
//! The bytes are generated rather than committed so no separately licensed
//! binary fixture is needed and every build target exercises the same input.
//! Image pages carry a raw-RGB XObject large enough for pdf-inspector to
//! classify the page as scanned; text pages use an uncompressed content
//! stream and a base-14 font.

// Each consumer uses a subset of the builders.
#![allow(dead_code)]

/// Width in pixels of the raw-RGB image placed on image pages. Together with
/// [`IMAGE_HEIGHT`] this clears the detector's scanned-page image-area
/// threshold.
pub const IMAGE_WIDTH: u32 = 1_700;

/// Height in pixels of the raw-RGB image placed on image pages.
pub const IMAGE_HEIGHT: u32 = 2_200;

/// US Letter, in PDF points.
const PAGE_WIDTH: u32 = 612;
const PAGE_HEIGHT: u32 = 792;

enum Page<'a> {
    /// Lines of ASCII text drawn with the base-14 Helvetica font.
    Text(&'a [&'a str]),
    /// The raw-RGB image scaled to fill the page.
    Image,
    /// The same image with one short line of text over it, the shape a
    /// scanned page carrying a tool watermark has.
    WatermarkedImage(&'a str),
}

/// A single-page PDF whose only content is a page-filling raw-RGB image.
pub fn image_only_pdf() -> Vec<u8> {
    build_pdf(&[Page::Image])
}

/// A three-page PDF: a text page, the image page, then another text page.
pub fn mixed_text_and_image_pdf() -> Vec<u8> {
    build_pdf(&[
        Page::Text(&["Structured first page", "with two extracted lines."]),
        Page::Image,
        Page::Text(&["Structured third page", "with two extracted lines."]),
    ])
}

/// The watermark a scanned page of [`watermarked_image_pdf`] carries.
pub const WATERMARK: &str = "Produced with a Trial Version of PDF Annotator";

/// A single-page PDF whose page is the raw-RGB image with nothing but a tool
/// watermark as text: real content only OCR can read.
pub fn watermarked_image_pdf() -> Vec<u8> {
    build_pdf(&[Page::WatermarkedImage(WATERMARK)])
}

/// A two-page PDF with text on every page and no images.
pub fn text_only_pdf() -> Vec<u8> {
    build_pdf(&[
        Page::Text(&["Structured first page", "with two extracted lines."]),
        Page::Text(&["Structured second page", "with two extracted lines."]),
    ])
}

fn build_pdf(pages: &[Page]) -> Vec<u8> {
    let has_text =
        pages.iter().any(|page| matches!(page, Page::Text(_) | Page::WatermarkedImage(_)));
    let has_image =
        pages.iter().any(|page| matches!(page, Page::Image | Page::WatermarkedImage(_)));
    // Objects 1 and 2 are the catalog and the page tree; each page then takes
    // a page dictionary and a content stream, and the shared font and image
    // resources follow.
    let font_id = 3 + pages.len() * 2;
    let image_id = font_id + usize::from(has_text);

    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0_usize];

    add_object(&mut pdf, &mut offsets, 1, b"<< /Type /Catalog /Pages 2 0 R >>");

    let kids = (0..pages.len())
        .map(|index| format!("{} 0 R", 3 + index * 2))
        .collect::<Vec<_>>()
        .join(" ");
    add_object(
        &mut pdf,
        &mut offsets,
        2,
        format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).as_bytes(),
    );

    for (index, page) in pages.iter().enumerate() {
        let page_id = 3 + index * 2;
        let content_id = page_id + 1;
        let (resources, content) = match page {
            Page::Text(lines) => {
                (format!("<< /Font << /F1 {font_id} 0 R >> >>"), text_content_stream(lines))
            }
            Page::Image => (
                format!("<< /XObject << /Im0 {image_id} 0 R >> >>"),
                format!("q {PAGE_WIDTH} 0 0 {PAGE_HEIGHT} 0 0 cm /Im0 Do Q"),
            ),
            Page::WatermarkedImage(watermark) => (
                format!("<< /Font << /F1 {font_id} 0 R >> /XObject << /Im0 {image_id} 0 R >> >>"),
                format!(
                    "q {PAGE_WIDTH} 0 0 {PAGE_HEIGHT} 0 0 cm /Im0 Do Q {}",
                    text_content_stream(&[watermark])
                ),
            ),
        };
        add_object(
            &mut pdf,
            &mut offsets,
            page_id,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_WIDTH} {PAGE_HEIGHT}] \
                 /Resources {resources} /Contents {content_id} 0 R >>"
            )
            .as_bytes(),
        );
        add_object(
            &mut pdf,
            &mut offsets,
            content_id,
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).as_bytes(),
        );
    }

    if has_text {
        add_object(
            &mut pdf,
            &mut offsets,
            font_id,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
        );
    }
    if has_image {
        let image = checkerboard_rgb();
        let mut stream = format!(
            "<< /Type /XObject /Subtype /Image /Width {IMAGE_WIDTH} /Height {IMAGE_HEIGHT} \
             /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >>\nstream\n",
            image.len()
        )
        .into_bytes();
        stream.extend_from_slice(&image);
        stream.extend_from_slice(b"\nendstream");
        add_object(&mut pdf, &mut offsets, image_id, &stream);
    }

    let xref_start = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len()).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_start}\n%%EOF",
            offsets.len()
        )
        .as_bytes(),
    );
    pdf
}

fn add_object(pdf: &mut Vec<u8>, offsets: &mut Vec<usize>, id: usize, body: &[u8]) {
    assert_eq!(id, offsets.len());
    offsets.push(pdf.len());
    pdf.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
    pdf.extend_from_slice(body);
    pdf.extend_from_slice(b"\nendobj\n");
}

fn text_content_stream(lines: &[&str]) -> String {
    let mut content = String::from("BT /F1 18 Tf 72 700 Td");
    for line in lines {
        assert!(
            line.is_ascii() && !line.contains(['(', ')', '\\']),
            "fixture text has to be escape-free ASCII"
        );
        content.push_str(&format!(" ({line}) Tj 0 -24 Td"));
    }
    content.push_str(" ET");
    content
}

/// Row-major RGB pixels forming coarse light and dark blocks.
fn checkerboard_rgb() -> Vec<u8> {
    const BLOCK: u32 = 100;
    let row = |offset: u32| {
        (0..IMAGE_WIDTH)
            .flat_map(|x| {
                if (x / BLOCK + offset).is_multiple_of(2) { [16, 16, 16] } else { [240, 240, 240] }
            })
            .collect::<Vec<u8>>()
    };
    let (even, odd) = (row(0), row(1));
    let mut image = Vec::with_capacity((IMAGE_WIDTH * IMAGE_HEIGHT * 3) as usize);
    for y in 0..IMAGE_HEIGHT {
        image.extend_from_slice(if (y / BLOCK).is_multiple_of(2) { &even } else { &odd });
    }
    image
}
