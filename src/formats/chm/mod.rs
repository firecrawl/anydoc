//! Compiled HTML Help (.chm): ordered HTML topics in an ITSF container.

mod archive;
mod html;
mod lzx;

use crate::error::ConvertError;
use crate::model::{
    AnchorId, Block, Document, ImageSource, Inline, LinkTarget, List, ListItem, MarkerKind,
};
use crate::package::path::Target;
use crate::package::xml::Element;
use crate::shared::assets::{AssetSink, media_type_for};
use crate::shared::html::{HtmlCtx, Stylesheet};
use crate::shared::uri::is_absolute_uri;
use archive::Archive;
use encoding_rs::Encoding;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let archive = Archive::open(bytes)?;
    let index = Index::new(archive.entries());
    let lcid = archive.lcid();
    let archive = RefCell::new(archive);
    let system = system_info(optional_part(&archive, "#SYSTEM")?.as_deref(), lcid);
    let pages: Vec<String> = index.paths.keys().filter(|path| is_html(path)).cloned().collect();
    if pages.is_empty() {
        return Err(ConvertError::malformed("CHM contains no HTML topics"));
    }
    let toc_path = system
        .toc
        .as_deref()
        .and_then(|path| resolve_local("", path, system.encoding, system.compiled.as_deref()))
        .and_then(|target| index.canonical(&target.path).map(str::to_string))
        .or_else(|| index.paths.keys().find(|path| extension(path) == "hhc").cloned());
    let mut toc = Vec::new();
    let mut toc_encoding = system.encoding;
    let mut toc_base = toc_path.clone().unwrap_or_default();
    if let Some(path) = &toc_path
        && let Some(bytes) = optional_part(&archive, index.original(path))?
    {
        toc_encoding = html::encoding(&bytes, system.encoding);
        match html::parse(&bytes, system.encoding) {
            Ok(tree) => {
                toc_base =
                    document_base(&tree, &toc_base, toc_encoding, system.compiled.as_deref());
                toc = parse_toc(&tree);
            }
            Err(e) if e.is_fatal() => return Err(e),
            Err(e) => log::warn!("skipping unusable CHM contents {path}: {e}"),
        }
    }
    let assets = RefCell::new(AssetSink::new());
    let mut doc = Document::default();
    let mut ordered = Vec::new();
    let mut seen = HashSet::new();
    collect_topics(&toc, &toc_base, &system, toc_encoding, &index, &mut ordered, &mut seen);
    // A missing/binary TOC, or a TOC naming only part of the book, never
    // prevents conversion of the remaining HTML. BTreeMap order is stable.
    for path in pages {
        if seen.insert(path.clone()) {
            ordered.push(path);
        }
    }
    let topics: HashSet<String> = ordered.iter().cloned().collect();
    let toc_ctx = Ctx {
        archive: &archive,
        index: &index,
        assets: &assets,
        base: toc_path.as_deref().unwrap_or(""),
        reference_base: &toc_base,
        topics: &topics,
        encoding: toc_encoding,
        compiled: system.compiled.as_deref(),
    };
    if let Some(list) = render_toc(&toc, &toc_ctx) {
        doc.blocks.push(list);
    }
    let mut css_cache = HashMap::new();
    let mut converted = 0usize;
    for path in &ordered {
        let Some(bytes) = optional_part(&archive, index.original(path))? else { continue };
        let (tree, encoding) = match html::parse_with_encoding(&bytes, system.encoding) {
            Ok(parsed) => parsed,
            Err(e) if e.is_fatal() => return Err(e),
            Err(e) => {
                log::warn!("skipping unusable CHM topic {path}: {e}");
                continue;
            }
        };
        let Some(body) = tree.descendants_any("body").next() else { continue };
        let reference_base = document_base(&tree, path, encoding, system.compiled.as_deref());
        let ctx = Ctx {
            archive: &archive,
            index: &index,
            assets: &assets,
            base: path,
            reference_base: &reference_base,
            topics: &topics,
            encoding,
            compiled: system.compiled.as_deref(),
        };
        let css = stylesheet(&tree, &ctx, &mut css_cache)?;
        let blocks = crate::shared::html::to_blocks(body, &css, &ctx)?;
        doc.blocks.push(Block::Paragraph(vec![Inline::Anchor(path.clone())]));
        if !blocks.iter().any(|block| matches!(block, Block::Heading { level: 1, .. }))
            && let Some(title) = tree.descendants_any("title").next().map(Element::text)
            && !title.trim().is_empty()
        {
            doc.blocks.push(Block::heading(1, vec![Inline::plain(title.trim())]));
        }
        doc.blocks.extend(blocks);
        converted += 1;
    }
    if converted == 0 {
        return Err(ConvertError::malformed("no HTML topic in the CHM could be read"));
    }
    doc.assets = std::mem::take(&mut assets.borrow_mut().assets);
    Ok(doc)
}

