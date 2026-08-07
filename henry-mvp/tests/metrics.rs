use anydoc_henry_mvp::summarize_markdown;

#[test]
fn counts_markdown_structure_without_copying_content() {
    let stats = summarize_markdown("# Title\n\n- one\n- two\n\n| A | B |\n| - | - |\n| 1 | 2 |\n");
    assert_eq!(stats.headings, 1);
    assert_eq!(stats.list_items, 2);
    assert_eq!(stats.table_rows, 3);
    assert!(stats.markdown_chars > 20);
}
