use std::path::Path;

#[test]
fn test_rst_conversion_doc1() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc_path = manifest_dir.join("tests-rst [do-not-commit]").join("doc1.rst");
    let markdown = anydoc::to_markdown(&doc_path).expect("Failed to convert doc1.rst");
    assert!(!markdown.is_empty(), "Markdown output should not be empty");
    assert!(markdown.contains("Sample 1"), "Should contain title");
    assert!(markdown.contains("Introduction"), "Should contain section header");
    assert!(markdown.contains("**bold text**"), "Should convert bold text");
    assert!(markdown.contains("*italic text*"), "Should convert italic text");
    assert!(markdown.contains("`inline code`"), "Should convert inline code");
    assert!(markdown.contains("[link](https://example.com)"), "Should convert link");
}

#[test]
fn test_rst_conversion_doc2() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc_path = manifest_dir.join("tests-rst [do-not-commit]").join("doc2.rst");
    let markdown = anydoc::to_markdown(&doc_path).expect("Failed to convert doc2.rst");
    assert!(!markdown.is_empty(), "Markdown output should not be empty");
    assert!(markdown.contains("Sample 2"), "Should contain title");
    assert!(markdown.contains("def hello_world():"), "Should contain Python code");
    assert!(markdown.contains("```python"), "Should render python codeblock");
    assert!(markdown.contains("Header 1"), "Should contain table header");
}

#[test]
fn test_rst_conversion_doc3() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc_path = manifest_dir.join("tests-rst [do-not-commit]").join("doc3.rst");
    let markdown = anydoc::to_markdown(&doc_path).expect("Failed to convert doc3.rst");
    assert!(!markdown.is_empty(), "Markdown output should not be empty");
    assert!(markdown.contains("Sample 3"), "Should contain title");
    assert!(markdown.contains("Anydoc"), "Should handle substitution replacement");
    assert!(markdown.contains("Custom Title"), "Should contain custom admonition title");
}
