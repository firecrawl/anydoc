use anydoc::{Format, to_markdown_bytes};

#[test]
fn unordered_list_ignores_non_rendering_children_without_splitting() {
    let html = br#"<!doctype html>
        <ul>
          <li>one</li>
          <script>ignored()</script>
          <li>two</li>
        </ul>"#;

    let markdown = to_markdown_bytes(html, Some(Format::Html)).unwrap();
    assert_eq!(markdown, "- one\n- two\n");
}

#[test]
fn ordered_list_ignores_non_rendering_children_without_splitting() {
    let html = br#"<!doctype html>
        <ol>
          <li>one</li>
          <style>.ignored { display: none }</style>
          <li>two</li>
        </ol>"#;

    let markdown = to_markdown_bytes(html, Some(Format::Html)).unwrap();
    assert_eq!(markdown, "1. one\n2. two\n");
}
