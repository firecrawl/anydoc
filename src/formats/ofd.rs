//! OFD fixed-page text extraction. The package, XML and path handling use
//! anydoc's existing bounded readers; no ofdrw runtime dependency is needed.

use crate::error::ConvertError;
use crate::model::{
    Block, Cell, Document, GridBuilder, Inline, LinkTarget, List, ListItem, MarkerKind, Style,
    Table, TableKind,
};
use crate::package::xml::Element;
use crate::package::{Package, path};
use crate::shared::header::resolve_header_rows;
use std::collections::HashMap;

const OFD: &str = "http://www.ofdspec.org/2016";

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let mut pkg = Package::open(bytes)?;
    if pkg.has_part("Encryptions.xml") {
        return Err(ConvertError::Encrypted);
    }
    let tree = pkg.required_xml_part("OFD.xml")?;
    let root = tree
        .child_elems()
        .next()
        .ok_or_else(|| ConvertError::malformed_part("OFD.xml", "expected OFD root element"))?;
    if !root.is(OFD, "OFD") {
        return Err(ConvertError::malformed_part("OFD.xml", "expected OFD root element"));
    }

    let mut doc = Document::default();
    let mut bodies = 0;
    let mut page_count = 0u32;
    let mut ocr_pages = Vec::new();
    for body in root.find_all(OFD, "DocBody") {
        bodies += 1;
        let root_loc = body
            .find(OFD, "DocRoot")
            .map(Element::text)
            .ok_or_else(|| ConvertError::malformed_part("OFD.xml", "DocBody has no DocRoot"))?;
        let root_path = resolve("OFD.xml", root_loc.trim())?;
        let document_tree = pkg.required_xml_part(&root_path)?;
        let document = document_tree.child_elems().next().ok_or_else(|| {
            ConvertError::malformed_part(&root_path, "expected Document root element")
        })?;
        if !document.is(OFD, "Document") {
            return Err(ConvertError::malformed_part(&root_path, "expected Document root element"));
        }
        let pages = document.find(OFD, "Pages").ok_or_else(|| {
            ConvertError::malformed_part(&root_path, "Document has no Pages element")
        })?;
        let templates: HashMap<&str, String> = document
            .find(OFD, "CommonData")
            .into_iter()
            .flat_map(|data| data.find_all(OFD, "TemplatePage"))
            .filter_map(|template| {
                Some((template.attr_any("ID")?, template.attr_any("BaseLoc")?.to_string()))
            })
            .collect();
        let mut annotation_pages = HashMap::new();
        if let Some(loc) = document.find(OFD, "Annotations").map(Element::text) {
            let list_path = resolve(&root_path, loc.trim())?;
            if let Some(tree) = pkg.optional_xml_part(&list_path)? {
                for item in tree.descendants(OFD, "Page") {
                    if let (Some(id), Some(file)) =
                        (item.attr_any("PageID"), item.find(OFD, "FileLoc"))
                    {
                        annotation_pages
                            .insert(id.to_string(), resolve(&list_path, file.text().trim())?);
                    }
                }
            }
        }
        let mut headings = Vec::new();
        if let Some(outlines) = document.find(OFD, "Outlines") {
            collect_outlines(outlines, 0, &mut headings);
        }
        for page in pages.find_all(OFD, "Page") {
            page_count += 1;
            let Some(loc) = page.attr_any("BaseLoc") else {
                return Err(ConvertError::malformed_part(&root_path, "Page has no BaseLoc"));
            };
            let page_path = resolve(&root_path, loc)?;
            let page_tree = pkg.required_xml_part(&page_path)?;
            let page_xml = page_tree.child_elems().next().ok_or_else(|| {
                ConvertError::malformed_part(&page_path, "expected Page root element")
            })?;
            if !page_xml.is(OFD, "Page") {
                return Err(ConvertError::malformed_part(&page_path, "expected Page root element"));
            }
            let links = if let Some(annotation_path) =
                page.attr_any("ID").and_then(|id| annotation_pages.get(id))
            {
                read_links(&mut pkg, annotation_path)?
            } else {
                Vec::new()
            };
            let mut template_stats = ContentStats::default();
            for template in page_xml.find_all(OFD, "Template") {
                if template.attr_any("ZOrder") != Some("Foreground") {
                    template_stats.merge(append_template(
                        &mut pkg, &templates, &root_path, template, &mut doc,
                    )?);
                }
            }
            let mut page_stats = ContentStats::default();
            if let Some(content) = page_xml.find(OFD, "Content") {
                page_stats = append_content(content, &page_path, &mut doc, &mut headings, &links)?;
            }
            for template in page_xml.find_all(OFD, "Template") {
                if template.attr_any("ZOrder") == Some("Foreground") {
                    template_stats.merge(append_template(
                        &mut pkg, &templates, &root_path, template, &mut doc,
                    )?);
                }
            }
            if (page_stats.image && !page_stats.text)
                || (template_stats.image && !template_stats.text && !page_stats.text)
            {
                ocr_pages.push(page_count);
            }
        }
    }
    if bodies == 0 {
        return Err(ConvertError::malformed_part("OFD.xml", "no DocBody elements"));
    }
    if !ocr_pages.is_empty() {
        return Err(ConvertError::NeedsOcr { pages: ocr_pages, page_count });
    }
    Ok(doc)
}