fn optional_part(
    archive: &RefCell<Archive<'_>>,
    path: &str,
) -> Result<Option<Rc<[u8]>>, ConvertError> {
    match archive.borrow_mut().part(path) {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.is_fatal() => Err(e),
        Err(e) => {
            log::warn!("skipping unreadable CHM part {path}: {e}");
            Ok(None)
        }
    }
}

struct Index {
    paths: BTreeMap<String, String>,
    folded: HashMap<String, String>,
}

impl Index {
    fn new<'a>(entries: impl Iterator<Item = &'a str>) -> Self {
        let mut paths = BTreeMap::new();
        for original in entries {
            // Directory names are already decoded by the archive. Percent
            // escapes belong to hrefs, not to the stored names themselves.
            let path = normalize_path(original);
            paths.entry(path).or_insert_with(|| original.to_string());
        }
        let mut folded = HashMap::new();
        for path in paths.keys() {
            folded.entry(path.to_ascii_lowercase()).or_insert_with(|| path.clone());
        }
        Self { paths, folded }
    }

    fn canonical(&self, path: &str) -> Option<&str> {
        self.paths
            .get_key_value(path)
            .map(|(key, _)| key.as_str())
            .or_else(|| self.folded.get(&path.to_ascii_lowercase()).map(String::as_str))
    }

    fn original<'a>(&'a self, path: &'a str) -> &'a str {
        self.paths.get(path).map(String::as_str).unwrap_or(path)
    }
}

fn normalize_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

fn extension(path: &str) -> String {
    path.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default()
}

fn is_html(path: &str) -> bool {
    !matches!(path.chars().next(), Some('#' | '$' | ':'))
        && matches!(extension(path).as_str(), "htm" | "html" | "xhtml" | "shtml")
}

struct System {
    toc: Option<String>,
    compiled: Option<String>,
    encoding: &'static Encoding,
}

fn system_info(bytes: Option<&[u8]>, lcid: u32) -> System {
    let mut records = Vec::new();
    let mut lcid = lcid;
    if let Some(bytes) = bytes {
        let mut pos = 4usize; // DWORD version, then WORD code/WORD length records.
        while let Some(header) = bytes.get(pos..pos.saturating_add(4)) {
            let code = u16::from_le_bytes([header[0], header[1]]);
            let len = u16::from_le_bytes([header[2], header[3]]) as usize;
            pos += 4;
            let Some(value) = bytes.get(pos..pos.saturating_add(len)) else {
                log::warn!("ignoring a truncated CHM #SYSTEM record");
                break;
            };
            if code == 4 && value.len() >= 4 {
                lcid = u32::from_le_bytes(value[..4].try_into().unwrap());
            }
            records.push((code, value));
            pos += len;
        }
    }
    let encoding = language_encoding(lcid);
    let string = |code| {
        records.iter().find(|(kind, _)| *kind == code).and_then(|(_, value)| {
            let value = value.split(|b| *b == 0).next().unwrap_or(value);
            let text = html::decode(value, encoding);
            (!text.trim().is_empty()).then(|| text.trim().to_string())
        })
    };
    System { toc: string(0), compiled: string(6), encoding }
}

