//! CHM conversion contracts. The vendor manual stays outside the fixture corpus.

use anydoc::{ConvertError, Format, to_document, to_markdown, to_markdown_bytes};
use std::path::Path;

fn format() -> Format {
    Format::from_extension("chm").expect("CHM must be a supported input format")
}

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chm/handmade-book.chm")
}

#[test]
fn chm_is_identified_by_content_and_extension() {
    let chm = format();
    assert_eq!(Format::from_extension("CHM"), Some(chm));
    assert_eq!(Format::from_path(Path::new("manual.chm")), Some(chm));
    assert_eq!(Format::from_bytes(b"ITSF"), Some(chm));
    assert_eq!(Format::from_bytes(b"ITS"), None);
}

#[test]
fn chm_preserves_topics_links_tables_and_assets() {
    let bytes = std::fs::read(fixture()).expect("committed CHM fixture");
    let md = to_markdown_bytes(&bytes, format()).expect("convert CHM bytes");
    assert_eq!(md, to_markdown_bytes(&bytes, None).unwrap());
    assert_eq!(md, to_markdown(fixture()).unwrap());
    assert!(md.contains("中文说明：配置路由器"), "{md}");
    assert!(md.contains("show version"), "{md}");
    assert!(md.contains("| 命令 | 说明 |"), "{md}");
    assert!(md.contains("[命令语法](#"), "{md}");
    assert!(!md.contains("](a-commands.htm"), "topic links must become anchors: {md}");
    assert!(md.contains("拓扑图"), "{md}");
    let intro = md.find("# 使用指南").expect("intro heading");
    let commands = md.find("命令参考\n").expect("commands heading");
    let extra = md.find("补充说明\n").expect("unlisted topic must survive");
    assert!(intro < commands && commands < extra, "TOC order followed by unlisted topics: {md}");
    assert_eq!(
        md.matches("中文说明：配置路由器").count(),
        1,
        "duplicate TOC entries duplicate no body"
    );
    let doc = to_document(&bytes, None).expect("CHM has a document model");
    assert_eq!(doc.assets.len(), 1, "shared image is retained once");
    assert_eq!(doc.assets[0].media_type, "image/png");
    assert_eq!(doc.assets[0].bytes, include_bytes!("fixture-src/dot.png"));
}

#[test]
fn truncated_chm_returns_a_typed_error() {
    assert!(matches!(
        to_markdown_bytes(b"ITSF\x03\0\0\0", format()),
        Err(ConvertError::Malformed { .. } | ConvertError::MissingPart { .. })
    ));
}

#[test]
fn chm_without_a_toc_has_a_stable_order() {
    let path = fixture().with_file_name("handmade-no-toc.chm");
    let md = to_markdown(path).unwrap();
    assert!(md.find("# First").unwrap() < md.find("# Second").unwrap(), "{md}");
}

#[test]
fn chm_decodes_big5_topics() {
    let path = fixture().with_file_name("handmade-big5.chm");
    let md = to_markdown(path).unwrap();
    assert!(md.contains("繁體中文"), "{md}");
    assert!(md.contains("路由器設定與命令參考"), "{md}");
    assert!(md.contains("[參數連結](#參數)"), "links use the topic encoding: {md}");
    assert!(!md.contains('\u{fffd}'));
}

#[test]
fn compressed_and_stored_chm_have_the_same_document_content() {
    let stored = to_markdown(fixture()).unwrap();
    let compressed = to_markdown(fixture().with_file_name("handmade-compressed.chm")).unwrap();
    assert_eq!(compressed, stored);
}

/// Run with ANYDOC_CHM_SAMPLE pointing to the local ZXR10 command manual:
/// `cargo test --test chm local_chm_manual -- --ignored --nocapture`
#[test]
#[ignore = "requires ANYDOC_CHM_SAMPLE; the external manual is not redistributed"]
fn local_chm_manual() {
    let path = std::env::var_os("ANYDOC_CHM_SAMPLE").expect("set ANYDOC_CHM_SAMPLE");
    let bytes = std::fs::read(&path).expect("read local CHM manual");
    assert_eq!(Format::from_bytes(&bytes), Some(format()));
    let start = std::time::Instant::now();
    let doc = to_document(&bytes, None).expect("parse the real manual into the shared model");
    let topics: std::collections::HashSet<_> = doc
        .blocks
        .iter()
        .filter_map(|block| match block {
            anydoc::model::Block::Paragraph(inlines) => match inlines.as_slice() {
                [anydoc::model::Inline::Anchor(path)] if path.ends_with(".html") => Some(path),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(topics.len(), 15_527, "every HTML topic in this manual must survive");
    assert!(!doc.assets.is_empty(), "the manual's embedded images must survive");
    eprintln!("retained {} topics and {} embedded assets", topics.len(), doc.assets.len());
    drop(doc);
    let md = to_markdown_bytes(&bytes, None).expect("convert the real compressed CHM manual");
    assert!(md.len() > 1_000_000, "a large manual must not be silently shortened");
    assert!(md.contains("ZXR10"), "product name survives decoding");
    assert!(md.contains("命令"), "Chinese text survives decoding");
    assert!(!md.contains('\u{fffd}'), "declared Chinese encodings must decode without replacement");
    eprintln!(
        "converted {} input bytes to {} Markdown bytes in {:?}",
        bytes.len(),
        md.len(),
        start.elapsed()
    );
}