fn append_template(
    pkg: &mut Package<'_>,
    templates: &HashMap<&str, String>,
    document_path: &str,
    template: &Element,
    doc: &mut Document,
) -> Result<ContentStats, ConvertError> {
    let Some(loc) = template.attr_any("TemplateID").and_then(|id| templates.get(id)) else {
        return Ok(ContentStats::default());
    };
    let template_path = resolve(document_path, loc)?;
    let tree = pkg.required_xml_part(&template_path)?;
    let Some(page) = tree.child_elems().next() else {
        return Err(ConvertError::malformed_part(&template_path, "empty template page"));
    };
    if !page.is(OFD, "Page") {
        return Err(ConvertError::malformed_part(&template_path, "expected Page root element"));
    }
    if let Some(content) = page.find(OFD, "Content") {
        return append_content(content, &template_path, doc, &mut Vec::new(), &[]);
    }
    Ok(ContentStats::default())
}

#[derive(Default)]
struct ContentStats {
    image: bool,
    text: bool,
}

impl ContentStats {
    fn merge(&mut self, other: Self) {
        self.image |= other.image;
        self.text |= other.text;
    }
}

fn read_links(pkg: &mut Package<'_>, part: &str) -> Result<Vec<(Bounds, String)>, ConvertError> {
    let Some(tree) = pkg.optional_xml_part(part)? else { return Ok(Vec::new()) };
    let mut links = Vec::new();
    for annot in tree.descendants(OFD, "Annot") {
        if annot.attr_any("Type") != Some("Link") {
            continue;
        }
        let Some(appearance) = annot.find(OFD, "Appearance") else { continue };
        let Some(bounds) = appearance.attr_any("Boundary").and_then(Bounds::parse) else {
            continue;
        };
        let Some(uri) = appearance
            .descendants(OFD, "URI")
            .next()
            .and_then(|uri| uri.attr_any("URI"))
            .filter(|uri| !uri.is_empty())
        else {
            continue;
        };
        links.push((bounds, uri.to_string()));
    }
    Ok(links)
}

#[derive(Clone, Copy)]
struct Bounds {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Bounds {
    fn parse(value: &str) -> Option<Self> {
        let values =
            value.split_whitespace().map(str::parse::<f64>).collect::<Result<Vec<_>, _>>().ok()?;
        let [x, y, w, h] = values.as_slice() else { return None };
        if !values.iter().all(|v| v.is_finite()) || *w <= 0.0 || *h <= 0.0 {
            return None;
        }
        Some(Self { x: *x, y: *y, w: *w, h: *h })
    }

    fn contains_center(self, other: Self) -> bool {
        let x = other.x + other.w / 2.0;
        let y = other.y + other.h / 2.0;
        x >= self.x - 0.1
            && x <= self.x + self.w + 0.1
            && y >= self.y - 0.1
            && y <= self.y + self.h + 0.1
    }
}

struct TextObject {
    text: String,
    style: Style,
    size: Option<f64>,
    bounds: Option<Bounds>,
    link: Option<String>,
}

fn text_inline(object: &TextObject) -> Inline {
    let text = Inline::Text { text: object.text.trim().to_string(), style: object.style };
    match &object.link {
        Some(uri) => {
            Inline::Link { content: vec![text], target: LinkTarget::External(uri.clone()) }
        }
        None => text,
    }
}

fn collect_outlines(parent: &Element, depth: u8, headings: &mut Vec<(String, u8)>) {
    for item in parent.find_all(OFD, "OutlineElem") {
        if let Some(title) = item.attr_any("Title") {
            headings.push((title.trim().to_string(), depth.saturating_add(1).min(6)));
        }
        collect_outlines(item, depth.saturating_add(1), headings);
    }
}

fn append_content(
    content: &Element,
    part: &str,
    doc: &mut Document,
    headings: &mut Vec<(String, u8)>,
    links: &[(Bounds, String)],
) -> Result<ContentStats, ConvertError> {
    let start = doc.blocks.len();
    let mut objects = Vec::new();
    let mut stats = ContentStats {
        image: content.descendants(OFD, "ImageObject").next().is_some(),
        text: false,
    };
    for object in content.descendants(OFD, "TextObject") {
        let mut text = String::new();
        for code in object.find_all(OFD, "TextCode") {
            text.push_str(&decode_text_code(&code.text(), part)?);
        }
        if text.trim().is_empty() {
            continue;
        }
        stats.text = true;
        let weight = object.attr_any("Weight").and_then(|v| v.parse::<u16>().ok()).unwrap_or(400);
        let style = Style {
            bold: weight >= 600,
            italic: object.attr_any("Italic").is_some_and(|v| v.eq_ignore_ascii_case("true")),
            ..Style::PLAIN
        };
        let bounds = object.attr_any("Boundary").and_then(Bounds::parse);
        objects.push(TextObject {
            text,
            style,
            size: object
                .attr_any("Size")
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| *v > 0.0 && v.is_finite()),
            bounds,
            link: object
                .descendants(OFD, "URI")
                .next()
                .and_then(|uri| uri.attr_any("URI"))
                .filter(|uri| !uri.is_empty())
                .map(str::to_string)
                .or_else(|| {
                    bounds.and_then(|bounds| {
                        links
                            .iter()
                            .find(|(region, _)| region.contains_center(bounds))
                            .map(|(_, uri)| uri.clone())
                    })
                }),
        });
    }