fn language_encoding(lcid: u32) -> &'static Encoding {
    // Script variants have different ANSI codepages even with the same
    // primary language. Other regional variants share their family's page.
    match lcid & 0xffff {
        0x0404 | 0x0c04 | 0x1404 => return encoding_rs::BIG5,
        0x082c | 0x0843 | 0x0c1a | 0x1c1a | 0x201a | 0x281a | 0x301a => {
            return encoding_rs::WINDOWS_1251;
        }
        _ => {}
    }
    match lcid & 0x03ff {
        0x04 => encoding_rs::GBK,
        0x11 => encoding_rs::SHIFT_JIS,
        0x12 => encoding_rs::EUC_KR,
        0x05 | 0x0e | 0x15 | 0x18 | 0x1a | 0x1b | 0x1c | 0x24 => encoding_rs::WINDOWS_1250,
        0x02 | 0x19 | 0x22 | 0x23 | 0x28 | 0x2f | 0x3f | 0x40 | 0x44 | 0x50 => {
            encoding_rs::WINDOWS_1251
        }
        0x08 => encoding_rs::WINDOWS_1253,
        0x1f | 0x2c | 0x43 => encoding_rs::WINDOWS_1254,
        0x0d => encoding_rs::WINDOWS_1255,
        0x01 | 0x20 | 0x29 | 0x63 | 0x80 => encoding_rs::WINDOWS_1256,
        0x25..=0x27 => encoding_rs::WINDOWS_1257,
        0x2a => encoding_rs::WINDOWS_1258,
        0x1e => encoding_rs::WINDOWS_874,
        _ => encoding_rs::WINDOWS_1252,
    }
}

#[derive(Debug)]
struct TocNode {
    title: String,
    href: Option<String>,
    children: Vec<TocNode>,
}

fn parse_toc(tree: &Element) -> Vec<TocNode> {
    tree.descendants_any("ul").next().map(toc_list).unwrap_or_default()
}

fn toc_list(list: &Element) -> Vec<TocNode> {
    let mut nodes: Vec<TocNode> = Vec::new();
    for child in list.child_elems() {
        if child.local == "ul" {
            let nested = toc_list(child);
            if let Some(previous) = nodes.last_mut() {
                previous.children.extend(nested);
            } else {
                nodes.extend(nested);
            }
            continue;
        }
        if child.local != "li" {
            continue;
        }
        let object = child.child_elems().find(|elem| {
            elem.local == "object"
                && elem
                    .attr_any("type")
                    .is_some_and(|kind| kind.eq_ignore_ascii_case("text/sitemap"))
        });
        let mut title = String::new();
        let mut href = None;
        if let Some(object) = object {
            for param in object.descendants_any("param") {
                let value = param.attr_any("value").unwrap_or("").trim();
                match param.attr_any("name").unwrap_or("").to_ascii_lowercase().as_str() {
                    "name" => title = value.to_string(),
                    "local" if !value.is_empty() => href = Some(value.to_string()),
                    _ => {}
                }
            }
        }
        // Some producers emit ordinary linked HTML rather than sitemap objects.
        if object.is_none()
            && let Some(link) = child.child_elems().find(|elem| elem.local == "a")
        {
            title = link.text().trim().to_string();
            href = link.attr_any("href").map(str::to_string);
        }
        let children: Vec<TocNode> =
            child.child_elems().filter(|elem| elem.local == "ul").flat_map(toc_list).collect();
        if !title.is_empty() || href.is_some() {
            nodes.push(TocNode { title, href, children });
        } else {
            nodes.extend(children);
        }
    }
    nodes
}

fn collect_topics(
    nodes: &[TocNode],
    base: &str,
    system: &System,
    encoding: &'static Encoding,
    index: &Index,
    ordered: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    for node in nodes {
        if let Some(href) = &node.href
            && let Some(target) = resolve_target(base, href, encoding, system.compiled.as_deref())
            && let Some(path) = index.canonical(&target.path).filter(|path| is_html(path))
            && seen.insert(path.to_string())
        {
            ordered.push(path.to_string());
        }
        collect_topics(&node.children, base, system, encoding, index, ordered, seen);
    }
}

