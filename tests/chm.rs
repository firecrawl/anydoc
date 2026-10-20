//! CHM conversion contracts. The vendor manual stays outside the fixture corpus.

use anydoc::model::{Block, Document, ImageSource, Inline, LinkTarget, inlines_to_plain_text};
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

#[test]
fn huffman_spaninfo_decodes_frames_across_dictionary_resets() {
    let bytes = std::fs::read(fixture().with_file_name("handmade-huffman-spaninfo.chm")).unwrap();
    assert!(bytes.len() < 10_000, "the original Huffman fixture stays small");
    let md = to_markdown_bytes(&bytes, None).expect("decode Huffman frames without ResetTable");
    assert_eq!(md.trim().len(), 131_072);
    assert_eq!(md.trim(), format!("{}{}", "A".repeat(65_536), "B".repeat(65_536)));
}

fn regression_document(name: &str) -> Document {
    let bytes = std::fs::read(fixture().with_file_name(name)).expect("committed CHM fixture");
    to_document(&bytes, None).expect("convert regression CHM")
}

fn body_inlines(doc: &Document) -> impl Iterator<Item = &Inline> {
    doc.blocks
        .iter()
        .filter_map(|block| match block {
            Block::Heading { content, .. } | Block::Paragraph(content) => Some(content),
            _ => None,
        })
        .flatten()
}

fn link<'a>(doc: &'a Document, label: &str) -> &'a LinkTarget {
    body_inlines(doc)
        .find_map(|inline| match inline {
            Inline::Link { content, target } if inlines_to_plain_text(content) == label => {
                Some(target)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing link {label:?}"))
}

#[test]
fn ascii_topics_use_the_book_encoding_for_percent_encoded_links() {
    let doc = regression_document("handmade-legacy-links.chm");
    assert_eq!(link(&doc, "Legacy link"), &LinkTarget::Anchor("中文.htm#参数".into()));
}

#[test]
fn ascii_toc_uses_the_book_encoding_and_preserves_topic_order() {
    let md = to_markdown(fixture().with_file_name("handmade-legacy-links.chm")).unwrap();
    assert!(md.find("# First topic").unwrap() < md.find("# Second topic").unwrap(), "{md}");
    assert!(!md.contains("%D6%D0%CE%C4"), "TOC links must resolve to converted topics: {md}");
}

#[test]
fn stylesheet_urls_use_the_page_encoding() {
    let md = to_markdown(fixture().with_file_name("handmade-legacy-css.chm")).unwrap();
    assert!(md.contains("VISIBLE-PATH"), "{md}");
    assert!(!md.contains("HIDDEN-PATH"), "page-encoded CSS path must resolve: {md}");
}

#[test]
fn stylesheet_charset_is_independent_of_the_book_encoding() {
    let md = to_markdown(fixture().with_file_name("handmade-legacy-css.chm")).unwrap();
    assert!(md.contains("VISIBLE-CHARSET"), "{md}");
    assert!(!md.contains("HIDDEN-CHARSET"), "CSS @charset must decode the class selector: {md}");
}

#[test]
fn polish_lcid_selects_windows_1250_without_html_metadata() {
    let md = to_markdown(fixture().with_file_name("handmade-polish.chm")).unwrap();
    assert!(md.contains("Składnia"), "{md}");
    assert!(md.contains("Zażółć gęślą jaźń."), "{md}");
    assert!(!md.contains('\u{fffd}'), "{md}");
}

#[test]
fn topic_title_survives_when_the_body_starts_at_a_subheading() {
    let md = to_markdown(fixture().with_file_name("handmade-polish.chm")).unwrap();
    assert!(md.contains("# Command manual\n"), "{md}");
}

#[test]
fn local_html_bases_resolve_links_images_and_stylesheets() {
    let doc = regression_document("handmade-base.chm");
    let md = to_markdown(fixture().with_file_name("handmade-base.chm")).unwrap();
    for kind in ["relative", "root", "same-book"] {
        assert_eq!(
            link(&doc, &format!("Link {kind}")),
            &LinkTarget::Anchor("shared/target.htm#dest".into()),
            "{kind} base must resolve the topic link"
        );
        assert!(body_inlines(&doc).any(|inline| matches!(
            inline,
            Inline::Image { alt, source: ImageSource::Asset(_) } if alt == &format!("Image {kind}")
        )), "{kind} base must resolve the embedded image");
        assert!(!md.contains(&format!("HIDDEN-{kind}")), "{kind} base must resolve CSS: {md}");
    }
    assert_eq!(doc.assets.len(), 1, "three references share one resolved image");
    assert_eq!(doc.assets[0].origin_part, "shared/dot.png");
}

#[test]
fn external_html_base_resolves_urls_and_fragment_references() {
    let doc = regression_document("handmade-base.chm");
    assert_eq!(
        link(&doc, "Link external"),
        &LinkTarget::External("https://example.invalid/shared/target.htm#dest".into())
    );
    assert_eq!(
        link(&doc, "Fragment external"),
        &LinkTarget::External("https://example.invalid/shared/#self".into()),
        "fragment-only references are relative to base href too"
    );
    assert!(
        body_inlines(&doc).any(|inline| matches!(
            inline,
            Inline::Image { alt, source: ImageSource::External(url) }
                if alt == "Image external" && url == "https://example.invalid/shared/dot.png"
        )),
        "external base must resolve an image URL without creating an embedded asset"
    );
}

#[test]
fn html_base_does_not_change_the_scope_of_anchor_definitions() {
    let doc = regression_document("handmade-base.chm");
    for kind in ["relative", "root", "same-book", "external"] {
        let expected = format!("topics/{kind}.htm#self");
        assert!(
            doc.blocks.iter().any(|block| matches!(
                block,
                Block::Heading { anchor: Some(anchor), .. } if anchor == &expected
            )),
            "base href must not move the definition {expected}"
        );
    }
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