    let rects: Vec<Bounds> =
        content.descendants(OFD, "PathObject").filter_map(rectangle_border).collect();
    let tables = detect_tables(&rects, &objects)?;
    let mut used = vec![false; objects.len()];
    let mut table_at = Vec::new();
    for (table, indexes) in tables {
        if indexes.iter().any(|index| used[*index]) {
            continue;
        }
        if let Some(at) = indexes.iter().copied().min() {
            table_at.push((at, table));
        }
        for index in indexes {
            used[index] = true;
        }
    }
    table_at.sort_by_key(|(at, _)| *at);
    let mut table_at = table_at.into_iter().peekable();
    let mut sizes: Vec<f64> =
        objects.iter().enumerate().filter(|(i, _)| !used[*i]).filter_map(|(_, o)| o.size).collect();
    sizes.sort_by(f64::total_cmp);
    let body_size = sizes.get(sizes.len().saturating_sub(1) / 2).copied();
    for (index, object) in objects.into_iter().enumerate() {
        if table_at.peek().is_some_and(|(at, _)| *at == index) {
            let (_, table) = table_at.next().expect("table anchor exists");
            doc.blocks.push(Block::Table(table));
        }
        if used[index] {
            continue;
        }
        let text = object.text.trim().to_string();
        let inline = text_inline(&object);
        if let Some(pos) = headings.iter().position(|(title, _)| title == &text) {
            let (_, level) = headings.remove(pos);
            doc.blocks.push(Block::heading(level, vec![inline]));
        } else if let (Some(size), Some(body)) = (object.size, body_size)
            && size >= body * 1.4
            && text.chars().count() <= 120
        {
            let level = if size >= body * 1.8 {
                1
            } else if size >= body * 1.6 {
                2
            } else {
                3
            };
            doc.blocks.push(Block::heading(level, vec![inline]));
        } else {
            doc.blocks.push(Block::Paragraph(vec![inline]));
        }
    }
    fold_lists(&mut doc.blocks, start);
    Ok(stats)
}

fn rectangle_border(path: &Element) -> Option<Bounds> {
    if !path.attr_any("Stroke")?.eq_ignore_ascii_case("true") {
        return None;
    }
    let rect = Bounds::parse(path.attr_any("Boundary")?)?;
    let data = path.find(OFD, "AbbreviatedData")?.text();
    let parts: Vec<&str> = data.split_whitespace().collect();
    let ["M", x0, y0, "L", x1, y1, "L", x2, y2, "L", x3, y3, "C"] = parts.as_slice() else {
        return None;
    };
    let points = [x0, y0, x1, y1, x2, y2, x3, y3]
        .map(|v| v.parse::<f64>().ok())
        .into_iter()
        .collect::<Option<Vec<_>>>()?;
    let expected = [0.0, 0.0, rect.w, 0.0, rect.w, rect.h, 0.0, rect.h];
    if points.iter().zip(expected).all(|(actual, want)| (*actual - want).abs() < 0.1) {
        Some(rect)
    } else {
        None
    }
}

