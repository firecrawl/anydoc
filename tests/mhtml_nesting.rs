use anydoc::{ConvertError, Format, to_markdown_bytes};

fn deeply_nested_mhtml(levels: usize) -> Vec<u8> {
    let mut out = String::from(
        "Snapshot-Content-Location: https://example.test/page\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b0\"\r\n\r\n",
    );
    for level in 0..levels {
        out.push_str(&format!(
            "--b{level}\r\nContent-Type: multipart/mixed; boundary=\"b{}\"\r\n\r\n",
            level + 1
        ));
    }
    out.push_str(&format!(
        "--b{levels}\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>deep</p>\r\n--b{levels}--\r\n"
    ));
    for level in (0..levels).rev() {
        out.push_str(&format!("--b{level}--\r\n"));
    }
    out.into_bytes()
}

#[test]
fn excessive_multipart_nesting_is_rejected_before_mime_parse() {
    let input = deeply_nested_mhtml(300);
    let error = to_markdown_bytes(&input, Some(Format::Mhtml)).unwrap_err();
    assert!(matches!(error, ConvertError::ResourceLimit { limit: "max_mime_depth", .. }));
}
