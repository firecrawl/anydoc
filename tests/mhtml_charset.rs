use anydoc::{Format, to_markdown_bytes};

#[test]
fn html_meta_charset_is_used_when_mime_charset_is_missing() {
    let mhtml = b"MIME-Version: 1.0\r\n\
Content-Type: multipart/related; type=\"text/html\"; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/html\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
<!doctype html><meta http-equiv=3D\"Content-Type\" content=3D\"text/html; charset=3Dwindows-1252\"><p>Contrata=E7=E3o dever=E1</p>\r\n\
--b--\r\n";

    assert_eq!(to_markdown_bytes(mhtml, Some(Format::Mhtml)).unwrap(), "Contratação deverá\n");
}
