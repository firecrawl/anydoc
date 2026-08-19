use anydoc::{ConvertError, Format};

const PASSWORD: &str = "testPassword";

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("tests/fixtures/encrypted/{name}")).unwrap()
}

#[test]
fn encrypted_ooxml_converts_with_the_password() {
    let docx = fixture("password-test.docx");
    let markdown = anydoc::to_markdown_bytes_with_password(&docx, None, PASSWORD).unwrap();
    assert!(!markdown.is_empty());

    let xlsx = fixture("password-test.xlsx");
    let document = anydoc::to_document_with_password(&xlsx, None, PASSWORD).unwrap();
    assert!(!document.blocks.is_empty());
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
