mod common;

use anydoc::{Format, to_markdown, to_markdown_bytes};
use common::fixture_root;
use std::fs;
use std::path::PathBuf;

fn html_fixture(name: &str) -> PathBuf {
    fixture_root().join("html").join(name)
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
    assert_eq!(
        markdown,
        "## Lists\n\n3. Third\n\n4. Fourth\n\n   - Nested A\n\n   - Nested B\n"
    );
}

#[test]
fn controlled_spanned_table_matches_existing_xlsx_semantics() {
    let html = to_markdown(html_fixture("controlled-merged-table.html")).unwrap();
    let source = to_markdown(fixture_root().join("xlsx").join("handmade-merged.xlsx")).unwrap();
    assert_eq!(html, source);
}
