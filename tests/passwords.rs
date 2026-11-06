use anydoc::{ConvertError, Format};

const PASSWORD: &str = "testPassword";

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("tests/fixtures/encrypted/{name}")).unwrap()
}

#[test]
fn encrypted_ooxml_converts_with_the_password() {
    let docx = fixture("password-test.docx");
    let markdown = anydoc::to_markdown_bytes_with_password(&docx, None, PASSWORD).unwrap();
    assert!(markdown.contains("testing testing"));

    let xlsx = fixture("password-test.xlsx");
    let document = anydoc::to_document_with_password(&xlsx, None, PASSWORD).unwrap();
    assert!(document.blocks.iter().any(|block| matches!(block, anydoc::model::Block::Table(_))));
    let markdown = anydoc::to_markdown_bytes_with_password(&xlsx, None, PASSWORD).unwrap();
    assert!(markdown.contains("| 1 | 2 | 3 | 4 | 5 |"));
    assert!(markdown.contains("heres some data"));
}

#[test]
fn decrypted_content_decides_the_format() {
    let docx = fixture("password-test.docx");
    let document = anydoc::to_document_with_password(&docx, Format::Excel, PASSWORD).unwrap();
    assert!(!document.blocks.is_empty());
}

#[test]
fn encrypted_ooxml_without_the_right_password_stays_encrypted() {
    let docx = fixture("password-test.docx");
    assert!(matches!(anydoc::to_markdown_bytes(&docx, Format::Docx), Err(ConvertError::Encrypted)));
    assert!(matches!(
        anydoc::to_markdown_bytes_with_password(&docx, Format::Docx, "wrong"),
        Err(ConvertError::Encrypted)
    ));
}

#[test]
fn oversized_encrypted_package_is_rejected() {
    // A tiny encrypted container declaring a plaintext length of u32::MAX.
    // Decryption must reject this before allocating the declared ~4 GiB.
    let bytes = include_bytes!("fixtures/malformed/oversized-encrypted-package--errors.docx");
    assert!(matches!(
        anydoc::to_markdown_bytes_with_password(bytes, None, PASSWORD),
        Err(ConvertError::Encrypted)
    ));
}
