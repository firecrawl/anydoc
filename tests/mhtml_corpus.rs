use anydoc::model::{Block, ImageSource, Inline};
use anydoc::{Format, to_document, to_markdown_bytes};
use std::{fs, path::PathBuf};
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/html").join(name)
}
fn wrap(html: &[u8], base64: bool) -> Vec<u8> {
    let body = if base64 { b64(html) } else { String::from_utf8(html.to_vec()).unwrap() };
    let transfer = if base64 { "Content-Transfer-Encoding: base64\r\n" } else { "" };
    format!("From: <Saved by Blink>\r\nSnapshot-Content-Location: https://example.test/docs/page.html\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related; type=\"text/html\"; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/html; charset=utf-8\r\nContent-Location: https://example.test/docs/page.html\r\n{transfer}\r\n{body}\r\n--b--\r\n").into_bytes()
}
fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::new();
    for c in bytes.chunks(3) {
        let a = c[0];
        let b = c.get(1).copied().unwrap_or(0);
        let d = c.get(2).copied().unwrap_or(0);
        o.push(T[(a >> 2) as usize] as char);
        o.push(T[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        o.push(if c.len() > 1 { T[(((b & 15) << 2) | (d >> 6)) as usize] as char } else { '=' });
        o.push(if c.len() > 2 { T[(d & 63) as usize] as char } else { '=' });
    }
    o
}
#[test]
fn realistic_resource_free_html_is_byte_invariant_through_mhtml() {
    for name in [
        "controlled-document.html",
        "controlled-lists.html",
        "controlled-merged-table.html",
        "libreoffice-docx-numbering.html",
        "libreoffice-xlsx-merged.html",
    ] {
        let h = fs::read(fixture(name)).unwrap();
        let s = to_markdown_bytes(&h, Some(Format::Html)).unwrap();
        let m = wrap(&h, false);
        assert_eq!(Format::from_bytes(&m), Some(Format::Mhtml), "{name}");
        assert_eq!(to_markdown_bytes(&m, Some(Format::Mhtml)).unwrap(), s, "{name}");
    }
}
#[test]
fn realistic_utf8_html_is_invariant_through_base64_root() {
    let h = fs::read(fixture("controlled-document.html")).unwrap();
    assert_eq!(
        to_markdown_bytes(&wrap(&h, true), Some(Format::Mhtml)).unwrap(),
        to_markdown_bytes(&h, Some(Format::Html)).unwrap()
    );
}
#[test]
fn libreoffice_relative_images_deliberately_resolve_via_mhtml_content_location() {
    let h = fs::read(fixture("libreoffice-docx-text.html")).unwrap();
    let s = to_markdown_bytes(&h, Some(Format::Html)).unwrap();
    let w = to_markdown_bytes(&wrap(&h, false), Some(Format::Mhtml)).unwrap();
    assert!(!s.contains("https://example.test/docs/text_html_c4ee90a9.png"));
    assert!(w.contains("https://example.test/docs/text_html_c4ee90a9.png"));
    for n in [
        "# Fixture Document",
        "First numbered",
        "Wide head",
        "Music clef 𝄞",
        "Persian with ZWNJ",
        "Family emoji",
        "[example](https://example.com/page)",
    ] {
        assert!(s.contains(n));
        assert!(w.contains(n));
    }
}
#[test]
fn realistic_related_css_and_image_resources_resolve() {
    let h=fs::read_to_string(fixture("controlled-document.html")).unwrap().replace("<style>\n    .strong { font-weight: bold; }\n    .hidden { display: none; }\n  </style>","<base href=\"https://cdn.example.test/assets/\"><link rel=\"stylesheet\" href=\"styles/site.css\">").replace("</body>","<p><img alt=\"pixel\" src=\"images/pixel.png\"></p></body>");
    let m=format!("From: <Saved by Blink>\r\nSnapshot-Content-Location: https://example.test/docs/page.html\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related; type=\"text/html\"; start=\"<root@id>\"; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/html; charset=utf-8\r\nContent-ID: <root@id>\r\nContent-Location: https://example.test/docs/page.html\r\n\r\n{h}\r\n--b\r\nContent-Type: text/css; charset=utf-8\r\nContent-Location: https://cdn.example.test/assets/styles/site.css\r\n\r\n.strong {{ font-weight: bold; }} .hidden {{ display: none; }}\r\n--b\r\nContent-Type: image/png\r\nContent-ID: <IMAGE@ID>\r\nContent-Location: https://cdn.example.test/assets/images/pixel.png\r\nContent-Transfer-Encoding: base64\r\n\r\nAAECAw==\r\n--b--\r\n").into_bytes();
    let d = to_document(&m, Some(Format::Mhtml)).unwrap();
    assert_eq!(d.assets.len(), 1);
    assert!(d.blocks.iter().any(|b|matches!(b,Block::Paragraph(xs)if xs.iter().any(|x|matches!(x,Inline::Image{source:ImageSource::Asset(_),..})))));
    let md = to_markdown_bytes(&m, Some(Format::Mhtml)).unwrap();
    assert!(md.contains("**Styled bold paragraph.**"));
    assert!(!md.contains("drop me"));
    assert!(md.contains("café Ł music 𝄞 family 👨‍👩‍👧"));
}
#[test]
fn common_mime_variants_remain_not_mhtml() {
    for subtype in ["mixed", "alternative"] {
        let e=format!("MIME-Version: 1.0\r\nContent-Type: multipart/{subtype}; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/html\r\nContent-ID: <root@id>\r\n\r\n<p>email</p>\r\n--b\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment\r\n\r\ndata\r\n--b--\r\n").into_bytes();
        assert_eq!(Format::from_bytes(&e), None);
    }
}
