//! Namespace-aware DOM built on quick-xml.
//!
//! Parses from bytes so BOMs and encoding declarations are honored. Element
//! and attribute names carry their resolved namespace URI (interned). Depth
//! and node caps are enforced during parsing; recoverable malformations
//! (unclosed or mismatched tags) are repaired deliberately and logged, never
//! silently.

use crate::error::ConvertError;
use crate::package::limits;
use quick_xml::NsReader;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{QName, ResolveResult};
use std::collections::HashMap;
use std::rc::Rc;

/// Well-known namespace URIs.
pub mod ns {
    pub const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
    pub const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    pub const A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
    pub const PIC: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
    pub const WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
    pub const MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
    pub const CHART: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
    pub const DGM: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";
    pub const P: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
    pub const PKG_RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

    pub const OFFICE: &str = "urn:oasis:names:tc:opendocument:xmlns:office:1.0";
    pub const TEXT: &str = "urn:oasis:names:tc:opendocument:xmlns:text:1.0";
    pub const TABLE: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";
    pub const DRAW: &str = "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
    pub const STYLE: &str = "urn:oasis:names:tc:opendocument:xmlns:style:1.0";
    pub const FO: &str = "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0";
    pub const PRESENTATION: &str = "urn:oasis:names:tc:opendocument:xmlns:presentation:1.0";
    pub const MANIFEST: &str = "urn:oasis:names:tc:opendocument:xmlns:manifest:1.0";
    pub const XLINK: &str = "http://www.w3.org/1999/xlink";
    pub const XML: &str = "http://www.w3.org/XML/1998/namespace";
    pub const SVG_COMPAT: &str = "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0";
    pub const XHTML: &str = "http://www.w3.org/1999/xhtml";

    pub const VML: &str = "urn:schemas-microsoft-com:vml";
    pub const O_VML: &str = "urn:schemas-microsoft-com:office:office";
    pub const WPS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";
    pub const WPG: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup";
}

#[derive(Debug)]
pub enum Node {
    Elem(Element),
    Text(String),
}

#[derive(Debug)]
pub struct Element {
    /// Resolved namespace URI (interned); `None` for unqualified names.
    pub ns: Option<Rc<str>>,
    pub local: String,
    pub attrs: Vec<Attr>,
    pub children: Vec<Node>,
}

#[derive(Debug)]
pub struct Attr {
    pub ns: Option<Rc<str>>,
    pub local: String,
    pub value: String,
}

impl Element {
    pub fn is(&self, ns: &str, local: &str) -> bool {
        self.local == local && self.ns.as_deref() == Some(ns)
    }

    /// Same-vocabulary attribute lookup: the qualified attribute wins, and
    /// an explicitly unqualified one with the same local name is accepted
    /// as a deliberate leniency - schemas often leave their own attributes
    /// unqualified (DrawingML `gridSpan`) and producers vary. Lookups in a
    /// *different* vocabulary than the element's (`r:id`, `xml:id`) must use
    /// [`Element::attr_qualified`], where that fallback would misattribute.
    pub fn attr(&self, ns: &str, local: &str) -> Option<&str> {
        self.attr_qualified(ns, local).or_else(|| self.attr_unqualified(local))
    }

