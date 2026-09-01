use anydoc::{Format, to_markdown, to_markdown_bytes};
use std::fs;
use std::path::PathBuf;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn html_fixture(name: &str) -> PathBuf {
    fixture_root().join("html").join(name)
}

fn assert_both_contain(source: &str, html: &str, needles: &[&str]) {
    for needle in needles {
        assert!(source.contains(needle), "source Markdown missing {needle:?}");
        assert!(html.contains(needle), "HTML Markdown missing {needle:?}");
    }
}

#[test]
fn controlled_document_preserves_rich_text_links_and_unicode() {
    let bytes = fs::read(html_fixture("controlled-document.html")).unwrap();
    let markdown = to_markdown_bytes(&bytes, Some(Format::Html)).unwrap();
    assert_eq!(
        markdown,
        "# Fixture Document\n\nPlain paragraph with **bold**, *italic*, and [example](https://example.com/page).\n\n## Unicode\n\ncafé Ł music 𝄞 family 👨‍👩‍👧.\n\n**Styled bold paragraph.**\n"
    );
}

#[test]
fn controlled_nested_lists_preserve_numbering_and_structure() {
    let bytes = fs::read(html_fixture("controlled-lists.html")).unwrap();
    let markdown = to_markdown_bytes(&bytes, Some(Format::Html)).unwrap();
    assert_eq!(markdown, "## Lists\n\n3. Third\n\n4. Fourth\n\n   - Nested A\n   - Nested B\n");
}

#[test]
fn controlled_spanned_table_matches_existing_xlsx_semantics() {
    let html = to_markdown(html_fixture("controlled-merged-table.html")).unwrap();
    let source = to_markdown(fixture_root().join("xlsx").join("handmade-merged.xlsx")).unwrap();
    assert_eq!(html, source);
}

#[test]
fn libreoffice_docx_text_preserves_core_document_semantics() {
    let source = to_markdown(fixture_root().join("docx").join("text.docx")).unwrap();
    let html = to_markdown(html_fixture("libreoffice-docx-text.html")).unwrap();

    assert_both_contain(
        &source,
        &html,
        &[
            "# Fixture Document",
            "Plain paragraph with **bold**, *italic*",
            "## Lists",
            "First numbered",
            "## Table",
            "Wide head",
            "Music clef 𝄞",
            "Persian with ZWNJ",
            "Family emoji",
            "[example](https://example.com/page)",
        ],
    );
}

#[test]
fn libreoffice_docx_numbering_preserves_list_content_and_emphasis() {
    let source = to_markdown(fixture_root().join("docx").join("handmade-numbering.docx")).unwrap();
    let html = to_markdown(html_fixture("libreoffice-docx-numbering.html")).unwrap();

    assert_both_contain(
        &source,
        &html,
        &[
            "One-one",
            "One-two",
            "One-two-a roman",
            "Deep bullet",
            "Interruption paragraph.",
            "One-four continues the count",
            "Ten-start via override",
            "pStyle-bound level one",
            "pStyle-bound level two",
            "pStyle-bound level one again",
            "Bold here,",
            "style-false keeps it bold",
            "direct on makes this italic",
        ],
    );
    assert!(html.contains("1. One-one"));
    assert!(html.contains("- Deep bullet"));
}

#[test]
fn libreoffice_xlsx_merged_table_matches_source_markdown() {
    let source = to_markdown(fixture_root().join("xlsx").join("handmade-merged.xlsx")).unwrap();
    let html = to_markdown(html_fixture("libreoffice-xlsx-merged.html")).unwrap();
    assert_eq!(html, source);
}