fn list_marker(text: &str) -> Option<(MarkerKind, u64, &str)> {
    let text = text.trim_start();
    if let Some(body) = text.strip_prefix(['•', '●', '-', '*'])
        && body.starts_with(char::is_whitespace)
    {
        return Some((MarkerKind::Bullet, 1, body.trim_start()));
    }
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let number = text[..digits].parse().ok()?;
    let rest = text.get(digits..)?;
    let body = rest.strip_prefix(['.', ')'])?;
    if body.starts_with(char::is_whitespace) {
        Some((MarkerKind::Decimal, number, body.trim_start()))
    } else {
        None
    }
}

fn fold_lists(blocks: &mut Vec<Block>, start: usize) {
    let tail = blocks.split_off(start);
    let mut output = Vec::with_capacity(tail.len());
    let mut iter = tail.into_iter().peekable();
    while let Some(block) = iter.next() {
        let Block::Paragraph(inlines) = &block else {
            output.push(block);
            continue;
        };
        let [Inline::Text { text, .. }] = inlines.as_slice() else {
            output.push(block);
            continue;
        };
        let Some((kind, number, _)) = list_marker(text) else {
            output.push(block);
            continue;
        };
        let mut run = vec![block];
        while let Some(Block::Paragraph(next)) = iter.peek() {
            let [Inline::Text { text, .. }] = next.as_slice() else { break };
            let Some((next_kind, next_number, _)) = list_marker(text) else { break };
            if next_kind != kind
                || (kind == MarkerKind::Decimal && next_number != number + run.len() as u64)
            {
                break;
            }
            run.push(iter.next().expect("peeked block exists"));
        }
        if run.len() < 2 {
            output.extend(run);
            continue;
        }
        let items = run
            .into_iter()
            .filter_map(|block| {
                let Block::Paragraph(mut inlines) = block else { return None };
                let Inline::Text { text, .. } = &mut inlines[0] else { return None };
                let (_, _, body) = list_marker(text)?;
                *text = body.to_string();
                Some(ListItem { blocks: vec![Block::Paragraph(inlines)], marker_label: None })
            })
            .collect();
        output.push(Block::List(List { marker: kind, start: number, items }));
    }
    blocks.extend(output);
}

fn detect_tables(
    rects: &[Bounds],
    objects: &[TextObject],
) -> Result<Vec<(Table, Vec<usize>)>, ConvertError> {
    if rects.len() < 4 || rects.len() > 2000 || objects.len() > 5000 {
        return Ok(Vec::new());
    }
    let mut seen = vec![false; rects.len()];
    let mut tables = Vec::new();
    for first in 0..rects.len() {
        if seen[first] {
            continue;
        }
        seen[first] = true;
        let mut group = vec![rects[first]];
        let mut pending = vec![first];
        while let Some(index) = pending.pop() {
            for other in 0..rects.len() {
                if !seen[other] && borders_touch(rects[index], rects[other]) {
                    seen[other] = true;
                    group.push(rects[other]);
                    pending.push(other);
                }
            }
        }
        if let Some(table) = detect_table(&group, objects)? {
            tables.push(table);
        }
    }
    Ok(tables)
}

fn borders_touch(a: Bounds, b: Bounds) -> bool {
    let overlap = |a0: f64, a1: f64, b0: f64, b1: f64| a1.min(b1) - a0.max(b0) > 0.1;
    (((a.x + a.w - b.x).abs() < 0.1 || (b.x + b.w - a.x).abs() < 0.1)
        && overlap(a.y, a.y + a.h, b.y, b.y + b.h))
        || (((a.y + a.h - b.y).abs() < 0.1 || (b.y + b.h - a.y).abs() < 0.1)
            && overlap(a.x, a.x + a.w, b.x, b.x + b.w))
}

