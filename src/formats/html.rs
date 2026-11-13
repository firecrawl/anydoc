//! Standalone HTML documents, including HTML exports saved with a `.doc`
//! extension.
//!
//! The DOM and block walker are shared with EPUB. Standalone documents have
//! no package base path, so external links and images are preserved while
//! relative resources degrade to their text/alt content.

use crate::error::ConvertError;
use crate::model::{AnchorId, Document, ImageSource, LinkTarget};
use crate::package::path::decode_fragment;
use crate::package::xml::{Element, Node, parse_xml};
use crate::shared::html::{HtmlCtx, Stylesheet, to_blocks};
use crate::shared::uri::is_absolute_uri;

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let mut tree = parse_xml(bytes)?;
    normalize_html_tree(&mut tree);

    let html = tree.child_elems().find(|e| e.local == "html").unwrap_or(&tree);
    let body = html.child_elems().find(|e| e.local == "body").unwrap_or(html);

    let mut css = Stylesheet::default();
    for style in tree.descendants_any("style") {
        css.add(&style.text());
    }

    let ctx = StandaloneCtx;
    Ok(Document { blocks: to_blocks(body, &css, &ctx)?, ..Document::default() })
}

/// HTML tag and attribute names are ASCII case-insensitive. Normalizing the
/// small DOM once lets the existing EPUB walker handle HTML exports from
/// producers that use uppercase names without adding case checks to every
/// element lookup.
fn normalize_html_tree(elem: &mut Element) {
    elem.local.make_ascii_lowercase();
    for attr in &mut elem.attrs {
        attr.local.make_ascii_lowercase();
    }
    for child in &mut elem.children {
        if let Node::Elem(child) = child {
            normalize_html_tree(child);
        }
    }
}

struct StandaloneCtx;

impl HtmlCtx for StandaloneCtx {
    fn link_target(&self, href: &str) -> Option<LinkTarget> {
        if href.is_empty() {
            return None;
        }
        if let Some(fragment) = href.strip_prefix('#') {
            let fragment = decode_fragment(fragment);
            return (!fragment.is_empty()).then_some(LinkTarget::Anchor(fragment));
        }
        if is_absolute_uri(href) {
            Some(LinkTarget::External(href.to_string()))
        } else {
            Some(LinkTarget::Relative(href.to_string()))
        }
    }

    fn image_source(&self, src: &str) -> Result<Option<ImageSource>, ConvertError> {
        if is_absolute_uri(src) {
            Ok(Some(ImageSource::External(src.to_string())))
        } else {
            // Resolving a relative resource requires the source file's path;
            // the byte-oriented public API intentionally has no such base.
            Ok(None)
        }
    }

    fn anchor_id(&self, raw: &str) -> AnchorId {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::markdown::document_to_markdown;

    #[test]
    fn jira_style_doc_extension_html_converts() {
        let html = br#"<!DOCTYPE html>
<html>
<head>
  <meta http-equiv="Content-Type" Content="application/vnd.ms-word; charset=UTF-8">
  <style> .hidden { display: none; } </style>
</head>
<body>
  <h1 id="title">Issue</h1>
  <p>Status: <b>Open</b></p>
  <p class="hidden">not visible</p>
  <p><a href="https://example.com">Tracker</a> <img src="https://example.com/icon.png" alt="icon"></p>
</body>
</html>"#;

        assert_eq!(crate::Format::from_bytes(html), Some(crate::Format::Html));
        assert_eq!(crate::Format::from_extension("doc"), Some(crate::Format::Doc));
        let doc = parse(html).unwrap();
        assert_eq!(
            document_to_markdown(&doc),
            "# Issue\n\nStatus: **Open**\n\n[Tracker](https://example.com) ![icon](https://example.com/icon.png)\n"
        );
    }

    #[test]
    fn html_names_and_fragment_links_are_case_insensitive_and_decoded() {
        let html = br##"<HTML><BODY><H2 ID="A B">Target</H2><P><A HREF="#A%20B">jump</A></P></BODY></HTML>"##;
        let doc = parse(html).unwrap();
        assert_eq!(document_to_markdown(&doc), "## Target\n\n[jump](#target)\n");
    }
}
