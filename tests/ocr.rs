//! End-to-end OCR tests against the standard models.

#![cfg(feature = "ocr")]

#[path = "support/pdf_fixture.rs"]
mod pdf_fixture;

use sha2::Digest as _;
use std::path::PathBuf;

fn model_path(variable: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set {variable} to the downloaded RTen model"))
}

fn model_converter() -> anydoc::Converter {
    let detection = std::fs::read(model_path("ANYDOC_OCR_DETECTION_MODEL")).unwrap();
    let recognition = std::fs::read(model_path("ANYDOC_OCR_RECOGNITION_MODEL")).unwrap();
    anydoc::Converter::builder().with_ocr_models(detection, recognition).unwrap().build()
}

#[test]
#[ignore = "requires the external ocrs detection and recognition models"]
fn standard_models_initialize() {
    assert!(model_converter().has_ocr());
}

/// The synthetic page carries no text, so the expected result is a successful
/// conversion to little or nothing. Only the shape can be asserted: the whole
/// render-and-recognize path runs, without panicking, to a bounded result.
#[test]
#[ignore = "requires the external ocrs detection and recognition models"]
fn standard_models_ocr_scanned_pdf() {
    /// One synthetic page cannot legitimately recognize more text than this.
    const MAX_RECOGNIZED_BYTES: usize = 64 * 1024;

    let markdown = model_converter()
        .to_markdown_bytes(&pdf_fixture::image_only_pdf(), anydoc::Format::Pdf)
        .expect("the scanned page converts");

    assert!(markdown.len() < MAX_RECOGNIZED_BYTES, "{} bytes recognized", markdown.len());
    assert!(!markdown.contains("\n\n\n"));
    assert!(markdown.is_empty() || markdown.ends_with('\n'));
}

/// A scanned page whose only extractable text is a tool watermark has to be
/// recognized anyway, and what it says has to reach the output. The file is
/// pinned by content hash so a corpus update cannot quietly change the claim.
#[test]
#[ignore = "requires the external ocrs models and the pinned sample-files corpus"]
fn corpus_scanned_watermark_routes_to_ocr() {
    const SHA256: &str = "aaad90df16fce40ec768629d2135479b98f65b39bb27c7f80fb106393187d619";

    let corpus = std::env::var_os("ANYDOC_PDF_SAMPLE_FILES")
        .map(PathBuf::from)
        .expect("set ANYDOC_PDF_SAMPLE_FILES to the pinned sample-files checkout");
    let pdf = std::fs::read(corpus.join("018-base64-image").join("base64image.pdf")).unwrap();
    let digest: String =
        sha2::Sha256::digest(&pdf).iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(digest, SHA256, "the pinned corpus file changed");

    let markdown =
        model_converter().to_markdown_bytes(&pdf, anydoc::Format::Pdf).expect("the scan converts");
    let normalized = markdown.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(normalized.contains("fix the issue and close it"), "recognized: {markdown}");
}
