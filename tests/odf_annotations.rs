//! ODF comments must not become part of the paragraph they annotate.

use anydoc::{Format, to_markdown_bytes};
use std::io::{Cursor, Write};

fn odt(body: &str) -> Vec<u8> {
    let content = format!(
        r#"<?xml version="1.0"?><office:document-content
        xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
        xmlns:comments="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
        xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
        xmlns:dc="http://purl.org/dc/elements/1.1/"
        xmlns:xlink="http://www.w3.org/1999/xlink">
        <office:body><office:text>{body}</office:text></office:body>
        </office:document-content>"#
    );
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for (name, body) in
        [("mimetype", "application/vnd.oasis.opendocument.text"), ("content.xml", content.as_str())]
    {
        zip.start_file(name, opts).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn point_comment_does_not_split_the_anchored_word() {
    let bytes = odt(r#"<text:p>Text with a comment on Lamb<office:annotation>
        <dc:creator>Author</dc:creator><dc:date>2026-01-01T00:00:00</dc:date>
        <text:p>First comment paragraph.</text:p>
        <text:p><text:span>Second comment paragraph.</text:span></text:p>
        </office:annotation>daword here.</text:p>"#);
    let md = to_markdown_bytes(&bytes, Format::Odt).unwrap();
    assert_eq!(md.trim_end(), "Text with a comment on Lambdaword here.");
}

#[test]
fn ranged_comment_keeps_the_link_and_surrounding_text() {
    let bytes = odt(concat!(
        r#"<text:p>Read <text:a xlink:href="https://example.com">"#,
        r#"<text:span>the<comments:annotation comments:name="comment1">"#,
        "<dc:creator>Reviewer</dc:creator><text:p>Comment on the link.</text:p>",
        "</comments:annotation> guide</text:span></text:a>",
        r#"<comments:annotation-end comments:name="comment1"/> today.</text:p>"#,
    ));
    let md = to_markdown_bytes(&bytes, Format::Odt).unwrap();
    assert_eq!(md.trim_end(), "Read [the guide](https://example.com) today.");
}
