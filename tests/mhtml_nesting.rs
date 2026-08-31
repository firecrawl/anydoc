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

#[test]
fn non_multipart_boundary_parameter_is_not_mime_nesting() {
    let mut mhtml = String::from(
        "MIME-Version: 1.0\r\nSnapshot-Content-Location: https://example.test/page\r\nContent-Type: multipart/related; boundary=\"outer\"\r\n\r\n--outer\r\nContent-Type: text/plain; boundary=\"fake0\"\r\n\r\n",
    );
    for depth in 0..90 {
        mhtml.push_str(&format!(
            "--fake{depth}\r\nContent-Type: text/plain; boundary=\"fake{}\"\r\n\r\n",
            depth + 1
        ));
    }
    mhtml.push_str(
        "payload\r\n--outer\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<!doctype html><p>ok</p>\r\n--outer--\r\n",
    );
    assert_eq!(to_markdown_bytes(mhtml.as_bytes(), Some(Format::Mhtml)).unwrap(), "ok\n");
}
