//! Password-protected OOXML packages (#102).
//!
//! The fixture is an agile (SHA-512) encrypted `.docx` taken from the
//! office-crypto crate's MIT-licensed test suite; its password is
//! `testPassword`. Without a password the corpus sweep already records the
//! rejection under the fixture's `--errors` annotation.

use common::fixture_root;

mod common;

use anydoc::{ConvertError, Format};

const FIXTURE: &str = "encrypted/agile-sha512-docx--errors.docx";
const PASSWORD: &str = "testPassword";

#[test]
fn encrypted_docx_converts_with_password() {
    let path = fixture_root().join(FIXTURE);
    let bytes = std::fs::read(&path).unwrap();
    let md = anydoc::to_markdown_bytes_with_password(&bytes, None, Some(PASSWORD))
        .expect("decrypt+convert");
    assert!(md.contains("testing testing"), "unexpected body: {md}");
}

#[test]
fn encrypted_docx_without_password_stays_encrypted() {
    let path = fixture_root().join(FIXTURE);
    let bytes = std::fs::read(&path).unwrap();
    let err = anydoc::to_markdown_bytes(&bytes, Format::Docx).unwrap_err();
    assert!(matches!(err, ConvertError::Encrypted));
}

#[test]
fn encrypted_docx_with_wrong_password_is_encrypted() {
    let path = fixture_root().join(FIXTURE);
    let bytes = std::fs::read(&path).unwrap();
    let err =
        anydoc::to_markdown_bytes_with_password(&bytes, Format::Docx, Some("wrong")).unwrap_err();
    assert!(matches!(err, ConvertError::Encrypted));
}

#[test]
fn empty_password_behaves_like_no_password() {
    let path = fixture_root().join(FIXTURE);
    let bytes = std::fs::read(&path).unwrap();
    let err = anydoc::to_markdown_bytes_with_password(&bytes, Format::Docx, Some("")).unwrap_err();
    assert!(matches!(err, ConvertError::Encrypted));
}

#[test]
fn plaintext_docx_ignores_password_argument() {
    // A password on a non-encrypted file must not change anything.
    let path = fixture_root().join("docx/handmade-numbering.docx");
    let bytes = std::fs::read(&path).unwrap();
    let with_pw = anydoc::to_markdown_bytes_with_password(&bytes, None, Some("ignored"));
    let without = anydoc::to_markdown_bytes(&bytes, None).unwrap();
    assert_eq!(with_pw.unwrap(), without);
}