fn detect_table(
    rects: &[Bounds],
    objects: &[TextObject],
) -> Result<Option<(Table, Vec<usize>)>, ConvertError> {
    // A border-only grid is strong evidence for a table. Without a complete
    // grid, leave the text as paragraphs rather than inventing cell order.
    if rects.len() < 4 || rects.len() > 2000 {
        return Ok(None);
    }
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for rect in rects {
        xs.extend([rect.x, rect.x + rect.w]);
        ys.extend([rect.y, rect.y + rect.h]);
    }
    xs.sort_by(f64::total_cmp);
    ys.sort_by(f64::total_cmp);
    xs.dedup_by(|a, b| (*a - *b).abs() < 0.1);
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.1);
    let columns = xs.len().saturating_sub(1);
    let rows = ys.len().saturating_sub(1);
    if columns < 2 || rows < 2 || columns.saturating_mul(rows) > 2000 {
        return Ok(None);
    }
    let mut slots = vec![false; rows * columns];
    let mut origins = Vec::new();
    for rect in rects {
        let edge = |edges: &[f64], value: f64| edges.iter().position(|v| (*v - value).abs() < 0.1);
        let (Some(c0), Some(c1), Some(r0), Some(r1)) = (
            edge(&xs, rect.x),
            edge(&xs, rect.x + rect.w),
            edge(&ys, rect.y),
            edge(&ys, rect.y + rect.h),
        ) else {
            return Ok(None);
        };
        if c0 >= c1 || r0 >= r1 {
            return Ok(None);
        }
        for row in r0..r1 {
            for column in c0..c1 {
                let slot = &mut slots[row * columns + column];
                if *slot {
                    return Ok(None);
                }
                *slot = true;
            }
        }
        origins.push((r0, c0, r1 - r0, c1 - c0, *rect));
    }
    if slots.iter().any(|slot| !slot) {
        return Ok(None);
    }
    origins.sort_by_key(|(row, col, ..)| (*row, *col));
    let mut builder = GridBuilder::new();
    let mut used = Vec::new();
    let mut first_row_bold = true;
    let mut first_row_has_text = false;
    let mut later_bold = false;
    for row in 0..rows {
        builder.next_row();
        for (_, _, row_span, col_span, rect) in origins.iter().filter(|(r, ..)| *r == row) {
            let mut content = Vec::new();
            for (index, object) in objects.iter().enumerate() {
                if let Some(bounds) = object.bounds
                    && rect.contains_center(bounds)
                    && !used.contains(&index)
                {
                    content.push(text_inline(object));
                    used.push(index);
                    if row == 0 {
                        first_row_has_text = true;
                        first_row_bold &= object.style.bold;
                    } else {
                        later_bold |= object.style.bold;
                    }
                }
            }
            let blocks =
                if content.is_empty() { Vec::new() } else { vec![Block::Paragraph(content)] };
            builder.place(Cell::spanning(blocks, *col_span as u32, *row_span as u32))?;
        }
    }
    if used.len() < 2 {
        return Ok(None);
    }
    let mut table = builder.finish(TableKind::Data);
    table.header_rows = resolve_header_rows(
        &table,
        usize::from(first_row_has_text && first_row_bold && !later_bold),
    );
    Ok(Some((table, used)))
}

fn resolve(base: &str, loc: &str) -> Result<String, ConvertError> {
    if loc.is_empty() {
        return Err(ConvertError::malformed_part(base, "empty OFD part location"));
    }
    let target = path::resolve(base, loc)?;
    if target.path.is_empty() {
        return Err(ConvertError::malformed_part(base, "empty OFD part location"));
    }
    Ok(target.path)
}