    /// Strictly namespace-qualified attribute lookup, no fallback.
    pub fn attr_qualified(&self, ns: &str, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|a| a.local == local && matches!(&a.ns, Some(n) if **n == *ns))
            .map(|a| a.value.as_str())
    }

    /// Explicitly unqualified attribute lookup (no namespace), no fallback.
    pub fn attr_unqualified(&self, local: &str) -> Option<&str> {
        self.attrs.iter().find(|a| a.local == local && a.ns.is_none()).map(|a| a.value.as_str())
    }

    /// Attribute by local name regardless of namespace. For elements whose
    /// vocabulary is unambiguous in context (relationships, container files).
    pub fn attr_any(&self, local: &str) -> Option<&str> {
        self.attrs.iter().find(|a| a.local == local).map(|a| a.value.as_str())
    }

    pub fn child_elems(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Elem(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    pub fn find(&self, ns: &str, local: &str) -> Option<&Element> {
        self.child_elems().find(|e| e.is(ns, local))
    }

    pub fn find_all<'a>(
        &'a self,
        ns: &'a str,
        local: &'a str,
    ) -> impl Iterator<Item = &'a Element> {
        self.child_elems().filter(move |e| e.is(ns, local))
    }

    /// All descendant nodes (elements and text), depth-first in document
    /// order, iteratively (deep DOMs must not overflow the call stack).
    pub fn descendant_nodes(&self) -> DescendantNodes<'_> {
        let mut stack: Vec<&Node> = self.children.iter().collect();
        stack.reverse();
        DescendantNodes { stack }
    }

    /// All descendant elements, depth-first in document order, lazily.
    pub fn descendant_elems(&self) -> impl Iterator<Item = &Element> {
        self.descendant_nodes().filter_map(|node| match node {
            Node::Elem(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    /// First descendant with the given name, depth-first.
    pub fn first_descendant(&self, ns: &str, local: &str) -> Option<&Element> {
        self.descendant_elems().find(|e| e.is(ns, local))
    }

    /// All descendants with the given name, in document order, lazily.
    pub fn descendants<'a>(
        &'a self,
        ns: &'a str,
        local: &'a str,
    ) -> impl Iterator<Item = &'a Element> {
        self.descendant_elems().filter(move |e| e.is(ns, local))
    }

    /// All descendants matching a local name regardless of namespace, in
    /// document order. For vocabularies where the namespace varies across
    /// producers (EPUB container/OPF metadata).
    pub fn descendants_any<'a>(&'a self, local: &'a str) -> impl Iterator<Item = &'a Element> {
        self.descendant_elems().filter(move |e| e.local == local)
    }

    /// Concatenated text of all descendant text nodes.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for node in self.descendant_nodes() {
            if let Node::Text(t) = node {
                out.push_str(t);
            }
        }
        out
    }
}

/// Iterative depth-first traversal over a subtree's nodes; the shared core
/// of every descendant lookup.
pub struct DescendantNodes<'a> {
    stack: Vec<&'a Node>,
}

impl<'a> Iterator for DescendantNodes<'a> {
    type Item = &'a Node;

    fn next(&mut self) -> Option<&'a Node> {
        let node = self.stack.pop()?;
        if let Node::Elem(e) = node {
            let start = self.stack.len();
            self.stack.extend(e.children.iter());
            self.stack[start..].reverse();
        }
        Some(node)
    }
}