fn render_toc(nodes: &[TocNode], ctx: &Ctx<'_, '_>) -> Option<Block> {
    let items: Vec<ListItem> = nodes
        .iter()
        .map(|node| {
            let label = if node.title.is_empty() {
                node.href.as_deref().unwrap_or("")
            } else {
                &node.title
            };
            let content = vec![Inline::plain(label)];
            let content = match node.href.as_deref().and_then(|href| ctx.link_target(href)) {
                Some(target) => vec![Inline::Link { content, target }],
                None => content,
            };
            let mut blocks = vec![Block::Paragraph(content)];
            if let Some(children) = render_toc(&node.children, ctx) {
                blocks.push(children);
            }
            ListItem { blocks, marker_label: None }
        })
        .collect();
    (!items.is_empty()).then_some(Block::List(List { marker: MarkerKind::Bullet, start: 1, items }))
}

fn stylesheet(
    tree: &Element,
    ctx: &Ctx<'_, '_>,
    cache: &mut HashMap<(String, &'static str), Option<String>>,
) -> Result<Stylesheet, ConvertError> {
    let mut sheet = Stylesheet::default();
    for elem in tree.descendant_elems() {
        if elem.local == "style" {
            sheet.add(&elem.text());
        } else if elem.local == "link"
            && elem
                .attr_any("rel")
                .unwrap_or("")
                .split_whitespace()
                .any(|rel| rel.eq_ignore_ascii_case("stylesheet"))
            && let Some(href) = elem.attr_any("href")
            && let Some(target) = ctx.resolve(href)
            && let Some(path) = ctx.index.canonical(&target.path)
        {
            // A stylesheet without its own charset inherits the referring
            // page's encoding, so the decoded cache must include that context.
            let key = (path.to_string(), ctx.encoding.name());
            if !cache.contains_key(&key) {
                let text = optional_part(ctx.archive, ctx.index.original(path))?
                    .map(|bytes| html::decode_stylesheet(&bytes, ctx.encoding));
                cache.insert(key.clone(), text);
            }
            if let Some(Some(text)) = cache.get(&key) {
                sheet.add(text);
            }
        }
    }
    Ok(sheet)
}

struct Ctx<'a, 'b> {
    archive: &'b RefCell<Archive<'a>>,
    index: &'b Index,
    assets: &'b RefCell<AssetSink>,
    base: &'b str,
    reference_base: &'b str,
    topics: &'b HashSet<String>,
    encoding: &'static Encoding,
    compiled: Option<&'b str>,
}

impl Ctx<'_, '_> {
    fn resolve(&self, reference: &str) -> Option<Target> {
        resolve_target(self.reference_base, reference, self.encoding, self.compiled)
    }
}

impl HtmlCtx for Ctx<'_, '_> {
    fn link_target(&self, href: &str) -> Option<LinkTarget> {
        if href.trim().is_empty() {
            return None;
        }
        if let Some(target) = self.resolve(href)
            && let Some(path) =
                self.index.canonical(&target.path).filter(|path| self.topics.contains(*path))
        {
            return Some(LinkTarget::Anchor(scoped(path, target.fragment.as_deref())));
        }
        let href = resolve_reference(self.reference_base, href);
        Some(if is_absolute_uri(&href) || href.starts_with("//") {
            LinkTarget::External(href)
        } else {
            LinkTarget::Relative(href)
        })
    }

    fn image_source(&self, src: &str) -> Result<Option<ImageSource>, ConvertError> {
        if src.trim().is_empty() {
            return Ok(None);
        }
        if let Some(target) = self.resolve(src) {
            let Some(path) = self.index.canonical(&target.path) else { return Ok(None) };
            return match optional_part(self.archive, self.index.original(path))? {
                Some(bytes) => self
                    .assets
                    .borrow_mut()
                    .add(media_type_for(path), path.to_string(), &bytes)
                    .map(|id| Some(ImageSource::Asset(id))),
                None => Ok(None),
            };
        }
        let src = resolve_reference(self.reference_base, src);
        Ok((is_absolute_uri(&src) || src.starts_with("//")).then_some(ImageSource::External(src)))
    }

    fn anchor_id(&self, raw: &str) -> AnchorId {
        scoped(self.base, Some(raw))
    }
}

fn scoped(path: &str, fragment: Option<&str>) -> AnchorId {
    match fragment.filter(|fragment| !fragment.is_empty()) {
        Some(fragment) => format!("{path}#{fragment}"),
        None => path.to_string(),
    }
}

/// The first base href controls references, while anchor definitions always
/// stay scoped to the actual topic path in Ctx::base.
fn document_base(
    tree: &Element,
    path: &str,
    encoding: &'static Encoding,
    compiled: Option<&str>,
) -> String {
    let Some(href) = tree.descendants_any("base").find_map(|elem| elem.attr_any("href")) else {
        return path.to_string();
    };
    let href = href.trim();
    if let Some(mut target) = resolve_local(path, href, encoding, compiled) {
        let raw_path = href.split(['?', '#']).next().unwrap_or(href);
        if raw_path.ends_with(['/', '\\'])
            || matches!(raw_path.rsplit('/').next(), Some("." | ".."))
        {
            target.path.push('/');
        }
        return target.path;
    }
    if (is_absolute_uri(href) || href.starts_with("//"))
        && !href.to_ascii_lowercase().starts_with("javascript:")
        && !href.to_ascii_lowercase().starts_with("data:")
    {
        return href.to_string();
    }
    path.to_string()
}

fn resolve_target(
    base: &str,
    reference: &str,
    encoding: &'static Encoding,
    compiled: Option<&str>,
) -> Option<Target> {
    let reference = resolve_reference(base, reference);
    if (is_absolute_uri(base) || base.starts_with("//"))
        && !is_absolute_uri(&reference)
        && !reference.starts_with("::")
    {
        return None;
    }
    resolve_local(base, &reference, encoding, compiled)
}

/// Join an external base without decoding its percent-escaped path. Local
/// CHM references instead retain the legacy encoding and archive checks.
fn resolve_reference(base: &str, reference: &str) -> String {
    let reference = reference.trim();
    if is_absolute_uri(reference) || reference.starts_with("::") {
        return reference.to_string();
    }
    let hierarchy =
        base.find("://").map(|pos| pos + 3).or_else(|| base.starts_with("//").then_some(2));
    let (origin, path) = if let Some(start) = hierarchy {
        if reference.starts_with("//") {
            return base.find("://").map_or_else(
                || reference.to_string(),
                |pos| format!("{}{reference}", &base[..=pos]),
            );
        }
        let end = base[start..].find(['/', '?', '#']).map_or(base.len(), |pos| start + pos);
        (&base[..end], &base[end..])
    } else if is_absolute_uri(base)
        && let Some((origin, path)) = base.split_once("::")
    {
        (&base[..origin.len() + 2], path)
    } else {
        return reference.to_string();
    };
    if reference.starts_with('#') {
        return format!("{origin}{}{reference}", path.split('#').next().unwrap_or(path));
    }
    if reference.starts_with('?') {
        return format!("{origin}{}{reference}", path.split(['?', '#']).next().unwrap_or(path));
    }
    let base_path = path.split(['?', '#']).next().unwrap_or(path);
    let split = reference.find(['?', '#']).unwrap_or(reference.len());
    let (relative_path, suffix) = reference.split_at(split);
    if relative_path.is_empty() {
        return format!("{origin}{path}");
    }
    let joined = if relative_path.starts_with('/') {
        relative_path.to_string()
    } else {
        let directory = base_path.rsplit_once('/').map_or("", |(dir, _)| dir);
        format!("{directory}/{relative_path}")
    };
    let trailing = joined.ends_with('/') || matches!(joined.rsplit('/').next(), Some("." | ".."));
    let mut normalized = format!("/{}", normalize_path(&joined));
    if trailing && !normalized.ends_with('/') {
        normalized.push('/');
    }
    format!("{origin}{normalized}{suffix}")
}

fn resolve_local(
    base: &str,
    reference: &str,
    encoding: &'static Encoding,
    compiled: Option<&str>,
) -> Option<Target> {
    let mut reference = reference.trim();
    let mut archive_root = false;
    let lower = reference.to_ascii_lowercase();
    if lower.starts_with("ms-its:")
        || lower.starts_with("its:")
        || lower.starts_with("mk:@msitstore:")
    {
        let (book, topic) = reference.split_once("::")?;
        let book = book.split_once(':')?.1.trim_start_matches('@');
        let book = book.rsplit(['/', '\\', ':']).next().unwrap_or(book);
        let compiled = compiled?;
        let stem = |name: &str| {
            let name = name.to_ascii_lowercase();
            name.strip_suffix(".chm").unwrap_or(&name).to_string()
        };
        if stem(book) != stem(compiled) {
            return None;
        }
        reference = topic.split('>').next().unwrap_or(topic);
        archive_root = true;
    } else if let Some(topic) = reference.strip_prefix("::") {
        reference = topic;
        archive_root = true;
    } else if is_absolute_uri(reference) || reference.starts_with("//") {
        return None;
    }
    let (path, fragment) = match reference.split_once('#') {
        Some((path, fragment)) => (path, Some(decode_url(fragment, encoding))),
        None => (reference, None),
    };
    let path = path.split('?').next().unwrap_or(path).replace('\\', "/");
    if path.is_empty() {
        return Some(Target { path: base.to_string(), fragment });
    }
    let mut parts: Vec<String> = if archive_root || path.starts_with('/') {
        Vec::new()
    } else {
        base.rsplit_once('/')
            .map(|(dir, _)| {
                dir.split('/').filter(|part| !part.is_empty()).map(str::to_string).collect()
            })
            .unwrap_or_default()
    };
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => {
                let decoded = decode_url(part, encoding);
                if decoded.contains(['/', '\\']) || decoded == "." || decoded == ".." {
                    return None;
                }
                parts.push(decoded);
            }
        }
    }
    Some(Target { path: parts.join("/"), fragment })
}

