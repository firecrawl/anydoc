//! Standalone HTML: browser-grade HTML5 parsing followed by the shared
//! semantic HTML -> document-model frontend used by EPUB.

use crate::error::ConvertError;
use crate::model::{AnchorId, Document, ImageSource, LinkTarget};
use crate::package::limits;
use crate::package::xml::{Attr, Element, Node};
use crate::shared::html::{HtmlCtx, Stylesheet};
use crate::shared::uri::is_absolute_uri;
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, WINDOWS_1252};
use scraper::{ElementRef, Html, Node as HtmlNode};
use std::rc::Rc;

/// Parse a standalone HTML document into anydoc's document model.
///
/// HTML5 tree construction is delegated to `scraper`/`html5ever`; the
/// resulting DOM is adapted into the small namespace-aware tree already used
/// by `shared::html`, so HTML and EPUB keep one semantic conversion path.
pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    if bytes.len() as u64 > limits::MAX_ENTRY_BYTES {
        return Err(ConvertError::ResourceLimit {
            limit: "max_entry_bytes",
            detail: format!(
                "HTML input is {} bytes; maximum is {}",
                bytes.len(),
                limits::MAX_ENTRY_BYTES
            ),
        });
    }

    let text = decode_html(bytes);
    let parsed = Html::parse_document(&text);
    let root = parsed.root_element();

    let mut css = Stylesheet::default();
    for style in root.descendent_elements().filter(|e| e.value().name() == "style") {
        css.add(&style.text().collect::<String>());
    }

    let body = root
        .descendent_elements()
        .find(|e| e.value().name() == "body")
        .ok_or_else(|| ConvertError::malformed("HTML parser produced no body element"))?;

    let mut node_count = 0usize;
    let body = adapt_element(body, 1, &mut node_count)?;
    let blocks = crate::shared::html::to_blocks(&body, &css, &StandaloneCtx)?;

    Ok(Document { blocks, ..Document::default() })
}

fn decode_html(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return UTF_16LE.decode(rest).0.into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return UTF_16BE.decode(rest).0.into_owned();
    }
    if let Some(encoding) = sniff_meta_charset(bytes) {
        return encoding.decode(bytes).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => WINDOWS_1252.decode(bytes).0.into_owned(),
    }
}

fn sniff_meta_charset(bytes: &[u8]) -> Option<&'static Encoding> {
    const SNIFF_BYTES: usize = 1024;
    let mut prefix = bytes[..bytes.len().min(SNIFF_BYTES)].to_vec();
    prefix.make_ascii_lowercase();

    let mut offset = 0usize;
    while let Some(found) = find_bytes(&prefix[offset..], b"charset") {
        let mut pos = offset + found + b"charset".len();
        while prefix.get(pos).is_some_and(u8::is_ascii_whitespace) {
            pos += 1;
        }
        if prefix.get(pos) != Some(&b'=') {
            offset = offset + found + 1;
            continue;
        }
        pos += 1;
        while prefix.get(pos).is_some_and(u8::is_ascii_whitespace) {
            pos += 1;
        }
        let quote = match prefix.get(pos) {
            Some(b'\'') | Some(b'"') => {
                let q = prefix[pos];
                pos += 1;
                Some(q)
            }
            _ => None,
        };
        let start = pos;
        while let Some(&byte) = prefix.get(pos) {
            let stop = quote.map_or_else(
                || byte.is_ascii_whitespace() || matches!(byte, b';' | b'>' | b'/' | b'\'' | b'"'),
                |q| byte == q,
            );
            if stop {
                break;
            }
            pos += 1;
        }
        if pos > start
            && let Some(encoding) = Encoding::for_label(&prefix[start..pos])
        {
            return Some(encoding);
        }
        offset = offset + found + 1;
    }
    None
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn adapt_element(
    source: ElementRef<'_>,
    depth: usize,
    node_count: &mut usize,
) -> Result<Element, ConvertError> {
    if depth > limits::MAX_XML_DEPTH {
        return Err(ConvertError::ResourceLimit {
            limit: "max_xml_depth",
            detail: format!("HTML element nesting depth {depth} exceeds {}", limits::MAX_XML_DEPTH),
        });
    }
    bump_node_count(node_count)?;

    let value = source.value();
    let attrs = value
        .attrs
        .iter()
        .map(|(name, value)| Attr {
            ns: optional_namespace(name.ns.as_ref()),
            local: name.local.as_ref().to_owned(),
            value: value.to_string(),
        })
        .collect();

    let mut children = Vec::new();
    for child in source.children() {
        match child.value() {
            HtmlNode::Element(_) => {
                if let Some(element) = ElementRef::wrap(child) {
                    children.push(Node::Elem(adapt_element(element, depth + 1, node_count)?));
                }
            }
            HtmlNode::Text(text) => {
                bump_node_count(node_count)?;
                children.push(Node::Text(text.text.to_string()));
            }
            HtmlNode::Comment(_) | HtmlNode::Doctype(_) | HtmlNode::ProcessingInstruction(_) => {
                bump_node_count(node_count)?;
            }
            HtmlNode::Document | HtmlNode::Fragment => {}
        }
    }

    Ok(Element {
        ns: optional_namespace(value.name.ns.as_ref()),
        local: value.name.local.as_ref().to_owned(),
        attrs,
        children,
    })
}

fn optional_namespace(namespace: &str) -> Option<Rc<str>> {
    (!namespace.is_empty()).then(|| Rc::<str>::from(namespace))
}

fn bump_node_count(node_count: &mut usize) -> Result<(), ConvertError> {
    *node_count = node_count.saturating_add(1);
    if *node_count > limits::MAX_XML_NODES {
        return Err(ConvertError::ResourceLimit {
            limit: "max_xml_nodes",
            detail: format!("HTML tree has more than {} nodes", limits::MAX_XML_NODES),
        });
    }
    Ok(())
}

struct StandaloneCtx;

impl HtmlCtx for StandaloneCtx {
    fn link_target(&self, href: &str) -> Option<LinkTarget> {
        let href = href.trim();
        if href.is_empty() {
            return None;
        }
        if let Some(fragment) = href.strip_prefix('#') {
            let fragment = crate::package::path::decode_fragment(fragment);
            return Some(LinkTarget::Anchor(fragment));
        }
        Some(if is_absolute_uri(href) {
            LinkTarget::External(href.to_owned())
        } else {
            LinkTarget::Relative(href.to_owned())
        })
    }

    fn image_source(&self, src: &str) -> Result<Option<ImageSource>, ConvertError> {
        let src = src.trim();
        if src.is_empty() {
            return Ok(None);
        }
        Ok(is_absolute_uri(src).then(|| ImageSource::External(src.to_owned())))
    }

    fn anchor_id(&self, raw: &str) -> AnchorId {
        raw.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charset_sniff_accepts_meta_and_http_equiv_forms() {
        assert_eq!(
            sniff_meta_charset(b"<meta charset='windows-1252'>").map(Encoding::name),
            Some("windows-1252")
        );
        assert_eq!(
            sniff_meta_charset(
                b"<meta http-equiv=content-type content='text/html; charset=iso-8859-1'>"
            )
            .map(Encoding::name),
            Some("windows-1252")
        );
    }
}
