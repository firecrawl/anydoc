//! End-to-end OCR model initialization tests.

#![cfg(feature = "ocr")]

use std::path::PathBuf;

fn model_path(variable: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set {variable} to the downloaded RTen model"))
}

#[test]
#[ignore = "requires the external ocrs detection and recognition models"]
fn standard_models_initialize() {
    let detection = std::fs::read(model_path("ANYDOC_OCR_DETECTION_MODEL")).unwrap();
    let recognition = std::fs::read(model_path("ANYDOC_OCR_RECOGNITION_MODEL")).unwrap();
    let converter =
        anydoc::Converter::builder().with_ocr_models(detection, recognition).unwrap().build();
    assert!(converter.has_ocr());
}