fn decode_url(component: &str, encoding: &'static Encoding) -> String {
    let bytes = component.as_bytes();
    let mut decoded = String::with_capacity(bytes.len());
    let mut pending = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        if bytes[pos] == b'%'
            && let Some(pair) = bytes.get(pos + 1..pos + 3)
            && pair.iter().all(u8::is_ascii_hexdigit)
        {
            pending.push(u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap());
            pos += 3;
        } else {
            let ch = component[pos..].chars().next().unwrap();
            if ch.is_ascii() {
                // A literal ASCII byte can be a legacy multibyte trail, as
                // in GBK %81A. Decode it with the adjacent escaped bytes.
                pending.push(ch as u8);
            } else {
                decoded.push_str(&encoding.decode(&pending).0);
                pending.clear();
                decoded.push(ch);
            }
            pos += ch.len_utf8();
        }
    }
    decoded.push_str(&encoding.decode(&pending).0);
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regional_lcids_use_their_legacy_windows_codepage() {
        for (lcid, expected) in [
            (0x0415, encoding_rs::WINDOWS_1250),
            (0x0408, encoding_rs::WINDOWS_1253),
            (0x041f, encoding_rs::WINDOWS_1254),
            (0x040d, encoding_rs::WINDOWS_1255),
            (0x0c01, encoding_rs::WINDOWS_1256),
            (0x0425, encoding_rs::WINDOWS_1257),
            (0x042a, encoding_rs::WINDOWS_1258),
            (0x041e, encoding_rs::WINDOWS_874),
            (0x081a, encoding_rs::WINDOWS_1250),
            (0x0c1a, encoding_rs::WINDOWS_1251),
            (0x082c, encoding_rs::WINDOWS_1251),
        ] {
            assert_eq!(system_info(None, lcid).encoding, expected, "LCID {lcid:#06x}");
        }
    }

    #[test]
    fn legacy_percent_escapes_keep_the_selected_encoding_and_literal_unicode() {
        assert_eq!(decode_url("%C2%A9", encoding_rs::GBK), "漏");
        assert_eq!(decode_url("漏%C2%A9", encoding_rs::GBK), "漏漏");
        assert_eq!(decode_url("%C2%A9", encoding_rs::UTF_8), "©");
        assert_eq!(decode_url("%81A", encoding_rs::GBK), "丄");
        assert_eq!(decode_url("乙%A4A", encoding_rs::BIG5), "乙乙");
    }

    #[test]
    fn external_bases_keep_authority_query_and_fragment_semantics() {
        for (base, reference, expected) in [
            (
                "https://example.invalid/a/page.htm?old#old",
                "../b.htm?q#f",
                "https://example.invalid/b.htm?q#f",
            ),
            (
                "https://example.invalid/a/page.htm?old#old",
                "#new",
                "https://example.invalid/a/page.htm?old#new",
            ),
            (
                "https://example.invalid/a/page.htm?old",
                "?new#f",
                "https://example.invalid/a/page.htm?new#f",
            ),
            ("https://example.invalid/a/", "//cdn.invalid/p.png", "https://cdn.invalid/p.png"),
            ("//example.invalid:8080/a/", "//cdn.invalid/p.png", "//cdn.invalid/p.png"),
            ("https://example.invalid", "p.png", "https://example.invalid/p.png"),
        ] {
            assert_eq!(resolve_reference(base, reference), expected);
        }
    }

    #[test]
    fn hhc_preserves_nested_groups_and_repeated_topic_references() {
        let tree = html::parse(
            br#"<UL><LI><OBJECT TYPE="text/sitemap"><PARAM NAME=Name VALUE=Group></OBJECT>
            <UL><LI><OBJECT TYPE="text/sitemap"><PARAM NAME=Name VALUE=Intro><PARAM NAME=Local VALUE=z.htm></OBJECT>
            <LI><OBJECT TYPE="text/sitemap"><PARAM NAME=Name VALUE=Syntax><PARAM NAME=Local VALUE=a.htm#syntax></OBJECT></UL>
            <LI><OBJECT TYPE="text/sitemap"><PARAM NAME=Name VALUE=Again><PARAM NAME=Local VALUE=z.htm></OBJECT></UL>"#,
            encoding_rs::UTF_8,
        )
        .unwrap();
        let nodes = parse_toc(&tree);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].title, "Group");
        assert!(nodes[0].href.is_none());
        assert_eq!(nodes[0].children.len(), 2);
        assert_eq!(nodes[0].children[1].href.as_deref(), Some("a.htm#syntax"));
        assert_eq!(nodes[1].href.as_deref(), Some("z.htm"));
    }

    #[test]
    fn local_references_decode_legacy_urls_and_keep_encoded_structure_rejected() {
        let target = resolve_local(
            "docs/start.htm",
            "../%C5%E4%D6%C3.htm#%B2%CE%CA%FD",
            encoding_rs::GBK,
            None,
        )
        .unwrap();
        assert_eq!(target.path, "配置.htm");
        assert_eq!(target.fragment.as_deref(), Some("参数"));
        assert_eq!(
            resolve_local("docs/start.htm", r"..\images\pic.png", encoding_rs::UTF_8, None)
                .unwrap()
                .path,
            "images/pic.png"
        );
        assert!(resolve_local("", "%2e%2e/escape.htm", encoding_rs::UTF_8, None).is_none());
        assert!(resolve_local("", "a%2fb.htm", encoding_rs::UTF_8, None).is_none());
    }

    #[test]
    fn chm_wrapper_links_are_local_only_for_the_same_book() {
        let target = resolve_local(
            "",
            "ms-its:manual.chm::/topic.htm#api>main",
            encoding_rs::UTF_8,
            Some("manual"),
        )
        .unwrap();
        assert_eq!(target.path, "topic.htm");
        assert_eq!(target.fragment.as_deref(), Some("api"));
        for reference in ["ms-its:manual.chm::topic.htm#api", "::topic.htm#api"] {
            assert_eq!(
                resolve_local("docs/start.htm", reference, encoding_rs::UTF_8, Some("manual"))
                    .unwrap()
                    .path,
                "topic.htm"
            );
        }
        assert!(
            resolve_local(
                "",
                "mk:@MSITStore:other.chm::/topic.htm",
                encoding_rs::UTF_8,
                Some("manual")
            )
            .is_none()
        );
    }
}
