//! Standalone HTML: browser-grade HTML5 parsing followed by the shared
//! semantic HTML -> document-model frontend used by EPUB.

use crate::error::ConvertError;
use crate::model::{AnchorId, Document, ImageSource, LinkTarget};
use crate::package::limits;
use crate::package::xml::{Attr, Element, Node};
use crate::shared::html::{HtmlCtx, Stylesheet};
use crate::shared::uri::is_absolute_uri;
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, WINDOWS_1252};
use html5ever::LocalName;
use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::{Rawtext, Rcdata, ScriptData};
use html5ever::tokenizer::{
    BufferQueue, EndTag, StartTag, TagToken, Token, TokenSink, TokenSinkResult, Tokenizer,
};
use scraper::{ElementRef, Html, Node as HtmlNode};
use std::cell::{Cell, RefCell};
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
    preflight_html_complexity(&text)?;

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
    const SNIFF_BYTES: usize = 64 * 1024;
    let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(SNIFF_BYTES)]);
    let parsed = Html::parse_document(prefix.as_ref());
    let root = parsed.root_element();

    for meta in root.descendent_elements().filter(|element| element.value().name() == "meta") {
        if let Some(label) = meta.value().attr("charset")
            && let Some(encoding) = Encoding::for_label(label.trim().as_bytes())
        {
            return Some(encoding);
        }

        let is_content_type = meta
            .value()
            .attr("http-equiv")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("content-type"));
        if is_content_type
            && let Some(content) = meta.value().attr("content")
            && let Some(label) = content_type_charset(content)
            && let Some(encoding) = Encoding::for_label(label.as_bytes())
        {
            return Some(encoding);
        }
    }
    None
}

fn content_type_charset(content: &str) -> Option<&str> {
    let mut start = 0usize;
    let mut quote = None;
    for (index, ch) in content.char_indices() {
        match (quote, ch) {
            (Some(active), current) if current == active => quote = None,
            (None, '\'' | '"') => quote = Some(ch),
            (None, ';') => {
                if let Some(label) = charset_parameter(&content[start..index]) {
                    return Some(label);
                }
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    charset_parameter(&content[start..])
}

fn charset_parameter(parameter: &str) -> Option<&str> {
    let (name, value) = parameter.split_once('=')?;
    if !name.trim().eq_ignore_ascii_case("charset") {
        return None;
    }
    let label = value.trim().trim_matches(|c| c == '\'' || c == '"').trim();
    (!label.is_empty()).then_some(label)
}

#[derive(Default)]
struct HtmlComplexitySink {
    node_count: Cell<usize>,
    open_elements: RefCell<Vec<LocalName>>,
    node_limit_exceeded: Cell<bool>,
    depth_limit_exceeded: Cell<bool>,
}

impl HtmlComplexitySink {
    fn bump_node(&self) {
        let count = self.node_count.get().saturating_add(1);
        self.node_count.set(count);
        if count > limits::MAX_XML_NODES {
            self.node_limit_exceeded.set(true);
        }
    }

    fn push_element(&self, name: &LocalName) {
        let mut open = self.open_elements.borrow_mut();
        close_implied_before_start(&mut open, name.as_ref());
        open.push(name.clone());
        if open.len() > limits::MAX_XML_DEPTH {
            self.depth_limit_exceeded.set(true);
        }
    }

    fn close_element(&self, name: &LocalName) {
        let mut open = self.open_elements.borrow_mut();
        if let Some(position) = open.iter().rposition(|candidate| candidate == name) {
            open.truncate(position);
        }
    }
}

impl TokenSink for HtmlComplexitySink {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<Self::Handle> {
        match token {
            TagToken(tag) => match tag.kind {
                StartTag => {
                    self.bump_node();
                    let name = tag.name.as_ref();
                    if !tag.self_closing && !is_void_html_element(name) {
                        self.push_element(&tag.name);
                    }
                    match name {
                        "title" | "textarea" => TokenSinkResult::RawData(Rcdata),
                        "style" | "xmp" | "iframe" | "noembed" | "noframes" => {
                            TokenSinkResult::RawData(Rawtext)
                        }
                        "script" => TokenSinkResult::RawData(ScriptData),
                        "plaintext" => TokenSinkResult::Plaintext,
                        _ => TokenSinkResult::Continue,
                    }
                }
                EndTag => {
                    self.close_element(&tag.name);
                    TokenSinkResult::Continue
                }
            },
            Token::CharacterTokens(text) => {
                if !text.is_empty() {
                    self.bump_node();
                }
                TokenSinkResult::Continue
            }
            Token::CommentToken(_) | Token::DoctypeToken(_) | Token::NullCharacterToken => {
                self.bump_node();
                TokenSinkResult::Continue
            }
            Token::EOFToken | Token::ParseError(_) => TokenSinkResult::Continue,
        }
    }
}

fn close_implied_before_start(open: &mut Vec<LocalName>, name: &str) {
    let implied = match name {
        "li" => &["li"][..],
        "p" => &["p"][..],
        "dt" | "dd" => &["dt", "dd"][..],
        "rt" | "rp" => &["rt", "rp"][..],
        "option" => &["option"][..],
        "optgroup" => &["option", "optgroup"][..],
        "tr" => &["tr"][..],
        "td" | "th" => &["td", "th"][..],
        _ => return,
    };
    if let Some(position) = open.iter().rposition(|candidate| implied.contains(&candidate.as_ref()))
    {
        open.truncate(position);
    }
}

fn is_void_html_element(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn preflight_html_complexity(text: &str) -> Result<(), ConvertError> {
    const CHUNK_BYTES: usize = 64 * 1024;
    let tokenizer = Tokenizer::new(HtmlComplexitySink::default(), Default::default());
    let input = BufferQueue::default();
    let mut offset = 0usize;

    while offset < text.len() {
        let mut end = offset.saturating_add(CHUNK_BYTES).min(text.len());
        while end > offset && !text.is_char_boundary(end) {
            end -= 1;
        }
        input.push_back(StrTendril::from(&text[offset..end]));
        let _ = tokenizer.feed(&input);
        check_preflight_limits(&tokenizer.sink)?;
        offset = end;
    }

    tokenizer.end();
    check_preflight_limits(&tokenizer.sink)
}

fn check_preflight_limits(sink: &HtmlComplexitySink) -> Result<(), ConvertError> {
    if sink.node_limit_exceeded.get() {
        return Err(ConvertError::ResourceLimit {
            limit: "max_xml_nodes",
            detail: format!("HTML token stream has more than {} nodes", limits::MAX_XML_NODES),
        });
    }
    if sink.depth_limit_exceeded.get() {
        return Err(ConvertError::ResourceLimit {
            limit: "max_xml_depth",
            detail: format!(
                "HTML source nesting depth exceeds {} before DOM construction",
                limits::MAX_XML_DEPTH
            ),
        });
    }
    Ok(())
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
        Ok((is_absolute_uri(src) || src.starts_with("//"))
            .then(|| ImageSource::External(src.to_owned())))
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

    #[test]
    fn charset_sniff_ignores_non_meta_text_and_attributes() {
        assert_eq!(sniff_meta_charset(b"<p data-note='charset=windows-1252'>utf-8</p>"), None);
        assert_eq!(sniff_meta_charset(b"<!-- <meta charset=windows-1252> --><p>utf-8</p>"), None);
        assert_eq!(
            sniff_meta_charset(b"<script>const x='<meta charset=windows-1252>';</script>"),
            None
        );
    }
}