/// Parse an XML part into a synthetic root element containing the top-level
/// nodes. Encoding comes from the BOM or XML declaration; the part is
/// transcoded to UTF-8 before parsing so namespace resolution sees one
/// consistent encoding.
pub fn parse_xml(bytes: &[u8]) -> Result<Element, ConvertError> {
    let utf8 = to_utf8(bytes);
    let mut reader = NsReader::from_reader(utf8.as_ref());
    reader.config_mut().check_end_names = false;

    let mut interner: HashMap<Vec<u8>, Rc<str>> = HashMap::new();
    let mut root =
        Element { ns: None, local: String::new(), attrs: Vec::new(), children: Vec::new() };
    let mut stack: Vec<Element> = Vec::new();
    let mut nodes: usize = 0;
    let mut recovered = false;

    loop {
        let event = reader
            .read_event()
            .map_err(|e| ConvertError::malformed(format!("unparseable xml: {e}")))?;
        match event {
            Event::Start(e) => {
                if stack.len() >= limits::MAX_XML_DEPTH {
                    return Err(ConvertError::ResourceLimit {
                        limit: "max_xml_depth",
                        detail: format!("element nesting exceeds {}", limits::MAX_XML_DEPTH),
                    });
                }
                bump_nodes(&mut nodes)?;
                let elem = start_to_element(&e, &reader, &mut interner);
                if is_html_void_element(&elem) {
                    attach(&mut stack, &mut root, Node::Elem(elem));
                } else {
                    stack.push(elem);
                }
            }
            Event::Empty(e) => {
                bump_nodes(&mut nodes)?;
                let elem = start_to_element(&e, &reader, &mut interner);
                attach(&mut stack, &mut root, Node::Elem(elem));
            }
            Event::End(end) => match stack.pop() {
                Some(elem) => {
                    if elem.local.as_bytes() != end.local_name().as_ref() {
                        recovered = true;
                    }
                    attach(&mut stack, &mut root, Node::Elem(elem));
                }
                None => recovered = true,
            },
            Event::Text(e) => {
                let text = match e.decode() {
                    Ok(t) => t.into_owned(),
                    Err(_) => String::from_utf8_lossy(e.as_ref()).into_owned(),
                };
                if !text.is_empty() {
                    bump_nodes(&mut nodes)?;
                    push_text(&mut stack, &mut root, text);
                }
            }
            Event::GeneralRef(e) => {
                let name = String::from_utf8_lossy(e.as_ref()).into_owned();
                let resolved = resolve_entity(&name).unwrap_or_else(|| format!("&{name};"));
                bump_nodes(&mut nodes)?;
                push_text(&mut stack, &mut root, resolved);
            }
            Event::CData(e) => {
                let text = String::from_utf8_lossy(e.as_ref()).into_owned();
                bump_nodes(&mut nodes)?;
                push_text(&mut stack, &mut root, text);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        recovered = true;
        while let Some(elem) = stack.pop() {
            attach(&mut stack, &mut root, Node::Elem(elem));
        }
    }
    if recovered {
        log::warn!("recovered malformed xml (unclosed or mismatched elements)");
    }
    Ok(root)
}

/// HTML permits these elements to omit an end tag. Treating them as empty
/// while parsing the shared XML-ish DOM keeps a standalone HTML document from
/// swallowing its following siblings (for example, `<meta>` capturing the
/// `<body>`). Namespaced XHTML and unqualified HTML are both accepted; other
/// vocabularies still follow the normal XML recovery rules.
fn is_html_void_element(elem: &Element) -> bool {
    matches!(elem.ns.as_deref(), None | Some(ns::XHTML))
        && [
            "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img",
            "input", "keygen", "link", "meta", "param", "source", "track", "wbr",
        ]
        .iter()
        .any(|name| elem.local.eq_ignore_ascii_case(name))
}

/// Transcode an XML part to UTF-8 based on its BOM or encoding declaration.
fn to_utf8(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    use std::borrow::Cow;
    match bytes {
        [0xFF, 0xFE, ..] => {
            let (s, _, _) = encoding_rs::UTF_16LE.decode(bytes);
            Cow::Owned(s.into_owned().into_bytes())
        }
        [0xFE, 0xFF, ..] => {
            let (s, _, _) = encoding_rs::UTF_16BE.decode(bytes);
            Cow::Owned(s.into_owned().into_bytes())
        }
        [0xEF, 0xBB, 0xBF, rest @ ..] => Cow::Borrowed(rest),
        _ => {
            // Sniff a non-UTF-8 encoding declaration in the XML prolog.
            let head = &bytes[..bytes.len().min(200)];
            if let Some(label) = declared_encoding(head)
                && let Some(enc) = encoding_rs::Encoding::for_label(label.as_bytes())
                && enc != encoding_rs::UTF_8
            {
                let (s, _, _) = enc.decode(bytes);
                return Cow::Owned(s.into_owned().into_bytes());
            }
            Cow::Borrowed(bytes)
        }
    }
}

fn declared_encoding(head: &[u8]) -> Option<String> {
    let head = std::str::from_utf8(head).ok()?;
    let idx = head.find("encoding")?;
    let rest = &head[idx + "encoding".len()..];
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let rest = &rest[1..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn bump_nodes(nodes: &mut usize) -> Result<(), ConvertError> {
    *nodes += 1;
    if *nodes > limits::MAX_XML_NODES {
        return Err(ConvertError::ResourceLimit {
            limit: "max_xml_nodes",
            detail: format!("part exceeds {} xml nodes", limits::MAX_XML_NODES),
        });
    }
    Ok(())
}

fn attach(stack: &mut [Element], root: &mut Element, node: Node) {
    match stack.last_mut() {
        Some(parent) => parent.children.push(node),
        None => root.children.push(node),
    }
}

/// ISO/IEC 29500 *Strict* namespace and relationship-type URIs are the
/// Transitional ones re-rooted under `http://purl.oclc.org/ooxml/` with the
/// `2006` version segment dropped. Normalizing to the Transitional family at
/// parse time lets the rest of the crate match a single set of constants.
pub fn normalize_ooxml_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("http://purl.oclc.org/ooxml/")?;
    let (family, tail) = rest.split_once('/')?;
    Some(format!("http://schemas.openxmlformats.org/{family}/2006/{tail}"))
}

fn intern(interner: &mut HashMap<Vec<u8>, Rc<str>>, uri: &[u8]) -> Rc<str> {
    if let Some(rc) = interner.get(uri) {
        return rc.clone();
    }
    let raw = String::from_utf8_lossy(uri).into_owned();
    let rc: Rc<str> = Rc::from(normalize_ooxml_uri(&raw).unwrap_or(raw));
    interner.insert(uri.to_vec(), rc.clone());
    rc
}

fn start_to_element(
    e: &BytesStart,
    reader: &NsReader<&[u8]>,
    interner: &mut HashMap<Vec<u8>, Rc<str>>,
) -> Element {
    let (res, local) = reader.resolver().resolve_element(e.name());
    let ns = match res {
        ResolveResult::Bound(namespace) => Some(intern(interner, namespace.as_ref())),
        _ => None,
    };
    let local = String::from_utf8_lossy(local.as_ref()).into_owned();
    let mut attrs = Vec::new();
    for attr in e.attributes() {
        let attr = match attr {
            Ok(a) => a,
            Err(e) => {
                // Malformed attributes degrade to absent, but visibly.
                log::debug!("dropping undecodable attribute: {e}");
                continue;
            }
        };
        // Namespace declarations are consumed by the reader's resolver; the
        // DOM stores only resolved names.
        if attr.key.as_ref().strip_prefix(b"xmlns").is_some_and(|r| matches!(r, [] | [b':', ..])) {
            continue;
        }
        let (res, alocal) = reader.resolver().resolve_attribute(attr.key);
        let ans = match res {
            ResolveResult::Bound(namespace) => Some(intern(interner, namespace.as_ref())),
            _ => None,
        };
        let value = attr
            .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder())
            .map(|v| v.into_owned())
            .unwrap_or_else(|_| String::from_utf8_lossy(&attr.value).into_owned());
        attrs.push(Attr {
            ns: ans,
            local: String::from_utf8_lossy(alocal.as_ref()).into_owned(),
            value,
        });
    }
    // `mc:Choice/@Requires` holds namespace *prefixes* evaluated in the
    // element's lexical scope (prefixes may be rebound locally). Resolve
    // them to URIs here, where the scope is known, so consumers never need
    // a document-wide prefix map. Unresolvable prefixes stay literal and
    // match no requirement.
    if local == "Choice"
        && ns.as_deref() == Some(ns::MC)
        && let Some(requires) = attrs.iter_mut().find(|a| a.local == "Requires")
    {
        requires.value = requires
            .value
            .split_whitespace()
            .map(|prefix| {
                let probe = format!("{prefix}:x");
                match reader.resolver().resolve_element(QName(probe.as_bytes())).0 {
                    ResolveResult::Bound(namespace) => {
                        let raw = String::from_utf8_lossy(namespace.as_ref()).into_owned();
                        normalize_ooxml_uri(&raw).unwrap_or(raw)
                    }
                    _ => prefix.to_string(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
    }
    Element { ns, local, attrs, children: Vec::new() }
}

fn push_text(stack: &mut [Element], root: &mut Element, text: String) {
    let target = match stack.last_mut() {
        Some(parent) => &mut parent.children,
        None => &mut root.children,
    };
    if let Some(Node::Text(prev)) = target.last_mut() {
        prev.push_str(&text);
    } else {
        target.push(Node::Text(text));
    }
}

/// Resolve entity references quick-xml surfaces as GeneralRef events.
fn resolve_entity(name: &str) -> Option<String> {
    if let Some(num) = name.strip_prefix('#') {
        let code = if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            num.parse().ok()?
        };
        return char::from_u32(code).map(String::from);
    }
    let ch = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "apos" => '\'',
        "quot" => '"',
        "nbsp" => '\u{a0}',
        "shy" => '\u{ad}',
        "mdash" => '\u{2014}',
        "ndash" => '\u{2013}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "ldquo" => '\u{201c}',
        "rdquo" => '\u{201d}',
        "hellip" => '\u{2026}',
        "copy" => '\u{a9}',
        "reg" => '\u{ae}',
        "trade" => '\u{2122}',
        "deg" => '\u{b0}',
        "middot" => '\u{b7}',
        "bull" => '\u{2022}',
        "sect" => '\u{a7}',
        "para" => '\u{b6}',
        "laquo" => '\u{ab}',
        "raquo" => '\u{bb}',
        "times" => '\u{d7}',
        "divide" => '\u{f7}',
        "plusmn" => '\u{b1}',
        "frac12" => '\u{bd}',
        "frac14" => '\u{bc}',
        "eacute" => '\u{e9}',
        "egrave" => '\u{e8}',
        "agrave" => '\u{e0}',
        "ccedil" => '\u{e7}',
        "uuml" => '\u{fc}',
        "ouml" => '\u{f6}',
        "auml" => '\u{e4}',
        "szlig" => '\u{df}',
        "aring" => '\u{e5}',
        "oslash" => '\u{f8}',
        "aelig" => '\u{e6}',
        "euro" => '\u{20ac}',
        "pound" => '\u{a3}',
        "yen" => '\u{a5}',
        "cent" => '\u{a2}',
        _ => return None,
    };
    Some(ch.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_namespaces_regardless_of_prefix() {
        let xml =
            br#"<x:root xmlns:x="urn:a" xmlns:y="urn:b"><y:kid x:id="1" plain="p"/></x:root>"#;
        let root = parse_xml(xml).unwrap();
        let r = root.find("urn:a", "root").unwrap();
        let kid = r.find("urn:b", "kid").unwrap();
        assert_eq!(kid.attr("urn:a", "id"), Some("1"));
        assert_eq!(kid.attr("urn:whatever", "plain"), Some("p"));
        assert!(r.find("urn:b", "root").is_none());
    }

    #[test]
    fn html_void_elements_do_not_capture_following_siblings() {
        let root = parse_xml(
            br#"<!doctype html><html><head><meta charset="utf-8"><style>p { display: none; }</style></head><body><p>visible sibling</p><img src="x.png"><p>second</p></body></html>"#,
        )
        .unwrap();
        let html = root.child_elems().next().unwrap();
        let head = html.child_elems().find(|e| e.local == "head").unwrap();
        assert_eq!(head.child_elems().filter(|e| e.local == "meta").count(), 1);
        assert!(!head.child_elems().any(|e| e.local == "body"));
        let body = html.child_elems().find(|e| e.local == "body").unwrap();
        assert_eq!(body.child_elems().filter(|e| e.local == "p").count(), 2);
    }

    #[test]
    fn qualified_and_unqualified_lookups_are_separate() {
        let xml =
            br#"<x:root xmlns:x="urn:a" xmlns:r="urn:r"><x:kid r:id="rel" id="plain"/></x:root>"#;
        let root = parse_xml(xml).unwrap();
        let kid = root.first_descendant("urn:a", "kid").unwrap();
        assert_eq!(kid.attr_qualified("urn:r", "id"), Some("rel"));
        assert_eq!(kid.attr_unqualified("id"), Some("plain"));
        // The lenient lookup prefers the qualified match, while a strict
        // cross-namespace lookup never picks up the unqualified one.
        assert_eq!(kid.attr("urn:r", "id"), Some("rel"));
        assert_eq!(kid.attr_qualified("urn:other", "id"), None);
    }

    #[test]
    fn strict_ooxml_namespaces_normalize_to_transitional() {
        assert_eq!(
            normalize_ooxml_uri("http://purl.oclc.org/ooxml/wordprocessingml/main").as_deref(),
            Some(ns::W)
        );
        assert_eq!(
            normalize_ooxml_uri("http://purl.oclc.org/ooxml/officeDocument/relationships")
                .as_deref(),
            Some(ns::R)
        );
        assert_eq!(
            normalize_ooxml_uri(
                "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument"
            )
            .as_deref(),
            Some(
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"
            )
        );
        assert_eq!(normalize_ooxml_uri(ns::W), None);
        assert_eq!(normalize_ooxml_uri("urn:schemas-microsoft-com:vml"), None);

        let xml = br#"<w:document xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main"><w:body/></w:document>"#;
        let root = parse_xml(xml).unwrap();
        let doc = root.find(ns::W, "document").unwrap();
        assert!(doc.find(ns::W, "body").is_some());
    }

    #[test]
    fn default_namespace_applies_to_elements_not_attributes() {
        let xml = br#"<root xmlns="urn:d"><kid id="1"/></root>"#;
        let root = parse_xml(xml).unwrap();
        let r = root.find("urn:d", "root").unwrap();
        let kid = r.find("urn:d", "kid").unwrap();
        assert_eq!(kid.attr_any("id"), Some("1"));
    }

    #[test]
    fn requires_prefixes_resolve_in_their_lexical_scope() {
        // M4: the same prefix rebound locally must resolve per element, not
        // through a document-wide first-declaration-wins map.
        let xml = br#"<mc:AlternateContent
            xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">
            <mc:Choice xmlns:z="urn:one" Requires="z"/>
            <mc:Choice xmlns:z="urn:two" Requires="z unknownprefix"/>
        </mc:AlternateContent>"#;
        let root = parse_xml(xml).unwrap();
        let alt = root.find(ns::MC, "AlternateContent").unwrap();
        let choices: Vec<&Element> = alt.find_all(ns::MC, "Choice").collect();
        assert_eq!(choices[0].attr_any("Requires"), Some("urn:one"));
        assert_eq!(choices[1].attr_any("Requires"), Some("urn:two unknownprefix"));
    }

    #[test]
    fn utf16_with_bom_decodes() {
        let text = "<r a=\"caf\u{e9}\">\u{4f60}\u{597d}</r>";
        let mut bytes = vec![0xFF, 0xFE];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        let root = parse_xml(&bytes).unwrap();
        let r = &root.child_elems().next().unwrap();
        assert_eq!(r.attr_any("a"), Some("caf\u{e9}"));
        assert_eq!(r.text(), "\u{4f60}\u{597d}");
    }

    #[test]
    fn unclosed_elements_recovered() {
        let root = parse_xml(b"<a><b>text").unwrap();
        let a = root.child_elems().next().unwrap();
        assert_eq!(a.local, "a");
        assert_eq!(a.text(), "text");
    }

    #[test]
    fn depth_limit_is_hard_error() {
        let mut xml = Vec::new();
        for _ in 0..(limits::MAX_XML_DEPTH + 2) {
            xml.extend_from_slice(b"<d>");
        }
        let err = parse_xml(&xml).unwrap_err();
        assert!(matches!(err, ConvertError::ResourceLimit { limit: "max_xml_depth", .. }));
    }

    #[test]
    fn deep_dom_traversal_is_iterative() {
        // Just under the cap: descendants/text must not overflow the stack.
        let depth = limits::MAX_XML_DEPTH - 1;
        let mut xml = Vec::new();
        for _ in 0..depth {
            xml.extend_from_slice(b"<d>");
        }
        xml.extend_from_slice(b"leaf");
        for _ in 0..depth {
            xml.extend_from_slice(b"</d>");
        }
        let root = parse_xml(&xml).unwrap();
        assert_eq!(root.text(), "leaf");
        assert_eq!(root.descendants("", "never").count(), 0);
    }
}
