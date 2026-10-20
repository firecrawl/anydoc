//! Legacy HTML decoding and adaptation to the shared element tree.

use crate::error::ConvertError;
use crate::package::limits;
use crate::package::xml::{Attr, Element, Node};
use encoding_rs::Encoding;
use html5ever::interface::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{BufferQueue, StartTag, Token, TokenSink, TokenSinkResult, Tokenizer};
use html5ever::{Attribute, QualName, parse_document};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

pub(super) fn parse(bytes: &[u8], fallback: &'static Encoding) -> Result<Element, ConvertError> {
    parse_with_encoding(bytes, fallback).map(|(tree, _)| tree)
}

pub(super) fn parse_with_encoding(
    bytes: &[u8],
    fallback: &'static Encoding,
) -> Result<(Element, &'static Encoding), ConvertError> {
    if bytes.len() as u64 > limits::MAX_ENTRY_BYTES {
        return Err(limit_error("max_entry_bytes"));
    }
    let encoding = encoding(bytes, fallback);
    let text = decode_as(bytes, encoding);
    let sink = Sink {
        document: DomNode::new(Kind::Document),
        nodes: Cell::new(1),
        failure: Rc::new(Cell::new(None)),
    };
    let failure = Rc::clone(&sink.failure);
    let mut parser = parse_document(sink, Default::default());
    // Check the sink between bounded chunks: malformed HTML cannot keep
    // building a DOM after the first exhausted resource budget.
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + 8192).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        parser.process(text[start..end].into());
        if let Some(limit) = failure.get() {
            return Err(limit_error(limit));
        }
        start = end;
    }
    let document = parser.finish();
    if let Some(limit) = failure.get() {
        return Err(limit_error(limit));
    }
    // HTML's adoption/reparenting rules can move a whole subtree. Validate
    // the final depth too, before the shared recursive element walker runs.
    let mut stack = vec![(Rc::clone(&document), 0usize)];
    while let Some((node, depth)) = stack.pop() {
        let depth = depth + usize::from(matches!(node.kind, Kind::Element { .. }));
        if depth > limits::MAX_XML_DEPTH {
            return Err(limit_error("max_xml_depth"));
        }
        stack.extend(node.children.borrow().iter().map(|child| (Rc::clone(child), depth)));
    }
    Ok((
        Element {
            ns: None,
            local: String::new(),
            attrs: Vec::new(),
            children: document.children.borrow().iter().filter_map(adapt).collect(),
        },
        encoding,
    ))
}

fn limit_error(limit: &'static str) -> ConvertError {
    ConvertError::ResourceLimit {
        limit,
        detail: "CHM HTML exceeds the document parsing budget".into(),
    }
}

/// HTML metadata is ASCII even when the body is not. Sniff it before
/// decoding; unlike XML, old HTML commonly declares its charset in a meta tag.
pub(super) fn decode(bytes: &[u8], fallback: &'static Encoding) -> String {
    decode_as(bytes, encoding(bytes, fallback))
}

pub(super) fn decode_stylesheet(bytes: &[u8], fallback: &'static Encoding) -> String {
    let encoding = Encoding::for_bom(bytes)
        .map(|(enc, _)| enc)
        .or_else(|| {
            stylesheet_charset(bytes).map(|(encoding, _)| {
                // An ASCII @charset declaration cannot describe UTF-16 bytes.
                if encoding == encoding_rs::UTF_16LE || encoding == encoding_rs::UTF_16BE {
                    encoding_rs::UTF_8
                } else {
                    encoding
                }
            })
        })
        .unwrap_or(fallback);
    let mut text = decode_as(bytes, encoding);
    // The shared CSS subset starts with selectors, so remove the consumed
    // declaration before handing it the first rule. Inspect decoded text
    // too: a BOM may have taken precedence over the declared encoding.
    if let Some((_, end)) = stylesheet_charset(text.as_bytes()) {
        text.drain(..end);
    }
    text
}