// OFD TextCode uses backslash followed by four hex digits for UTF-16 units.
// Surrogate pairs are decoded together; literal backslashes remain literal.
fn decode_text_code(raw: &str, part: &str) -> Result<String, ConvertError> {
    let bytes = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && bytes.get(i + 1..i + 5).is_some_and(|s| s.iter().all(u8::is_ascii_hexdigit))
        {
            let unit = u16::from_str_radix(&raw[i + 1..i + 5], 16)
                .map_err(|_| ConvertError::malformed_part(part, "invalid TextCode escape"))?;
            i += 5;
            let scalar = if (0xD800..=0xDBFF).contains(&unit) {
                if bytes.get(i) != Some(&b'\\')
                    || !bytes.get(i + 1..i + 5).is_some_and(|s| s.iter().all(u8::is_ascii_hexdigit))
                {
                    return Err(ConvertError::malformed_part(part, "unpaired TextCode surrogate"));
                }
                let low = u16::from_str_radix(&raw[i + 1..i + 5], 16)
                    .map_err(|_| ConvertError::malformed_part(part, "invalid TextCode escape"))?;
                if !(0xDC00..=0xDFFF).contains(&low) {
                    return Err(ConvertError::malformed_part(part, "unpaired TextCode surrogate"));
                }
                i += 5;
                0x10000 + (u32::from(unit - 0xD800) << 10) + u32::from(low - 0xDC00)
            } else {
                u32::from(unit)
            };
            let ch = char::from_u32(scalar)
                .ok_or_else(|| ConvertError::malformed_part(part, "unpaired TextCode surrogate"))?;
            out.push(ch);
        } else {
            let ch = raw[i..]
                .chars()
                .next()
                .ok_or_else(|| ConvertError::malformed_part(part, "invalid TextCode"))?;
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn package(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, xml) in parts {
            writer.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            writer.write_all(xml.as_bytes()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn text_from_pages_and_bodies_in_declared_order() {
        let bytes = package(&[
            (
                "OFD.xml",
                r#"<ofd:OFD xmlns:ofd="http://www.ofdspec.org/2016"><ofd:DocBody><ofd:DocRoot>Doc_0/Document.xml</ofd:DocRoot></ofd:DocBody><ofd:DocBody><ofd:DocRoot>Doc_1/Document.xml</ofd:DocRoot></ofd:DocBody></ofd:OFD>"#,
            ),
            (
                "Doc_0/Document.xml",
                r#"<Document xmlns="http://www.ofdspec.org/2016"><CommonData><TemplatePage ID="8" BaseLoc="Tpl/header.xml"/></CommonData><Pages><Page ID="2" BaseLoc="Pages/p2.xml"/><Page ID="1" BaseLoc="Pages/p1.xml"/></Pages></Document>"#,
            ),
            (
                "Doc_0/Pages/p2.xml",
                r#"<Page xmlns="http://www.ofdspec.org/2016"><Template TemplateID="8" ZOrder="Background"/><Content><Layer><TextObject><TextCode>Second</TextCode><TextCode> page</TextCode></TextObject></Layer></Content></Page>"#,
            ),
            (
                "Doc_0/Tpl/header.xml",
                r#"<Page xmlns="http://www.ofdspec.org/2016"><Content><Layer><TextObject><TextCode>Header</TextCode></TextObject></Layer></Content></Page>"#,
            ),
            (
                "Doc_0/Pages/p1.xml",
                r#"<Page xmlns="http://www.ofdspec.org/2016"><Content><Layer><PageBlock><TextObject><TextCode>First page</TextCode></TextObject></PageBlock></Layer></Content></Page>"#,
            ),
            (
                "Doc_1/Document.xml",
                r#"<Document xmlns="http://www.ofdspec.org/2016"><Pages><Page BaseLoc="Page.xml"/></Pages></Document>"#,
            ),
            (
                "Doc_1/Page.xml",
                r#"<Page xmlns="http://www.ofdspec.org/2016"><Content><Layer><TextObject><TextCode>\4F60\597D \D83D\DE00</TextCode></TextObject></Layer></Content></Page>"#,
            ),
        ]);
        assert_eq!(crate::Format::from_bytes(&bytes), Some(crate::Format::Ofd));
        assert_eq!(
            crate::to_markdown_bytes(&bytes, None).unwrap(),
            "Header\n\nSecond page\n\nFirst page\n\n你好 😀\n"
        );
    }

    #[test]
    fn encrypted_package_is_rejected() {
        let bytes = package(&[(
            "Encryptions.xml",
            r#"<Encryptions xmlns="http://www.ofdspec.org/2016"/>"#,
        )]);
        assert_eq!(crate::Format::from_bytes(&bytes), Some(crate::Format::Ofd));
        assert!(matches!(parse(&bytes), Err(ConvertError::Encrypted)));
    }

    #[test]
    fn invalid_surrogate_is_rejected() {
        assert!(decode_text_code("\\D83D", "page.xml").is_err());
    }

    #[test]
    fn outline_emphasis_and_bordered_table_render_as_markdown() {
        let bytes = package(&[
            (
                "OFD.xml",
                r#"<OFD xmlns="http://www.ofdspec.org/2016"><DocBody><DocRoot>Doc/Document.xml</DocRoot></DocBody></OFD>"#,
            ),
            (
                "Doc/Document.xml",
                r#"<Document xmlns="http://www.ofdspec.org/2016"><Outlines><OutlineElem Title="Report"><OutlineElem Title="Details"/></OutlineElem></Outlines><Pages><Page BaseLoc="Page.xml"/></Pages></Document>"#,
            ),
            (
                "Doc/Page.xml",
                r#"<Page xmlns="http://www.ofdspec.org/2016"><Content><Layer>
                <TextObject Size="8" Weight="700" Boundary="0 0 80 10"><TextCode>Report</TextCode></TextObject>
                <TextObject Size="6" Boundary="0 11 80 8"><TextCode>Details</TextCode></TextObject>
                <TextObject Size="4" Italic="true" Boundary="0 70 80 6"><TextCode>Note</TextCode></TextObject>
                <PathObject Stroke="true" Boundary="0 20 40 10"><AbbreviatedData>M 0 0 L 40 0 L 40 10 L 0 10 C</AbbreviatedData></PathObject>
                <PathObject Stroke="true" Boundary="40 20 40 10"><AbbreviatedData>M 0 0 L 40 0 L 40 10 L 0 10 C</AbbreviatedData></PathObject>
                <PathObject Stroke="true" Boundary="0 30 40 10"><AbbreviatedData>M 0 0 L 40 0 L 40 10 L 0 10 C</AbbreviatedData></PathObject>
                <PathObject Stroke="true" Boundary="40 30 40 10"><AbbreviatedData>M 0 0 L 40 0 L 40 10 L 0 10 C</AbbreviatedData></PathObject>
                <TextObject Weight="700" Boundary="2 22 35 6"><TextCode>Name</TextCode></TextObject>
                <TextObject Weight="700" Boundary="42 22 35 6"><TextCode>Score</TextCode></TextObject>
                <TextObject Boundary="2 32 35 6"><TextCode>Ada</TextCode></TextObject>
                <TextObject Boundary="42 32 35 6"><TextCode>95</TextCode></TextObject>
            </Layer></Content></Page>"#,
            ),
        ]);
        let markdown = crate::to_markdown_bytes(&bytes, None).unwrap();
        assert!(markdown.contains("# **Report**\n\n## Details"), "{markdown}");
        assert!(
            markdown.contains("| **Name** | **Score** |\n| --- | --- |\n| Ada | 95 |"),
            "{markdown}"
        );
        assert!(markdown.contains("*Note*"), "{markdown}");
    }

    #[test]
    fn larger_font_without_outline_becomes_heading() {
        let bytes = package(&[
            (
                "OFD.xml",
                r#"<OFD xmlns="http://www.ofdspec.org/2016"><DocBody><DocRoot>Doc/Document.xml</DocRoot></DocBody></OFD>"#,
            ),
            (
                "Doc/Document.xml",
                r#"<Document xmlns="http://www.ofdspec.org/2016"><Pages><Page BaseLoc="Page.xml"/></Pages></Document>"#,
            ),
            (
                "Doc/Page.xml",
                r#"<Page xmlns="http://www.ofdspec.org/2016"><Content><Layer><TextObject Size="9"><TextCode>Large title</TextCode></TextObject><TextObject Size="4"><TextCode>Body text</TextCode></TextObject><TextObject Size="4"><TextCode>More body</TextCode></TextObject></Layer></Content></Page>"#,
            ),
        ]);
        assert_eq!(
            crate::to_markdown_bytes(&bytes, None).unwrap(),
            "# Large title\n\nBody text\n\nMore body\n"
        );
    }

    #[test]
    fn consecutive_printed_markers_become_lists() {
        let bytes = package(&[
            (
                "OFD.xml",
                r#"<OFD xmlns="http://www.ofdspec.org/2016"><DocBody><DocRoot>Doc/Document.xml</DocRoot></DocBody></OFD>"#,
            ),
            (
                "Doc/Document.xml",
                r#"<Document xmlns="http://www.ofdspec.org/2016"><Pages><Page BaseLoc="Page.xml"/></Pages></Document>"#,
            ),
            (
                "Doc/Page.xml",
                r#"<Page xmlns="http://www.ofdspec.org/2016"><Content><Layer><TextObject><TextCode>• First</TextCode></TextObject><TextObject><TextCode>• Second</TextCode></TextObject><TextObject><TextCode>2. Two</TextCode></TextObject><TextObject><TextCode>3. Three</TextCode></TextObject></Layer></Content></Page>"#,
            ),
        ]);
        assert_eq!(
            crate::to_markdown_bytes(&bytes, None).unwrap(),
            "- First\n- Second\n\n2. Two\n3. Three\n"
        );
    }

    #[test]
    fn separate_table_grids_are_detected_independently() {
        let mut rects = Vec::new();
        let mut objects = Vec::new();
        for offset in [0.0, 100.0] {
            for row in 0..2 {
                for col in 0..2 {
                    let x = offset + f64::from(col) * 40.0;
                    let y = f64::from(row) * 10.0;
                    rects.push(Bounds { x, y, w: 40.0, h: 10.0 });
                    objects.push(TextObject {
                        text: format!("{row}-{col}"),
                        style: Style::PLAIN,
                        size: Some(4.0),
                        bounds: Some(Bounds { x: x + 2.0, y: y + 2.0, w: 30.0, h: 6.0 }),
                        link: None,
                    });
                }
            }
        }
        assert_eq!(detect_tables(&rects, &objects).unwrap().len(), 2);
    }

    #[test]
    fn uri_link_annotation_wraps_text_in_markdown_link() {
        let bytes = package(&[
            (
                "OFD.xml",
                r#"<OFD xmlns="http://www.ofdspec.org/2016"><DocBody><DocRoot>Doc/Document.xml</DocRoot></DocBody></OFD>"#,
            ),
            (
                "Doc/Document.xml",
                r#"<Document xmlns="http://www.ofdspec.org/2016"><Pages><Page ID="1" BaseLoc="Page.xml"/></Pages><Annotations>Annots/Annotations.xml</Annotations></Document>"#,
            ),
            (
                "Doc/Page.xml",
                r#"<Page xmlns="http://www.ofdspec.org/2016"><Content><Layer><TextObject Boundary="10 10 40 8"><TextCode>Read more</TextCode></TextObject></Layer></Content></Page>"#,
            ),
            (
                "Doc/Annots/Annotations.xml",
                r#"<Annotations xmlns="http://www.ofdspec.org/2016"><Page PageID="1"><FileLoc>Page_1/Annotation.xml</FileLoc></Page></Annotations>"#,
            ),
            (
                "Doc/Annots/Page_1/Annotation.xml",
                r#"<PageAnnot xmlns="http://www.ofdspec.org/2016"><Annot Type="Link"><Appearance Boundary="10 10 40 8"><PathObject><Actions><Action Event="CLICK"><URI URI="https://example.com"/></Action></Actions></PathObject></Appearance></Annot></PageAnnot>"#,
            ),
        ]);
        assert_eq!(
            crate::to_markdown_bytes(&bytes, None).unwrap(),
            "[Read more](https://example.com)\n"
        );
    }

    #[test]
    fn image_only_page_needs_ocr() {
        let bytes = package(&[
            (
                "OFD.xml",
                r#"<OFD xmlns="http://www.ofdspec.org/2016"><DocBody><DocRoot>Doc/Document.xml</DocRoot></DocBody></OFD>"#,
            ),
            (
                "Doc/Document.xml",
                r#"<Document xmlns="http://www.ofdspec.org/2016"><Pages><Page BaseLoc="Page.xml"/></Pages></Document>"#,
            ),
            (
                "Doc/Page.xml",
                r#"<Page xmlns="http://www.ofdspec.org/2016"><Content><Layer><ImageObject ResourceID="1"/></Layer></Content></Page>"#,
            ),
        ]);
        assert!(
            matches!(parse(&bytes), Err(ConvertError::NeedsOcr { pages, page_count }) if pages == [1] && page_count == 1)
        );
    }

    #[test]
    fn empty_text_object_does_not_hide_image_page() {
        let bytes = package(&[
            (
                "OFD.xml",
                r#"<OFD xmlns="http://www.ofdspec.org/2016"><DocBody><DocRoot>Doc/Document.xml</DocRoot></DocBody></OFD>"#,
            ),
            (
                "Doc/Document.xml",
                r#"<Document xmlns="http://www.ofdspec.org/2016"><Pages><Page BaseLoc="Page.xml"/></Pages></Document>"#,
            ),
            (
                "Doc/Page.xml",
                r#"<Page xmlns="http://www.ofdspec.org/2016"><Content><Layer><ImageObject ResourceID="1"/><TextObject><TextCode>   </TextCode></TextObject></Layer></Content></Page>"#,
            ),
        ]);
        assert!(
            matches!(parse(&bytes), Err(ConvertError::NeedsOcr { pages, page_count }) if pages == [1] && page_count == 1)
        );
    }

    #[test]
    fn image_only_template_needs_ocr() {
        let bytes = package(&[
            (
                "OFD.xml",
                r#"<OFD xmlns="http://www.ofdspec.org/2016"><DocBody><DocRoot>Doc/Document.xml</DocRoot></DocBody></OFD>"#,
            ),
            (
                "Doc/Document.xml",
                r#"<Document xmlns="http://www.ofdspec.org/2016"><CommonData><TemplatePage ID="1" BaseLoc="Tpl.xml"/></CommonData><Pages><Page BaseLoc="Page.xml"/></Pages></Document>"#,
            ),
            (
                "Doc/Page.xml",
                r#"<Page xmlns="http://www.ofdspec.org/2016"><Template TemplateID="1" ZOrder="Background"/></Page>"#,
            ),
            (
                "Doc/Tpl.xml",
                r#"<Page xmlns="http://www.ofdspec.org/2016"><Content><Layer><ImageObject ResourceID="1"/></Layer></Content></Page>"#,
            ),
        ]);
        assert!(
            matches!(parse(&bytes), Err(ConvertError::NeedsOcr { pages, page_count }) if pages == [1] && page_count == 1)
        );
    }

    #[test]
    fn empty_first_row_is_not_a_table_header() {
        let rects = (0..2)
            .flat_map(|row| {
                (0..2).map(move |col| Bounds {
                    x: f64::from(col) * 40.0,
                    y: f64::from(row) * 10.0,
                    w: 40.0,
                    h: 10.0,
                })
            })
            .collect::<Vec<_>>();
        let objects = (0..2)
            .map(|col| TextObject {
                text: format!("value {col}"),
                style: Style::PLAIN,
                size: None,
                bounds: Some(Bounds { x: f64::from(col) * 40.0 + 2.0, y: 12.0, w: 30.0, h: 6.0 }),
                link: None,
            })
            .collect::<Vec<_>>();
        let (table, _) = detect_table(&rects, &objects).unwrap().unwrap();
        assert_eq!(table.header_rows, 0);
    }
}
