use anydoc::model::{Block, ImageSource, Inline};
use anydoc::{ConvertError, Format, to_document, to_markdown_bytes};

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
fn pdf_header_before_html_marker_remains_pdf() {
    let bytes = b"  %PDF-1.7\n<html><body>not an HTML root</body></html>";
    assert_eq!(Format::from_bytes(bytes), Some(Format::Pdf));
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
fn utf16le_doctype_allows_long_whitespace_between_keyword_and_name() {
    let source = format!("<!DOCTYPE{}html><html></html>", " ".repeat(80));
    let mut bytes = vec![0xFF, 0xFE];
    for unit in source.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    assert_eq!(Format::from_bytes(&bytes), Some(Format::Html));
}

#[test]
fn utf16be_doctype_allows_long_whitespace_between_keyword_and_name() {
    let source = format!("<!DOCTYPE{}html><html></html>", " ".repeat(80));
    let mut bytes = vec![0xFE, 0xFF];
    for unit in source.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    assert_eq!(Format::from_bytes(&bytes), Some(Format::Html));
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

#[test]
fn quoted_mime_parameter_semicolon_does_not_fake_charset() {
    let mut html = br#"<!doctype html><meta http-equiv="content-type" content='text/html; note="x;charset=utf-8"; charset=windows-1252'><p>"#.to_vec();
    html.push(0x80);
    html.extend_from_slice(b"</p>");
    let markdown = to_markdown_bytes(&html, Some(Format::Html)).unwrap();
    assert_eq!(markdown, "€\n");
}

#[test]
fn optional_li_end_tags_do_not_count_as_nested_depth() {
    let mut html = String::from("<!doctype html><ul>");
    for i in 0..300 {
        html.push_str(&format!("<li>item {i}"));
    }
    html.push_str("</ul>");
    let markdown = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap();
    assert!(markdown.contains("item 299"));
}

#[test]
fn optional_p_end_tags_do_not_count_as_nested_depth() {
    let mut html = String::from("<!doctype html>");
    for i in 0..300 {
        html.push_str(&format!("<p>paragraph {i}"));
    }
    let markdown = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap();
    assert!(markdown.contains("paragraph 299"));
}

#[test]
fn meta_charset_after_first_kib_is_still_honored() {
    let mut html = b"<!doctype html><html><head><style>/*".to_vec();
    html.extend(std::iter::repeat_n(b'x', 2048));
    html.extend_from_slice(b"*/</style><meta charset=iso-8859-2></head><body><p>");
    html.push(0xA3);
    html.extend_from_slice(b"</p></body></html>");
    let markdown = to_markdown_bytes(&html, Some(Format::Html)).unwrap();
    assert_eq!(markdown, "Ł\n");
}

#[test]
fn protocol_relative_image_is_preserved_as_external() {
    let html = br#"<!doctype html><p><img alt="pixel" src="//cdn.example.test/image.png"></p>"#;
    let document = to_document(html, Some(Format::Html)).unwrap();
    match &document.blocks[0] {
        Block::Paragraph(inlines) => assert!(matches!(
            &inlines[0],
            Inline::Image { source: ImageSource::External(url), .. }
                if url == "//cdn.example.test/image.png"
        )),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

fn assert_preflight_depth_limit(error: ConvertError) {
    match error {
        ConvertError::ResourceLimit { limit, detail } => {
            assert_eq!(limit, "max_xml_depth");
            assert!(
                detail.contains("before DOM construction"),
                "expected preflight depth rejection, got: {detail}"
            );
        }
        other => panic!("expected max_xml_depth resource limit, got {other:?}"),
    }
}

#[test]
fn non_void_self_closing_html_tags_still_count_toward_preflight_depth() {
    let mut html = String::from("<!doctype html>");
    for _ in 0..300 {
        html.push_str("<div/>");
    }

    assert_preflight_depth_limit(
        to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err(),
    );
}

#[test]
fn successive_headings_are_implicitly_closed_before_preflight_depth_counting() {
    let mut html = String::from("<!doctype html>");
    for i in 0..300 {
        html.push_str(&format!("<h1>heading {i}"));
    }

    let markdown = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap();
    assert!(markdown.contains("# heading 299"));
}

#[test]
fn alternating_headings_are_implicitly_closed_before_preflight_depth_counting() {
    const HEADINGS: [&str; 6] = ["h1", "h2", "h3", "h4", "h5", "h6"];
    let mut html = String::from("<!doctype html>");
    for i in 0..300 {
        let heading = HEADINGS[i % HEADINGS.len()];
        html.push_str(&format!("<{heading}>heading {i}"));
    }

    let markdown = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap();
    assert!(markdown.contains("heading 299"));
}

#[test]
fn foreign_self_closing_svg_elements_do_not_accumulate_html_depth() {
    let mut html = String::from("<!doctype html><svg>");
    for _ in 0..300 {
        html.push_str("<path/>");
    }
    html.push_str("</svg><p>ok</p>");

    let markdown = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap();
    assert!(markdown.contains("ok"));
}

#[test]
fn html_inside_svg_foreign_object_still_counts_self_closing_non_void_depth() {
    let mut html = String::from("<!doctype html><svg><foreignObject>");
    for _ in 0..300 {
        html.push_str("<div/>");
    }
    html.push_str("</foreignObject></svg>");

    assert_preflight_depth_limit(
        to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err(),
    );
}

#[test]
fn nested_list_wrappers_preserve_structural_children() {
    let html = br#"<!doctype html>
        <ol><ol>
          <li><h2>Nested heading</h2></li>
          <table><tr><td>A</td></tr></table>
        </ol></ol>"#;
    let markdown = to_markdown_bytes(html, Some(Format::Html)).unwrap();
    assert!(markdown.contains("Nested heading"), "{markdown:?}");
    assert!(markdown.contains("| A |"), "{markdown:?}");
}

#[test]
fn repeated_unclosed_anchors_do_not_trigger_depth_limit() {
    let mut html = String::from("<!doctype html>");
    for index in 0..300 {
        html.push_str(&format!("<a href=\"#{index}\">link"));
    }
    let result = to_markdown_bytes(html.as_bytes(), Some(Format::Html));
    assert!(
        result.is_ok(),
        "HTML5 repairs repeated anchors; preflight must not reject them: {result:?}"
    );
}

#[test]
fn intervening_blocks_between_unclosed_anchors_still_hit_preflight_depth_limit() {
    let mut html = String::from("<!doctype html>");
    for index in 0..300 {
        html.push_str(&format!("<a href=\"#{index}\"><div>"));
    }

    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn frameset_html_converts_without_body_error() {
    let html = br#"<!doctype html><html><head><title>frames</title></head><frameset cols="*"><frame src="page.html"></frameset></html>"#;
    assert_eq!(to_markdown_bytes(html, Some(Format::Html)).unwrap(), "");
}

#[test]
fn relative_image_is_preserved_without_fetching() {
    let html = br#"<!doctype html><p><img src="images/pixel.png"></p>"#;
    let document = to_document(html, Some(Format::Html)).unwrap();
    match &document.blocks[0] {
        Block::Paragraph(inlines) => assert!(matches!(
            &inlines[0],
            Inline::Image { source: ImageSource::External(url), .. }
                if url == "images/pixel.png"
        )),
        other => panic!("expected paragraph, got {other:?}"),
    }
}

#[test]
fn paragraph_div_pairs_do_not_accumulate_paragraph_depth() {
    // html5ever implicitly closes each <p> when the <div> arrives, so the
    // real DOM nests 200 divs - well under max_xml_depth.
    let mut html = String::from("<!doctype html><body>");
    for _ in 0..200 {
        html.push_str("<p>text<div>");
    }
    html.push_str("</body>");
    assert!(to_markdown_bytes(html.as_bytes(), Some(Format::Html)).is_ok());
}

#[test]
fn deeply_nested_paragraph_div_pairs_are_rejected_before_dom() {
    let mut html = String::from("<!doctype html><body>");
    for _ in 0..300 {
        html.push_str("<p>text<div>");
    }
    html.push_str("</body>");
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert!(matches!(error, ConvertError::ResourceLimit { limit: "max_xml_depth", .. }));
}

#[test]
fn bare_hash_link_is_preserved_as_relative_url() {
    let html = br##"<!doctype html><p><a href="#">top</a></p>"##;
    let markdown = to_markdown_bytes(html, Some(Format::Html)).unwrap();
    assert_eq!(markdown, "[top](#)\n");
}

#[test]
fn foreign_content_breakout_does_not_bypass_depth_limit() {
    // <p> breaks out of <svg>; the HTML <path/> tags that follow must not
    // be honored as foreign self-closing, otherwise the depth accounting
    // never runs and the DOM is constructed before any limit fires.
    let mut html = String::from("<!doctype html><body><svg><p>");
    for _ in 0..300 {
        html.push_str("<path/>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn headings_with_open_inlines_are_rejected_before_dom_construction() {
    // html5ever pops the current node only when it is already a heading, so
    // unclosed inline elements between headings really do nest in the DOM.
    // The preflight mirrors the parser and must reject the shape before DOM
    // construction.
    let mut html = String::from("<!doctype html><body>");
    for i in 0..300 {
        html.push_str(&format!("<h{}><span>t", (i % 6) + 1));
    }
    html.push_str("</body>");
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn optgroup_start_closes_previous_option_and_group() {
    let mut html = String::from("<!doctype html><body><select>");
    for _ in 0..300 {
        html.push_str("<optgroup><option>a");
    }
    html.push_str("</select></body>");
    assert!(to_markdown_bytes(html.as_bytes(), Some(Format::Html)).is_ok());
}

#[test]
fn row_group_start_closes_previous_row_group() {
    let mut html = String::from("<!doctype html><body><table>");
    for _ in 0..300 {
        html.push_str("<tbody><tr><td>x");
    }
    html.push_str("</table></body>");
    assert!(to_markdown_bytes(html.as_bytes(), Some(Format::Html)).is_ok());
}

#[test]
fn text_directly_under_list_is_preserved() {
    let html = br##"<!doctype html><ul>stray bullet<li>b item</ul><ol start="2">stray ordered<li>o item</ol>"##;
    let markdown = to_markdown_bytes(html, Some(Format::Html)).unwrap();
    assert!(markdown.contains("stray bullet"), "got: {markdown:?}");
    assert!(markdown.contains("stray ordered"), "got: {markdown:?}");
    assert!(markdown.contains("b item"), "got: {markdown:?}");
    assert!(markdown.contains("o item"), "got: {markdown:?}");
}

#[test]
fn foreign_content_title_does_not_swallow_nested_markup() {
    // html5ever parses an svg <title>'s children as markup (svg title is an
    // HTML integration point), so the preflight must count them and reject
    // pathological nesting before DOM construction instead of switching to
    // the HTML RCDATA state.
    let mut html = String::from("<!doctype html><body><svg><title>");
    for _ in 0..300 {
        html.push_str("<div>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn html_title_still_swallows_markup_as_raw_text() {
    // The HTML (non-foreign) <title> keeps RCDATA semantics: nested markup is
    // text, not elements, and the document converts normally.
    let html = br#"<!doctype html><html><head><title><div>not markup</div></title></head><body><p>ok</p></body></html>"#;
    let markdown = to_markdown_bytes(html, Some(Format::Html)).unwrap();
    assert!(markdown.contains("ok"), "got: {markdown:?}");
    assert!(!markdown.contains("not markup"), "got: {markdown:?}");
}

#[test]
fn foreign_content_script_does_not_swallow_nested_markup() {
    // html5ever does not switch the tokenizer to ScriptData for scripts
    // inside foreign content, so nested markup inside <svg><script> really
    // becomes elements and the preflight must count it.
    let mut html = String::from("<!doctype html><body><svg><script>");
    for _ in 0..300 {
        html.push_str("<div>");
    }
    html.push_str("</script></svg></body>");
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn html_script_still_swallows_markup_as_script_data() {
    // The script sits in the body so a tokenizer that failed to enter
    // ScriptData would render its source as text; the negative assertions
    // are what actually guard the raw-text swallow.
    let html = br#"<!doctype html><html><body><script>var x = "<div>not markup</div>";</script><p>ok</p></body></html>"#;
    let markdown = to_markdown_bytes(html, Some(Format::Html)).unwrap();
    assert!(markdown.contains("ok"), "got: {markdown:?}");
    assert!(!markdown.contains("var x"), "got: {markdown:?}");
    assert!(!markdown.contains("not markup"), "got: {markdown:?}");
}

#[test]
fn foreign_content_option_tags_do_not_get_html_implied_closes() {
    // Inside <svg>, repeated <option> elements are ordinary foreign elements
    // that genuinely nest; the preflight must not apply the HTML select
    // insertion rule there.
    let mut html = String::from("<!doctype html><body><svg>");
    for _ in 0..300 {
        html.push_str("<option>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn nested_foreign_roots_fully_exit_on_breakout() {
    // html5ever pops every foreign root on a breakout tag; leaving the outer
    // svg on the modeled stack would treat later <path/> tags as foreign
    // self-closing and bypass the depth guard.
    let mut html = String::from("<!doctype html><body><svg><math><p>");
    for _ in 0..300 {
        html.push_str("<path/>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn template_body_does_not_capture_document_conversion() {
    let html = br#"<!doctype html><html><head><template><body>template body</body></template></head><body><p>real body</p></body></html>"#;
    let markdown = to_markdown_bytes(html, Some(Format::Html)).unwrap();
    assert_eq!(markdown, "real body\n");
}

#[test]
fn depth_limit_aligns_with_the_post_parse_walk_at_the_boundary() {
    // 255 nested divs under an explicit body sit exactly at the limit the
    // DOM walk accepts (body = depth 1, innermost div = depth 256).
    let mut ok = String::from("<!doctype html><html><body>");
    for _ in 0..255 {
        ok.push_str("<div>");
    }
    ok.push_str("x</body></html>");
    assert!(to_markdown_bytes(ok.as_bytes(), Some(Format::Html)).is_ok());

    // One more level exceeds it and must be rejected before DOM construction.
    let mut too_deep = String::from("<!doctype html><html><body>");
    for _ in 0..256 {
        too_deep.push_str("<div>");
    }
    too_deep.push_str("x</body></html>");
    let error = to_markdown_bytes(too_deep.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn void_hr_closes_open_paragraph_in_preflight() {
    let mut html = String::from("<!doctype html><body>");
    for _ in 0..300 {
        html.push_str("<p>x<hr>");
    }
    html.push_str("</body>");
    assert!(to_markdown_bytes(html.as_bytes(), Some(Format::Html)).is_ok());
}

#[test]
fn foreign_void_elements_are_pushed_and_counted() {
    // Void HTML names that are not foreign-content breakout tags (input,
    // param, source, ...) are ordinary foreign elements inside <svg> that
    // html5ever pushes; 300 of them nest and must be rejected before DOM
    // construction.
    let mut html = String::from("<!doctype html><body><svg>");
    for _ in 0..300 {
        html.push_str("<input>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn out_of_scope_end_tag_does_not_pop_real_nesting() {
    // html5ever ignores </div> here (a td scope marker sits above the div),
    // so the spans keep nesting; the preflight must not truncate the stack
    // on the ignored end tag.
    let mut html = String::from("<!doctype html><body><div><table><td>");
    for _ in 0..200 {
        html.push_str("<span>");
    }
    html.push_str("</div>");
    for _ in 0..100 {
        html.push_str("<span>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn duplicate_body_tag_does_not_move_the_depth_baseline() {
    // html5ever ignores the second <body>; if the preflight pushed it, the
    // body-relative depth baseline would move up and undercount the divs.
    let mut html = String::from("<!doctype html><body>");
    for _ in 0..130 {
        html.push_str("<div>");
    }
    html.push_str("<body>");
    for _ in 0..130 {
        html.push_str("<div>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn paragraph_end_tag_respects_button_scope() {
    // A <button> above the open <p> puts it out of button scope, so
    // html5ever leaves the stack alone for </p>; truncating there would pop
    // the button and the 150 nested divs and undercount the later divs.
    let mut html = String::from("<!doctype html><body><p><button>");
    for _ in 0..150 {
        html.push_str("<div>");
    }
    html.push_str("</p>");
    for _ in 0..150 {
        html.push_str("<div>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn table_end_tag_respects_template_scope() {
    // template is a table-scope marker: html5ever ignores </table> while the
    // template is above it, so the spans keep nesting.
    let mut html = String::from("<!doctype html><body><table><template>");
    for _ in 0..100 {
        html.push_str("<span>");
    }
    html.push_str("</table>");
    for _ in 0..160 {
        html.push_str("<span>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn body_end_tag_does_not_pop_open_nesting() {
    // html5ever keeps body open on </body> (mode switch only); truncating
    // the modeled stack there would drop the 200 open divs and undercount
    // the ones that follow.
    let mut html = String::from("<!doctype html><body>");
    for _ in 0..200 {
        html.push_str("<div>");
    }
    html.push_str("</body>");
    for _ in 0..200 {
        html.push_str("<div>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn foreign_html_elements_are_not_treated_as_duplicate_wrappers() {
    // <html> inside <svg> is an ordinary foreign element that html5ever
    // pushes; skipping it as a duplicate wrapper would undercount depth.
    let mut html = String::from("<!doctype html><body><svg>");
    for _ in 0..300 {
        html.push_str("<html>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn li_end_tag_ignores_buttons_and_closes_the_item() {
    // button is not a list-item scope marker: </li> must still close the
    // item (and the spans with it), keeping the later divs shallow.
    let mut html = String::from("<!doctype html><body><ul><li><button>b</button>");
    for _ in 0..130 {
        html.push_str("<span>s");
    }
    html.push_str("</li></ul>");
    for _ in 0..130 {
        html.push_str("<div>");
    }
    assert!(to_markdown_bytes(html.as_bytes(), Some(Format::Html)).is_ok());
}

#[test]
fn any_other_end_tag_is_ignored_below_special_elements() {
    // html5ever's any-other-end-tag walk ignores </em> the moment it
    // reaches the special div, so the em/div pairs really keep nesting;
    // truncating at the deeper em match would keep the modeled stack flat
    // and bypass the depth guard.
    let mut html = String::from("<!doctype html><body>");
    for _ in 0..130 {
        html.push_str("<em><div></em>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn frameset_with_many_sibling_frames_is_not_rejected_for_depth() {
    // html5ever inserts each <frame> and immediately pops it (and ignores
    // stray frames in body context), so hundreds of sibling frames stay
    // shallow; the preflight must not stack them and falsely report
    // max_xml_depth on a valid frameset document.
    let mut html = String::from("<!doctype html><html><frameset>");
    for _ in 0..300 {
        html.push_str("<frame>");
    }
    html.push_str("</frameset></html>");
    assert!(to_markdown_bytes(html.as_bytes(), Some(Format::Html)).is_ok());
}

#[test]
fn markup_inside_select_really_nests_and_hits_the_depth_guard() {
    // html5ever has no "in select" insertion mode: start tags inside a
    // select are processed in body mode and really insert, so deep markup
    // there must be rejected before DOM construction. Ignoring select
    // content (as the HTML spec's in-select mode would) is what diverges
    // from this parser and would undercount the real DOM.
    let mut html = String::from("<!doctype html><body><select>");
    for _ in 0..260 {
        html.push_str("<div>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn table_family_starts_clear_stray_content_above_the_table() {
    // html5ever clears the stack back to the table context for caption,
    // col, colgroup, tbody/tfoot/thead, and td/th/tr starts, popping
    // foster-parented stray content; keeping that content on the modeled
    // stack would accumulate phantom depth and falsely reject valid
    // repaired table markup at max_xml_depth.
    let mut html = String::from("<!doctype html><body><table>");
    for _ in 0..253 {
        html.push_str("<div>");
    }
    html.push_str("<tbody><tr><td>celula</td></tr></tbody></table>");
    let markdown = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap();
    assert!(markdown.contains("celula"), "got: {markdown:?}");
}

#[test]
fn block_starts_close_paragraphs_with_inline_descendants() {
    // html5ever closes an open p (in button scope) when a block start tag
    // arrives, popping its inline descendants too. Leaving the stale p on
    // the modeled stack made a later <p> start truncate through the
    // intervening blockquote, undercounting the nested blockquotes the
    // parser really keeps open.
    let mut html = String::from("<!doctype html><body>");
    for _ in 0..255 {
        html.push_str("<blockquote><p><b>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn li_start_does_not_close_items_below_special_elements() {
    // html5ever's li insertion walk stops at any special element other than
    // address/div/p, so each ul opens a new nesting level and the inner li
    // stays open; an unscoped search would flatten the stack and bypass the
    // depth guard on deeply nested lists.
    let mut html = String::from("<!doctype html><body>");
    for _ in 0..130 {
        html.push_str("<ul><li>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn stray_table_cells_outside_a_table_do_not_flatten_nesting() {
    // html5ever ignores td tokens in body context (no open table), so the
    // divs opened between them really keep nesting; the preflight must not
    // push stray cells either, because a later implied close would truncate
    // the divs and undercount depth.
    let mut html = String::from("<!doctype html><body>");
    for _ in 0..300 {
        html.push_str("<div><td>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn option_end_tags_only_pop_the_current_node() {
    // html5ever pops an option end tag only when the option is the current
    // node; below a span the token is ignored and the pairs keep nesting.
    let mut html = String::from("<!doctype html><body>");
    for _ in 0..300 {
        html.push_str("<option><span></option>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn p_start_does_not_close_paragraphs_below_a_button() {
    // html5ever closes an open p from a p start tag only when the p is in
    // button scope; below a button it stays open and the pairs nest.
    let mut html = String::from("<!doctype html><body>");
    for _ in 0..130 {
        html.push_str("<p><button>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn dd_end_tag_respects_button_scope() {
    // </dd> uses button scope like </p>: with a button above the dd the
    // token is ignored and the divs keep nesting on both sides.
    let mut html = String::from("<!doctype html><body><dl><dd><button>");
    for _ in 0..150 {
        html.push_str("<div>");
    }
    html.push_str("</dd>");
    for _ in 0..150 {
        html.push_str("<div>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}

#[test]
fn div_end_tag_still_closes_open_paragraphs() {
    // The div family pops to its match through non-marker elements (the
    // any-other-end-tag special stop must not apply to it), so repeated
    // <div><p></div> blocks stay shallow and the document converts.
    let mut html = String::from("<!doctype html><body>");
    for _ in 0..300 {
        html.push_str("<div><p>x</div>");
    }
    html.push_str("</body>");
    assert!(to_markdown_bytes(html.as_bytes(), Some(Format::Html)).is_ok());
}

#[test]
fn foreign_html_end_tags_pop_the_foreign_element() {
    // Inside <svg>, html is an ordinary foreign element (not a breakout
    // tag), so its end tag really pops it; keeping it open would accumulate
    // phantom nesting and falsely hit the depth limit.
    let mut html = String::from("<!doctype html><body><svg>");
    for _ in 0..130 {
        html.push_str("<html>x</html>");
    }
    html.push_str("</svg></body>");
    assert!(to_markdown_bytes(html.as_bytes(), Some(Format::Html)).is_ok());
}

#[test]
fn scope_end_tags_pop_through_foreign_roots_to_their_target() {
    // html5ever routes </p> through the foreign-content walk and then
    // reprocesses it in body mode, where bare svg/math are not scope
    // markers: the p below the svg is in button scope, so the parser pops
    // the paths, the g, the svg, and the p. The preflight must pop too, or
    // it falsely rejects with max_xml_depth a document that converts (max
    // depth ~134 here).
    let mut html = String::from("<!doctype html><body><p><svg><g>");
    for _ in 0..130 {
        html.push_str("<path>");
    }
    html.push_str("</p>");
    for _ in 0..130 {
        html.push_str("<path>");
    }
    assert!(to_markdown_bytes(html.as_bytes(), Some(Format::Html)).is_ok());
}

#[test]
fn unmatched_end_tags_still_stop_at_special_elements_below_foreign_roots() {
    // The reprocessed walk stops at the special p: no span matches above
    // it, so html5ever ignores </span> and the foreign nesting keeps
    // growing; the preflight must reject before DOM construction. Bare
    // svg/math do not stop that walk either — the special p below them
    // does.
    let mut html = String::from("<!doctype html><body><p><svg>");
    for _ in 0..130 {
        html.push_str("<g>");
    }
    html.push_str("</span>");
    for _ in 0..130 {
        html.push_str("<g>");
    }
    let error = to_markdown_bytes(html.as_bytes(), Some(Format::Html)).unwrap_err();
    assert_preflight_depth_limit(error);
}