fn stylesheet_charset(bytes: &[u8]) -> Option<(&'static Encoding, usize)> {
    // CSS's byte-level encoding declaration must be the first rule,
    // with this exact syntax. HTML meta tags, comments, and strings
    // in a stylesheet do not declare its encoding.
    const PREFIX: &[u8] = b"@charset \"";
    let declaration = bytes[..bytes.len().min(1024)].strip_prefix(PREFIX)?;
    let end = declaration.iter().position(|&byte| byte == b'"')?;
    if declaration.get(end + 1) != Some(&b';') {
        return None;
    }
    let encoding = Encoding::for_label(&declaration[..end])?;
    Some((encoding, PREFIX.len() + end + 2))
}

pub(super) fn encoding(bytes: &[u8], fallback: &'static Encoding) -> &'static Encoding {
    // ASCII and some legacy byte sequences are also valid UTF-8. Keep the
    // book's encoding unless the page explicitly declares another one: it
    // also governs percent-encoded URLs in otherwise ASCII-only pages.
    Encoding::for_bom(bytes)
        .map(|(enc, _)| enc)
        .or_else(|| sniff_encoding(bytes))
        .unwrap_or(fallback)
}

fn decode_as(bytes: &[u8], encoding: &'static Encoding) -> String {
    let (text, _, errors) = encoding.decode(bytes);
    if errors {
        log::warn!("replaced undecodable characters in CHM HTML or stylesheet");
    }
    text.into_owned()
}

fn sniff_encoding(bytes: &[u8]) -> Option<&'static Encoding> {
    let head: String = bytes[..bytes.len().min(4096)]
        .iter()
        .map(|b| if b.is_ascii() { *b as char } else { ' ' })
        .collect();
    // Retain support for XHTML's XML declaration, restricted to the prolog.
    if let Some(prolog) = head.trim_start().strip_prefix("<?xml")
        && let Some((prolog, _)) = prolog.split_once("?>")
        && let Some(pos) = prolog.find("encoding")
        && let Some(value) = prolog[pos + "encoding".len()..].trim_start().strip_prefix('=')
    {
        let value = value.trim_start().trim_start_matches(['\'', '"']);
        let end = value.find(['\'', '"', ' ', '\t', '\r', '\n']).unwrap_or(value.len());
        if let Some(encoding) = Encoding::for_label(&value.as_bytes()[..end]) {
            return Some(encoding);
        }
    }
    // A bounded tokenizer pass recognizes real attributes and ignores
    // comments/raw text without building a second DOM.
    let tokenizer = Tokenizer::new(CharsetSniff(Cell::new(None)), Default::default());
    let input = BufferQueue::default();
    input.push_back(head.into());
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    tokenizer.sink.0.get()
}

