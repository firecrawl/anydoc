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
    let sink = Sink::new();
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
    let mut stack = vec![(Rc::clone(&document.root), 0usize)];
    while let Some((node, depth)) = stack.pop() {
        let depth = depth + usize::from(matches!(node.kind, Kind::Element { .. }));
        if depth > limits::MAX_XML_DEPTH {
            return Err(limit_error("max_xml_depth"));
        }
        stack.extend(node.children().map(|child| (child, depth)));
    }
    Ok((
        Element {
            ns: None,
            local: String::new(),
            attrs: Vec::new(),
            children: document.root.children().filter_map(|child| adapt(&child)).collect(),
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

#[cfg(test)]
thread_local! {
    // Count traversed/moved siblings, rather than relying on CI timing.
    static SIBLING_STEPS: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
fn sibling_steps(count: usize) {
    SIBLING_STEPS.with(|steps| steps.set(steps.get() + count));
}

struct DomNode {
    kind: Kind,
    links: RefCell<Links>,
}

// The flat arena owns nodes. Weak sibling links allow constant-time insertion
// and removal, without recursively dropping a wide sibling chain at the end.
#[derive(Default)]
struct Links {
    parent: Weak<DomNode>,
    previous: Weak<DomNode>,
    next: Weak<DomNode>,
    first: Weak<DomNode>,
    last: Weak<DomNode>,
}

struct Children {
    next: Option<Handle>,
}

impl Iterator for Children {
    type Item = Handle;

    fn next(&mut self) -> Option<Handle> {
        let node = self.next.take()?;
        #[cfg(test)]
        sibling_steps(1);
        self.next = node.links.borrow().next.upgrade();
        Some(node)
    }
}

struct Dom {
    root: Handle,
    _nodes: Vec<Handle>,
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
        Rc::new(Self { kind, links: RefCell::new(Links::default()) })
    }

    fn children(&self) -> Children {
        Children { next: self.links.borrow().first.upgrade() }
    }

    fn detach(node: &Handle) {
        let mut links = node.links.borrow_mut();
        let Some(parent) = links.parent.upgrade() else { return };
        let mut parent_links = parent.links.borrow_mut();
        if let Some(previous) = links.previous.upgrade() {
            previous.links.borrow_mut().next = links.next.clone();
        } else {
            parent_links.first = links.next.clone();
        }
        if let Some(next) = links.next.upgrade() {
            next.links.borrow_mut().previous = links.previous.clone();
        } else {
            parent_links.last = links.previous.clone();
        }
        links.parent = Weak::new();
        links.previous = Weak::new();
        links.next = Weak::new();
    }
}

struct Sink {
    document: Handle,
    nodes: Cell<usize>,
    arena: RefCell<Vec<Handle>>,
    failure: Rc<Cell<Option<&'static str>>>,
}

impl Sink {
    fn new() -> Self {
        let document = DomNode::new(Kind::Document);
        Self {
            arena: RefCell::new(vec![Rc::clone(&document)]),
            document,
            nodes: Cell::new(1),
            failure: Rc::new(Cell::new(None)),
        }
    }

    fn node(&self, kind: Kind) -> Handle {
        self.nodes.set(self.nodes.get().saturating_add(1));
        if self.nodes.get() > limits::MAX_XML_NODES {
            self.failure.set(Some("max_xml_nodes"));
        }
        let node = DomNode::new(kind);
        if !matches!(node.kind, Kind::Ignored) {
            self.arena.borrow_mut().push(Rc::clone(&node));
        }
        node
    }

    fn insert(&self, parent: &Handle, child: NodeOrText<Handle>, before: Option<&Handle>) {
        if self.failure.get().is_some() {
            return;
        }
        let before = before.filter(|sibling| {
            sibling.links.borrow().parent.upgrade().is_some_and(|p| Rc::ptr_eq(&p, parent))
        });
        let child = match child {
            NodeOrText::AppendNode(node) => {
                if before.is_some_and(|sibling| Rc::ptr_eq(sibling, &node)) {
                    return;
                }
                node
            }
            NodeOrText::AppendText(text) => {
                let previous = match before {
                    Some(sibling) => sibling.links.borrow().previous.upgrade(),
                    None => parent.links.borrow().last.upgrade(),
                };
                if let Some(previous) = previous
                    && let Kind::Text(contents) = &previous.kind
                {
                    contents.borrow_mut().push_tendril(&text);
                    return;
                }
                self.node(Kind::Text(RefCell::new(text)))
            }
        };
        if matches!(child.kind, Kind::Ignored) {
            return;
        }
        let mut cursor = Some(Rc::clone(parent));
        let mut depth = usize::from(matches!(child.kind, Kind::Element { .. }));
        while let Some(node) = cursor {
            if Rc::ptr_eq(&node, &child) {
                self.failure.set(Some("max_xml_depth"));
                return;
            }
            depth += usize::from(matches!(node.kind, Kind::Element { .. }));
            if depth > limits::MAX_XML_DEPTH {
                self.failure.set(Some("max_xml_depth"));
                return;
            }
            cursor = node.links.borrow().parent.upgrade();
        }
        DomNode::detach(&child);
        let previous = match before {
            Some(sibling) => sibling.links.borrow().previous.clone(),
            None => parent.links.borrow().last.clone(),
        };
        let child_weak = Rc::downgrade(&child);
        if let Some(previous) = previous.upgrade() {
            previous.links.borrow_mut().next = child_weak.clone();
        } else {
            parent.links.borrow_mut().first = child_weak.clone();
        }
        if let Some(next) = before {
            next.links.borrow_mut().previous = child_weak.clone();
        } else {
            parent.links.borrow_mut().last = child_weak;
        }
        let mut links = child.links.borrow_mut();
        links.parent = Rc::downgrade(parent);
        links.previous = previous;
        links.next = before.map_or_else(Weak::new, Rc::downgrade);
    }
}

impl TreeSink for Sink {
    type Handle = Handle;
    type Output = Dom;
    type ElemName<'a> = &'a QualName;

    fn finish(self) -> Dom {
        Dom { root: self.document, _nodes: self.arena.into_inner() }
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
        let parent = sibling.links.borrow().parent.upgrade();
        if let Some(parent) = parent {
            self.insert(&parent, child, Some(sibling));
        }
    }
    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        previous: &Handle,
        child: NodeOrText<Handle>,
    ) {
        if element.links.borrow().parent.upgrade().is_some() {
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
        if Rc::ptr_eq(node, parent) {
            return;
        }
        loop {
            let Some(child) = node.links.borrow().first.upgrade() else { break };
            self.append(parent, NodeOrText::AppendNode(child));
            if self.failure.get().is_some() {
                break;
            }
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
            children: node.children().filter_map(|child| adapt(&child)).collect(),
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

    fn test_sink() -> Sink {
        Sink::new()
    }

    fn test_element(sink: &Sink, local: &str) -> Handle {
        sink.create_element(
            QualName::new(None, Default::default(), local.into()),
            Vec::new(),
            ElementFlags::default(),
        )
    }

    fn assert_children(parent: &Handle, expected: &[&Handle]) {
        let children: Vec<_> = parent.children().collect();
        assert_eq!(children.len(), expected.len());
        for (index, (child, expected)) in children.iter().zip(expected).enumerate() {
            assert!(Rc::ptr_eq(child, expected));
            let links = child.links.borrow();
            assert!(links.parent.upgrade().is_some_and(|p| Rc::ptr_eq(&p, parent)));
            let previous = links.previous.upgrade();
            assert_eq!(previous.is_some(), index > 0);
            if let Some(previous) = previous {
                assert!(Rc::ptr_eq(&previous, &children[index - 1]));
            }
            let next = links.next.upgrade();
            assert_eq!(next.is_some(), index + 1 < children.len());
            if let Some(next) = next {
                assert!(Rc::ptr_eq(&next, &children[index + 1]));
            }
        }
        let links = parent.links.borrow();
        for (actual, expected) in
            [(links.first.upgrade(), children.first()), (links.last.upgrade(), children.last())]
        {
            assert_eq!(actual.is_some(), expected.is_some());
            if let (Some(actual), Some(expected)) = (actual, expected) {
                assert!(Rc::ptr_eq(&actual, expected));
            }
        }
    }

    #[test]
    fn sibling_moves_and_removals_preserve_both_directions() {
        let sink = test_sink();
        let parent = sink.get_document();
        let nodes: Vec<_> =
            ["a", "b", "c", "d"].iter().map(|local| test_element(&sink, local)).collect();
        for child in &nodes {
            sink.append(&parent, NodeOrText::AppendNode(Rc::clone(child)));
        }
        sink.append_before_sibling(&nodes[1], NodeOrText::AppendNode(Rc::clone(&nodes[3])));
        assert_children(&parent, &[&nodes[0], &nodes[3], &nodes[1], &nodes[2]]);
        sink.append_before_sibling(&nodes[2], NodeOrText::AppendNode(Rc::clone(&nodes[0])));
        assert_children(&parent, &[&nodes[3], &nodes[1], &nodes[0], &nodes[2]]);
        sink.append_before_sibling(&nodes[0], NodeOrText::AppendNode(Rc::clone(&nodes[0])));
        sink.append(&parent, NodeOrText::AppendNode(Rc::clone(&nodes[3])));
        assert_children(&parent, &[&nodes[1], &nodes[0], &nodes[2], &nodes[3]]);
        for (remove, expected) in [
            (1, vec![&nodes[0], &nodes[2], &nodes[3]]),
            (2, vec![&nodes[0], &nodes[3]]),
            (3, vec![&nodes[0]]),
            (0, vec![]),
        ] {
            sink.remove_from_parent(&nodes[remove]);
            sink.remove_from_parent(&nodes[remove]);
            assert_children(&parent, &expected);
        }
    }

    #[test]
    fn inserted_text_merges_only_with_its_previous_text_sibling() {
        let sink = test_sink();
        let parent = sink.get_document();
        let table = test_element(&sink, "table");
        sink.append(&parent, NodeOrText::AppendNode(Rc::clone(&table)));
        for text in ["one", "two"] {
            sink.append_before_sibling(&table, NodeOrText::AppendText(text.into()));
        }
        for text in ["after", "+more"] {
            sink.append(&parent, NodeOrText::AppendText(text.into()));
        }
        let children: Vec<_> = parent.children().collect();
        assert_children(&parent, &[&children[0], &table, &children[2]]);
        let text = |node: &Handle| match &node.kind {
            Kind::Text(text) => text.borrow().to_string(),
            _ => panic!("expected text"),
        };
        assert_eq!(text(&children[0]), "onetwo");
        assert_eq!(text(&children[2]), "after+more");
        let divider = test_element(&sink, "div");
        sink.append_before_sibling(&table, NodeOrText::AppendNode(Rc::clone(&divider)));
        sink.append_before_sibling(&table, NodeOrText::AppendText("three".into()));
        let children: Vec<_> = parent.children().collect();
        assert_children(&parent, &[&children[0], &divider, &children[2], &table, &children[4]]);
        assert_eq!(text(&children[2]), "three");
    }

    #[test]
    fn reparenting_keeps_order_and_template_contents_alive_until_finish() {
        let sink = test_sink();
        let source = test_element(&sink, "div");
        let target = test_element(&sink, "section");
        let nodes: Vec<_> =
            ["a", "b", "c"].iter().map(|local| test_element(&sink, local)).collect();
        sink.append(&target, NodeOrText::AppendNode(Rc::clone(&nodes[0])));
        for child in &nodes[1..] {
            sink.append(&source, NodeOrText::AppendNode(Rc::clone(child)));
        }
        sink.reparent_children(&source, &target);
        sink.reparent_children(&target, &target);
        assert_children(&source, &[]);
        assert_children(&target, &[&nodes[0], &nodes[1], &nodes[2]]);

        let mut flags = ElementFlags::default();
        flags.template = true;
        let template = sink.create_element(
            QualName::new(None, Default::default(), "template".into()),
            Vec::new(),
            flags,
        );
        let contents = sink.get_template_contents(&template);
        sink.append(&contents, NodeOrText::AppendText("inside".into()));
        let text = Rc::downgrade(&contents.children().next().unwrap());
        let document = sink.finish();
        assert!(text.upgrade().is_some());
        drop(document);
        assert!(text.upgrade().is_none());
    }

    #[test]
    fn malformed_tables_and_formatting_keep_html_repair_order() {
        let tree = parse(
            b"<table>one<div>two</div>three<tr><td>cell</table><p><b>four<i>five</b>six</i>seven",
            encoding_rs::UTF_8,
        )
        .unwrap();
        let body = tree.descendants_any("body").next().unwrap();
        assert_eq!(body.text(), "onetwothreecellfourfivesixseven");
        let paragraph = body.descendants_any("p").next().unwrap();
        let bold = paragraph
            .children
            .iter()
            .find_map(|node| match node {
                Node::Elem(element) if element.local == "b" => Some(element),
                _ => None,
            })
            .unwrap();
        assert_eq!(bold.text(), "fourfive");
        let italic: Vec<_> = paragraph.descendants_any("i").map(Element::text).collect();
        assert_eq!(italic, ["five", "six"]);
    }

    #[test]
    fn ignored_comments_do_not_break_sibling_links_or_text_merging() {
        let tree = parse(
            b"<p>before<!-- ignored -->after<span>middle</span><!-- ignored -->last",
            encoding_rs::UTF_8,
        )
        .unwrap();
        let paragraph = tree.descendants_any("p").next().unwrap();
        assert_eq!(paragraph.text(), "beforeaftermiddlelast");
        assert_eq!(paragraph.children.len(), 3);
        assert!(matches!(&paragraph.children[0], Node::Text(text) if text == "beforeafter"));
    }

    #[test]
    fn dropping_a_wide_dom_releases_its_nodes_without_a_recursive_sibling_chain() {
        let sink = test_sink();
        let parent = sink.get_document();
        let mut last = Weak::new();
        for _ in 0..32768 {
            let child = test_element(&sink, "div");
            last = Rc::downgrade(&child);
            sink.append(&parent, NodeOrText::AppendNode(child));
        }
        let document = sink.finish();
        assert!(last.upgrade().is_some());
        drop(document);
        assert!(last.upgrade().is_none());
    }

    #[test]
    fn inserting_and_detaching_wide_siblings_does_not_walk_the_parent() {
        let sink = test_sink();
        let parent = sink.get_document();
        let children: Vec<_> = (0..4096).map(|_| sink.node(Kind::Document)).collect();
        for child in &children {
            sink.append(&parent, NodeOrText::AppendNode(Rc::clone(child)));
        }
        SIBLING_STEPS.with(|steps| steps.set(0));
        for _ in 0..1024 {
            let child = sink.node(Kind::Document);
            sink.append_before_sibling(&children[2048], NodeOrText::AppendNode(Rc::clone(&child)));
            sink.remove_from_parent(&child);
        }
        // Foster parenting and adoption repair must not repeatedly scan or
        // shift all the siblings preceding/following their insertion point.
        let steps = SIBLING_STEPS.with(Cell::get);
        assert!(steps <= 1024 * 4, "wide sibling mutation took {steps} sibling steps");
        assert!(sink.failure.get().is_none());
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
