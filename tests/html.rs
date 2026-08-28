use anydoc::{ConvertError, Format, to_markdown_bytes};

#[test]
fn html_extensions_are_named() {
    assert_eq!(Format::from_extension("html"), Some(Format::Html));
    assert_eq!(Format::from_extension("HTM"), Some(Format::Html));
}

#[test]
fn html_doctype_is_detected_from_content() {
    let html = b"\xEF\xBB\xBF  <!DOCTYPE html><html><body><p>Hello</p></body></html>";
    assert_eq!(Format::from_bytes(html), Some(Format::Html));
}

#[test]
fn html_doctype_allows_html5_ascii_whitespace() {
    assert_eq!(Format::from_bytes(b"<!DOCTYPE\thtml><html></html>"), Some(Format::Html));
    assert_eq!(Format::from_bytes(b"<!DOCTYPE\nhtml><html></html>"), Some(Format::Html));
}

#[test]
fn html_prefix_wins_over_embedded_pdf_marker() {
    let html = b"<!doctype html><html><body>%PDF-1.7 is text here</body></html>";
    assert_eq!(Format::from_bytes(html), Some(Format::Html));
}

#[test]
fn utf16_html_is_detected_from_content() {
    let source = "<html><body>hello</body></html>";

    let mut le = vec![0xFF, 0xFE];
    for unit in source.encode_utf16() {
        le.extend_from_slice(&unit.to_le_bytes());
    }
    assert_eq!(Format::from_bytes(&le), Some(Format::Html));

    let mut be = vec![0xFE, 0xFF];
    for unit in source.encode_utf16() {
        be.extend_from_slice(&unit.to_be_bytes());
    }
    assert_eq!(Format::from_bytes(&be), Some(Format::Html));
}

#[test]
fn utf16_html_detection_allows_long_leading_whitespace() {
    let source = format!("{}<html><body>hello</body></html>", " ".repeat(300));

    let mut le = vec![0xFF, 0xFE];
    for unit in source.encode_utf16() {
        le.extend_from_slice(&unit.to_le_bytes());
    }
    assert_eq!(Format::from_bytes(&le), Some(Format::Html));

    let mut be = vec![0xFE, 0xFF];
    for unit in source.encode_utf16() {
        be.extend_from_slice(&unit.to_be_bytes());
    }
    assert_eq!(Format::from_bytes(&be), Some(Format::Html));
}

#[test]
fn unrelated_charset_attribute_does_not_change_decoding() {
    let html = "<!doctype html><p data-note='charset=windows-1252'>café</p>".as_bytes();
    let markdown = to_markdown_bytes(html, None).unwrap();
    assert_eq!(markdown, "café\n");
}

#[test]
fn meta_looking_text_in_comment_does_not_change_decoding() {
    let html = "<!doctype html><!-- <meta charset=windows-1252> --><p>café</p>".as_bytes();
    let markdown = to_markdown_bytes(html, None).unwrap();
    assert_eq!(markdown, "café\n");
}

#[test]
fn meta_looking_text_in_script_does_not_change_decoding() {
    let html =
        "<!doctype html><script>const fake = '<meta charset=windows-1252>';</script><p>café</p>"
            .as_bytes();
    let markdown = to_markdown_bytes(html, None).unwrap();
    assert_eq!(markdown, "café\n");
}

#[test]
fn html_node_limit_covers_nodes_outside_body_before_dom_materialization() {
    let mut html = String::from("<!doctype html><html><head>");
    for _ in 0..2_000_001 {
        html.push_str("<!---->");
    }
    html.push_str("</head><body>ok</body></html>");

    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert!(matches!(error, ConvertError::ResourceLimit { limit: "max_xml_nodes", .. }));
}

#[test]
fn malformed_html5_is_repaired_before_conversion() {
    let html = br#"<!doctype html><html><body><h1>Hello</h1><p>first<p><strong>second</strong></body></html>"#;
    let markdown = to_markdown_bytes(html, None).unwrap();
    assert_eq!(markdown, "# Hello\n\nfirst\n\n**second**\n");
}

#[test]
fn html5_table_inserts_implicit_structure() {
    let html = br#"<!doctype html><table><tr><th>A<th>B<tr><td>1<td>2</table>"#;
    let markdown = to_markdown_bytes(html, None).unwrap();
    assert_eq!(markdown, "| A | B |\n| --- | --- |\n| 1 | 2 |\n");
}

#[test]
fn style_blocks_feed_the_existing_semantic_css_subset() {
    let html = br#"<!doctype html><style>
        .hidden { display: none }
        .strong { font-weight: bold }
    </style><p class=hidden>drop me</p><p class=strong>keep me</p>"#;
    let markdown = to_markdown_bytes(html, None).unwrap();
    assert_eq!(markdown, "**keep me**\n");
}

#[test]
fn meta_charset_decodes_legacy_html() {
    let mut html = b"<!doctype html><meta charset=windows-1252><p>caf".to_vec();
    html.push(0xE9);
    html.extend_from_slice(b"</p>");
    let markdown = to_markdown_bytes(&html, None).unwrap();
    assert_eq!(markdown, "caf\u{e9}\n");
}

#[test]
fn scripts_are_not_document_content() {
    let html = br#"<!doctype html><p>before</p><script>document.write('not content')</script><p>after</p>"#;
    let markdown = to_markdown_bytes(html, None).unwrap();
    assert_eq!(markdown, "before\n\nafter\n");
}