struct CharsetSniff(Cell<Option<&'static Encoding>>);

impl TokenSink for CharsetSniff {
    type Handle = ();

    fn process_token(&self, token: Token, _: u64) -> TokenSinkResult<()> {
        let Token::TagToken(tag) = token else { return TokenSinkResult::Continue };
        if tag.kind != StartTag {
            return TokenSinkResult::Continue;
        }
        match tag.name.as_ref() {
            "script" => return TokenSinkResult::RawData(RawKind::ScriptData),
            "style" | "xmp" | "iframe" | "noembed" | "noframes" | "noscript" => {
                return TokenSinkResult::RawData(RawKind::Rawtext);
            }
            "textarea" | "title" => return TokenSinkResult::RawData(RawKind::Rcdata),
            "plaintext" => return TokenSinkResult::Plaintext,
            "meta" if self.0.get().is_none() => {
                let attr = |name: &str| {
                    tag.attrs.iter().find(|a| &*a.name.local == name).map(|a| &*a.value)
                };
                let label = attr("charset").or_else(|| {
                    if !attr("http-equiv")
                        .is_some_and(|value| value.eq_ignore_ascii_case("content-type"))
                    {
                        return None;
                    }
                    attr("content")?.split(';').find_map(|part| {
                        let (name, value) = part.split_once('=')?;
                        name.trim()
                            .eq_ignore_ascii_case("charset")
                            .then(|| value.trim().trim_matches(['\'', '"']))
                    })
                });
                if let Some(encoding) =
                    label.and_then(|label| Encoding::for_label(label.as_bytes()))
                {
                    self.0.set(Some(encoding));
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

type Handle = Rc<DomNode>;

struct DomNode {
    kind: Kind,
    parent: RefCell<Weak<DomNode>>,
    children: RefCell<Vec<Handle>>,
}

enum Kind {
    Document,
    Element {
        name: QualName,
        attrs: RefCell<Vec<Attribute>>,
        template: Option<Handle>,
        mathml_point: bool,
    },
    Text(RefCell<StrTendril>),
    Ignored,
}

impl DomNode {
    fn new(kind: Kind) -> Handle {
        Rc::new(Self {
            kind,
            parent: RefCell::new(Weak::new()),
            children: RefCell::new(Vec::new()),
        })
    }

    fn detach(node: &Handle) {
        if let Some(parent) = node.parent.borrow().upgrade() {
            parent.children.borrow_mut().retain(|child| !Rc::ptr_eq(child, node));
        }
        *node.parent.borrow_mut() = Weak::new();
    }
}

struct Sink {
    document: Handle,
    nodes: Cell<usize>,
    failure: Rc<Cell<Option<&'static str>>>,
}

impl Sink {
    fn node(&self, kind: Kind) -> Handle {
        self.nodes.set(self.nodes.get().saturating_add(1));
        if self.nodes.get() > limits::MAX_XML_NODES {
            self.failure.set(Some("max_xml_nodes"));
        }
        DomNode::new(kind)
    }

    fn insert(&self, parent: &Handle, child: NodeOrText<Handle>, before: Option<&Handle>) {
        if self.failure.get().is_some() {
            return;
        }
        let child = match child {
            NodeOrText::AppendNode(node) => node,
            NodeOrText::AppendText(text) => {
                let children = parent.children.borrow();
                let index = before
                    .and_then(|sibling| children.iter().position(|n| Rc::ptr_eq(n, sibling)))
                    .unwrap_or(children.len());
                if let Some(previous) = index.checked_sub(1).and_then(|i| children.get(i))
                    && let Kind::Text(contents) = &previous.kind
                {
                    contents.borrow_mut().push_tendril(&text);
                    return;
                }
                drop(children);
                self.node(Kind::Text(RefCell::new(text)))
            }
        };
        if matches!(child.kind, Kind::Ignored) {
            return;
        }
        let mut cursor = Some(Rc::clone(parent));
        let mut depth = usize::from(matches!(child.kind, Kind::Element { .. }));
        while let Some(node) = cursor {
            depth += usize::from(matches!(node.kind, Kind::Element { .. }));
            if depth > limits::MAX_XML_DEPTH {
                self.failure.set(Some("max_xml_depth"));
                return;
            }
            cursor = node.parent.borrow().upgrade();
        }
        DomNode::detach(&child);
        *child.parent.borrow_mut() = Rc::downgrade(parent);
        let mut children = parent.children.borrow_mut();
        let index = before
            .and_then(|sibling| children.iter().position(|n| Rc::ptr_eq(n, sibling)))
            .unwrap_or(children.len());
        children.insert(index, child);
    }
}

impl TreeSink for Sink {
    type Handle = Handle;
    type Output = Handle;
    type ElemName<'a> = &'a QualName;

    fn finish(self) -> Handle {
        self.document
    }
    fn parse_error(&self, _: Cow<'static, str>) {}
    fn get_document(&self) -> Handle {
        Rc::clone(&self.document)
    }
    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        Rc::ptr_eq(x, y)
    }
    fn set_quirks_mode(&self, _: QuirksMode) {}

    fn elem_name<'a>(&'a self, target: &'a Handle) -> &'a QualName {
        match &target.kind {
            Kind::Element { name, .. } => name,
            _ => unreachable!("HTML tree builder requested a non-element name"),
        }
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        self.node(Kind::Element {
            name,
            attrs: RefCell::new(attrs),
            template: flags.template.then(|| self.node(Kind::Document)),
            mathml_point: flags.mathml_annotation_xml_integration_point,
        })
    }

    fn create_comment(&self, _: StrTendril) -> Handle {
        self.node(Kind::Ignored)
    }
    fn create_pi(&self, _: StrTendril, _: StrTendril) -> Handle {
        self.node(Kind::Ignored)
    }
    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        self.insert(parent, child, None);
    }
    fn append_before_sibling(&self, sibling: &Handle, child: NodeOrText<Handle>) {
        if let Some(parent) = sibling.parent.borrow().upgrade() {
            self.insert(&parent, child, Some(sibling));
        }
    }
    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        previous: &Handle,
        child: NodeOrText<Handle>,
    ) {
        if element.parent.borrow().upgrade().is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(previous, child);
        }
    }
    fn append_doctype_to_document(&self, _: StrTendril, _: StrTendril, _: StrTendril) {}
    fn get_template_contents(&self, target: &Handle) -> Handle {
        match &target.kind {
            Kind::Element { template: Some(contents), .. } => Rc::clone(contents),
            _ => unreachable!("HTML tree builder requested a non-template's contents"),
        }
    }
    fn is_mathml_annotation_xml_integration_point(&self, target: &Handle) -> bool {
        matches!(&target.kind, Kind::Element { mathml_point: true, .. })
    }
    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        if let Kind::Element { attrs: existing, .. } = &target.kind {
            let mut existing = existing.borrow_mut();
            for attr in attrs {
                if !existing.iter().any(|a| a.name == attr.name) {
                    existing.push(attr);
                }
            }
        }
    }
    fn remove_from_parent(&self, target: &Handle) {
        DomNode::detach(target);
    }
    fn reparent_children(&self, node: &Handle, parent: &Handle) {
        let children = std::mem::take(&mut *node.children.borrow_mut());
        for child in children {
            self.append(parent, NodeOrText::AppendNode(child));
        }
    }
}

fn adapt(node: &Handle) -> Option<Node> {
    match &node.kind {
        Kind::Element { name, attrs, .. } => Some(Node::Elem(Element {
            ns: (!name.ns.is_empty()).then(|| Rc::from(name.ns.as_ref())),
            local: name.local.to_string(),
            attrs: attrs
                .borrow()
                .iter()
                .map(|attr| Attr {
                    ns: (!attr.name.ns.is_empty()).then(|| Rc::from(attr.name.ns.as_ref())),
                    local: attr.name.local.to_string(),
                    value: attr.value.to_string(),
                })
                .collect(),
            children: node.children.borrow().iter().filter_map(adapt).collect(),
        })),
        Kind::Text(text) => Some(Node::Text(text.borrow().to_string())),
        Kind::Document | Kind::Ignored => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AnchorId, Block, ImageSource, LinkTarget, inlines_to_plain_text};
    use crate::shared::html::{HtmlCtx, Stylesheet, to_blocks};

    struct Ctx;

    impl HtmlCtx for Ctx {
        fn link_target(&self, href: &str) -> Option<LinkTarget> {
            Some(LinkTarget::Relative(href.into()))
        }
        fn image_source(&self, _: &str) -> Result<Option<ImageSource>, ConvertError> {
            Ok(None)
        }
        fn anchor_id(&self, raw: &str) -> AnchorId {
            raw.into()
        }
    }

    #[test]
    fn old_html_recovers_void_tags_unquoted_attributes_and_omitted_end_tags() {
        let tree = parse(
            b"<HTML><BODY><P ID=first>One<BR>two<P>Next &copy;<UL><LI>A<LI>B</UL>",
            encoding_rs::WINDOWS_1252,
        )
        .unwrap();
        let body = tree.descendants_any("body").next().unwrap();
        let blocks = to_blocks(body, &Stylesheet::default(), &Ctx).unwrap();
        let Block::Paragraph(first) = &blocks[0] else { panic!("first paragraph") };
        assert_eq!(inlines_to_plain_text(first), "One\ntwo");
        let Block::Paragraph(second) = &blocks[1] else { panic!("second paragraph") };
        assert_eq!(inlines_to_plain_text(second), "Next ©");
        let Block::List(list) = &blocks[2] else { panic!("list") };
        assert_eq!(list.items.len(), 2);
        assert_eq!(body.descendants_any("p").next().unwrap().attr_any("id"), Some("first"));
    }

    #[test]
    fn html_meta_decodes_gbk_and_big5_without_an_xml_prolog() {
        for (encoding, label, text) in [
            (encoding_rs::GBK, "gb2312", "中文说明：配置路由器"),
            (encoding_rs::BIG5, "big5", "繁體中文說明"),
        ] {
            let input = format!(
                "<META HTTP-EQUIV=Content-Type CONTENT='text/html; charset={label}'><P>{text}"
            );
            let (bytes, _, errors) = encoding.encode(&input);
            assert!(!errors);
            let tree = parse(&bytes, encoding_rs::WINDOWS_1252).unwrap();
            assert!(tree.text().contains(text), "{}", tree.text());
        }
    }

    #[test]
    fn html_uses_the_book_encoding_when_no_charset_is_declared() {
        let (bytes, _, _) = encoding_rs::GBK.encode("<P>配置路由器");
        let tree = parse(&bytes, encoding_rs::GBK).unwrap();
        assert!(tree.text().contains("配置路由器"));
    }

    #[test]
    fn ascii_html_retains_the_book_encoding_for_percent_encoded_links() {
        for fallback in [encoding_rs::GBK, encoding_rs::BIG5, encoding_rs::WINDOWS_1250] {
            let (_, selected) =
                parse_with_encoding(b"<a href='%C2%A9.htm'>Topic</a>", fallback).unwrap();
            assert_eq!(selected, fallback);
        }
    }

    #[test]
    fn undeclared_html_does_not_guess_utf8_from_ambiguous_legacy_bytes() {
        for fallback in [encoding_rs::GBK, encoding_rs::BIG5, encoding_rs::WINDOWS_1250] {
            let input = b"<p>\xc2\xa9";
            let (tree, selected) = parse_with_encoding(input, fallback).unwrap();
            let (expected, _, errors) = fallback.decode(b"\xc2\xa9");
            assert!(!errors);
            assert_eq!(selected, fallback);
            assert_eq!(tree.text(), expected);
        }
    }

    #[test]
    fn explicit_html_encodings_take_precedence_over_the_book_encoding() {
        for input in [
            "\u{feff}<meta charset=big5><p>é",
            "<meta charset=utf-8><p>é",
            "<?xml version='1.0' encoding='utf-8'?><p>é",
        ] {
            let (tree, selected) = parse_with_encoding(input.as_bytes(), encoding_rs::GBK).unwrap();
            assert_eq!(selected, encoding_rs::UTF_8);
            assert!(tree.text().contains('é'));
        }
    }

    #[test]
    fn stylesheets_use_css_charset_declarations_before_the_fallback() {
        for (encoding, label, selector) in [
            (encoding_rs::GBK, "gbk", "隐藏"),
            (encoding_rs::BIG5, "big5", "隱藏"),
            (encoding_rs::WINDOWS_1250, "windows-1250", "ukryć"),
        ] {
            let input = format!("@charset \"{label}\"; .{selector} {{ display: none; }}");
            let (bytes, _, errors) = encoding.encode(&input);
            assert!(!errors);
            assert_eq!(
                decode_stylesheet(&bytes, encoding_rs::WINDOWS_1252),
                input.split_once(';').unwrap().1
            );
        }
    }

    #[test]
    fn stylesheet_bom_wins_over_charset_and_html_meta_is_not_a_declaration() {
        assert_eq!(
            decode_stylesheet(
                "\u{feff}@charset \"big5\"; .隱藏 { display: none; }".as_bytes(),
                encoding_rs::WINDOWS_1252,
            ),
            " .隱藏 { display: none; }"
        );
        for prefix in [
            "/* <meta charset=windows-1252> */",
            "/* @charset \"windows-1252\"; */",
            "p::before { content: '<meta charset=windows-1252>'; }",
        ] {
            let input = format!("{prefix} .隐藏 {{ display: none; }}");
            let (bytes, _, errors) = encoding_rs::GBK.encode(&input);
            assert!(!errors);
            assert_eq!(decode_stylesheet(&bytes, encoding_rs::GBK), input);
        }
    }

    #[test]
    fn css_charset_does_not_hide_the_first_rule_from_the_shared_stylesheet() {
        for (encoding, prefix, selector) in [
            (encoding_rs::GBK, "@charset \"gbk\";", "隐藏"),
            (encoding_rs::BIG5, "@charset \"big5\";", "隱藏"),
            (encoding_rs::WINDOWS_1250, "@charset \"windows-1250\";", "ukryć"),
            (encoding_rs::UTF_8, "\u{feff}@charset \"big5\";", "隱藏"),
        ] {
            let input = format!("{prefix} .{selector} {{ display: none; }}");
            let (bytes, _, errors) = encoding.encode(&input);
            assert!(!errors);
            let mut stylesheet = Stylesheet::default();
            stylesheet.add(&decode_stylesheet(&bytes, encoding_rs::WINDOWS_1252));
            let input = format!("<p class='{selector}'>Hidden</p><p>Visible</p>");
            let tree = parse(input.as_bytes(), encoding_rs::UTF_8).unwrap();
            let body = tree.descendants_any("body").next().unwrap();
            let blocks = to_blocks(body, &stylesheet, &Ctx).unwrap();
            assert_eq!(blocks.len(), 1, "{prefix}");
            let Block::Paragraph(text) = &blocks[0] else { panic!("visible paragraph") };
            assert_eq!(inlines_to_plain_text(text), "Visible");
        }
    }

    #[test]
    fn malformed_or_misplaced_css_charset_text_is_preserved() {
        for input in [
            "@charset \"unknown-codec\"; .hidden { display: none; }",
            "@charset 'gbk'; .hidden { display: none; }",
            "@charset \"gbk\" .hidden { display: none; }",
            " @charset \"gbk\"; .hidden { display: none; }",
        ] {
            assert_eq!(decode_stylesheet(input.as_bytes(), encoding_rs::GBK), input);
        }
    }

    #[test]
    fn charset_sniff_ignores_comments_and_script_text() {
        for prefix in [
            "<!-- <meta charset=windows-1252> -->",
            r#"<script>const text = "<meta charset=windows-1252>";</script>"#,
        ] {
            let input = format!("{prefix}<meta charset=gbk><p>中文说明：配置路由器");
            let (bytes, _, errors) = encoding_rs::GBK.encode(&input);
            assert!(!errors);
            let tree = parse(&bytes, encoding_rs::WINDOWS_1252).unwrap();
            let body = tree.descendants_any("body").next().unwrap();
            assert!(body.text().contains("中文说明：配置路由器"), "{}", body.text());
        }
    }

    #[test]
    fn deeply_nested_html_is_a_resource_error() {
        let input = format!("{}text{}", "<div>".repeat(300), "</div>".repeat(300));
        assert!(matches!(
            parse(input.as_bytes(), encoding_rs::UTF_8),
            Err(ConvertError::ResourceLimit { .. })
        ));
    }
}
