use anydoc::Format;

#[test]
fn strong_rtf_signature_wins_over_mhtml_header_heuristic() {
    let rtf = b"{\\rtf1\\ansi\nSnapshot-Content-Location: https://example.test/page\nContent-Type: multipart/related; boundary=\"b\"\n\nplain text}";

    assert_eq!(Format::from_bytes(rtf), Some(Format::Rtf));
}

#[test]
fn valid_mhtml_wins_over_pdf_marker_in_mime_preamble() {
    let mhtml = b"MIME-Version: 1.0\r\nSnapshot-Content-Location: https://example.test/page\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n%PDF-1.7 is ordinary MIME preamble text\r\n--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<!doctype html><p>ok</p>\r\n--b--\r\n";

    assert_eq!(Format::from_bytes(mhtml), Some(Format::Mhtml));
}
