//! MHTML/MHT MIME container frontend.

use crate::error::ConvertError;
use crate::model::{AnchorId, Asset, AssetId, Document, ImageSource, LinkTarget};
use crate::package::limits;
use crate::shared::html::HtmlCtx;
use crate::shared::uri::is_absolute_uri;
use mail_parser::decoders::{base64::base64_decode, quoted_printable::quoted_printable_decode};
use mail_parser::{Encoding, Message, MessageParser, MessagePart, MimeHeaders};
use scraper::Html;
use std::collections::HashMap;

const HEADER_SCAN_LIMIT: usize = 64 * 1024;

pub(crate) fn looks_like_mhtml(bytes: &[u8]) -> bool {
    let header = mime_header_block(bytes);
    let unfolded = unfold_headers(header);
    let lower = unfolded.to_ascii_lowercase();
    let snapshot = lower.lines().any(|line| line.starts_with("snapshot-content-location:"));

    let Some(content_type) = lower.lines().find_map(|line| line.strip_prefix("content-type:"))
    else {
        return false;
    };
    let compact: String = content_type.chars().filter(|c| !c.is_ascii_whitespace()).collect();

    (compact == "multipart/related" || compact.starts_with("multipart/related;")) && snapshot
}

fn mime_header_block(bytes: &[u8]) -> &[u8] {
    let bytes = &bytes[..bytes.len().min(HEADER_SCAN_LIMIT)];
    let crlf = bytes.windows(4).position(|w| w == b"\r\n\r\n");
    let lf = bytes.windows(2).position(|w| w == b"\n\n");
    let end = match (crlf, lf) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => bytes.len(),
    };
    &bytes[..end]
}

fn unfold_headers(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    for raw in text.split('\n') {
        let line = raw.trim_end_matches('\r');
        if line.starts_with([' ', '\t']) {
            out.push(' ');
            out.push_str(line.trim());
        } else {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
    }
    out
}

pub(crate) fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    if bytes.len() as u64 > limits::MAX_TOTAL_BYTES {
        return Err(ConvertError::ResourceLimit {
            limit: "max_total_bytes",
            detail: format!(
                "MHTML input is {} bytes; maximum is {}",
                bytes.len(),
                limits::MAX_TOTAL_BYTES
            ),
        });
    }

    let message = MessageParser::new()
        .with_mime_headers()
        .parse(bytes)
        .ok_or_else(|| ConvertError::malformed("MHTML MIME structure could not be parsed"))?;

    if !message.is_content_type("multipart", "related") {
        return Err(ConvertError::malformed("MHTML root is not multipart/related"));
    }

    let start = message
        .content_type()
        .and_then(|content_type| content_type.attribute("start"))
        .map(normalize_content_id);

    let (_, html_part) = if let Some(start) = start {
        message.parts.iter().enumerate().find(|(_, part)| {
            part.is_content_type("text", "html")
                && part
                    .content_id()
                    .map(normalize_content_id)
                    .is_some_and(|content_id| content_id == start)
        })
    } else {
        message.parts.iter().enumerate().find(|(_, part)| part.is_content_type("text", "html"))
    }
    .ok_or_else(|| ConvertError::malformed("MHTML contains no HTML root part"))?;

    let html_bytes = transfer_decoded_part_bytes(&message, html_part, "HTML root")?;
    if html_bytes.len() as u64 > limits::MAX_ENTRY_BYTES {
        return Err(ConvertError::ResourceLimit {
            limit: "max_entry_bytes",
            detail: format!(
                "MHTML HTML root is {} bytes; maximum is {}",
                html_bytes.len(),
                limits::MAX_ENTRY_BYTES
            ),
        });
    }

    let declared_charset =
        html_part.content_type().and_then(|content_type| content_type.attribute("charset"));
    let html = super::html::decode_html_with_charset(&html_bytes, declared_charset);
    super::html::preflight_html_complexity(&html)?;
    let parsed_html = Html::parse_document(&html);
    let root_location = html_part.content_location();
    let resource_base = html_resource_base(&parsed_html, root_location);

    let resource_index = build_resource_index(&message.parts, resource_base.as_deref());
    let stylesheets = collect_stylesheets_in_order(
        &parsed_html,
        &message.parts,
        &resource_index,
        resource_base.as_deref(),
    )?;
    let (assets, image_assets) = collect_image_assets(&message.parts, resource_base.as_deref())?;
    let ctx = MhtmlCtx { image_assets, resource_base };

    super::html::parse_text_with_context(&html, Some(&stylesheets), &ctx, assets)
}

fn transfer_decoded_part_bytes(
    message: &Message<'_>,
    part: &MessagePart<'_>,
    label: &str,
) -> Result<Vec<u8>, ConvertError> {
    let raw = message
        .raw_message
        .get(part.offset_body as usize..part.offset_end as usize)
        .ok_or_else(|| ConvertError::malformed(format!("MHTML {label} byte range is invalid")))?;

    match part.encoding {
        Encoding::None => Ok(raw.to_vec()),
        Encoding::QuotedPrintable => quoted_printable_decode(raw).ok_or_else(|| {
            ConvertError::malformed(format!("MHTML {label} quoted-printable body is invalid"))
        }),
        Encoding::Base64 => base64_decode(raw).ok_or_else(|| {
            ConvertError::malformed(format!("MHTML {label} base64 body is invalid"))
        }),
    }
}

fn html_resource_base(parsed: &Html, root_location: Option<&str>) -> Option<String> {
    let root = parsed.root_element();
    root.descendent_elements()
        .find(|element| element.value().name() == "base")
        .and_then(|element| element.value().attr("href"))
        .map(|href| resolve_resource_reference(root_location, href))
        .filter(|base| !base.is_empty())
        .or_else(|| root_location.map(canonical_reference))
}

fn build_resource_index(
    parts: &[mail_parser::MessagePart<'_>],
    resource_base: Option<&str>,
) -> HashMap<String, usize> {
    let mut index = HashMap::new();
    for (part_index, part) in parts.iter().enumerate() {
        for key in resource_keys(part, resource_base) {
            index.entry(key).or_insert(part_index);
        }
    }
    index
}

fn collect_stylesheets_in_order(
    parsed: &Html,
    parts: &[mail_parser::MessagePart<'_>],
    resource_index: &HashMap<String, usize>,
    resource_base: Option<&str>,
) -> Result<Vec<String>, ConvertError> {
    let root = parsed.root_element();
    let mut stylesheets = Vec::new();

    for element in root.descendent_elements() {
        match element.value().name() {
            "style" => stylesheets.push(element.text().collect::<String>()),
            "link" => {
                let rel = element.value().attr("rel").unwrap_or("");
                if !rel
                    .split_ascii_whitespace()
                    .any(|token| token.eq_ignore_ascii_case("stylesheet"))
                {
                    continue;
                }
                let Some(href) = element.value().attr("href") else {
                    continue;
                };
                let reference = resolve_resource_reference(resource_base, href);
                let lookup = resource_lookup_key(&reference);
                let Some(&part_index) = resource_index.get(lookup) else {
                    continue;
                };
                let part = &parts[part_index];
                if !part.is_content_type("text", "css") {
                    continue;
                }
                let Some(css) = part.text_contents() else {
                    continue;
                };
                if css.len() as u64 > limits::MAX_ENTRY_BYTES {
                    return Err(ConvertError::ResourceLimit {
                        limit: "max_entry_bytes",
                        detail: format!(
                            "MHTML stylesheet is {} bytes; maximum is {}",
                            css.len(),
                            limits::MAX_ENTRY_BYTES
                        ),
                    });
                }
                stylesheets.push(css.to_owned());
            }
            _ => {}
        }
    }

    Ok(stylesheets)
}

fn collect_image_assets(
    parts: &[mail_parser::MessagePart<'_>],
    resource_base: Option<&str>,
) -> Result<(Vec<Asset>, HashMap<String, AssetId>), ConvertError> {
    let mut assets = Vec::new();
    let mut image_assets = HashMap::new();
    let mut total_bytes = 0u64;

    for (part_index, part) in parts.iter().enumerate() {
        let Some(content_type) = part.content_type() else {
            continue;
        };
        if !content_type.ctype().eq_ignore_ascii_case("image") {
            continue;
        }

        let bytes = part.contents();
        if bytes.len() as u64 > limits::MAX_ENTRY_BYTES {
            return Err(ConvertError::ResourceLimit {
                limit: "max_entry_bytes",
                detail: format!(
                    "MHTML image part is {} bytes; maximum is {}",
                    bytes.len(),
                    limits::MAX_ENTRY_BYTES
                ),
            });
        }
        total_bytes = total_bytes.saturating_add(bytes.len() as u64);
        if total_bytes > limits::MAX_ASSET_TOTAL_BYTES as u64 {
            return Err(ConvertError::ResourceLimit {
                limit: "max_asset_total_bytes",
                detail: format!(
                    "MHTML embedded images retain {total_bytes} bytes; maximum is {}",
                    limits::MAX_ASSET_TOTAL_BYTES
                ),
            });
        }

        let id = AssetId(assets.len());
        let subtype = content_type.subtype().unwrap_or("octet-stream");
        let origin_part = part
            .content_location()
            .or_else(|| part.content_id())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("mhtml-part-{part_index}"));
        assets.push(Asset {
            id,
            media_type: format!("{}/{subtype}", content_type.ctype()),
            origin_part,
            bytes: bytes.to_vec(),
        });
        for key in resource_keys(part, resource_base) {
            image_assets.entry(key).or_insert(id);
        }
    }

    Ok((assets, image_assets))
}

fn resource_keys(part: &mail_parser::MessagePart<'_>, resource_base: Option<&str>) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(content_id) = part.content_id() {
        let id = normalize_content_id(content_id);
        if !id.is_empty() {
            keys.push(format!("cid:{id}"));
        }
    }
    if let Some(location) = part.content_location() {
        let location = resolve_resource_reference(resource_base, location);
        let location = resource_lookup_key(&location).to_owned();
        if !location.is_empty() && !keys.contains(&location) {
            keys.push(location);
        }
    }
    keys
}

fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let head = value.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &value[prefix.len()..])
}

fn canonical_reference(value: &str) -> String {
    let value = value.trim();
    if let Some(content_id) = strip_prefix_ignore_ascii_case(value, "cid:") {
        format!("cid:{}", normalize_content_id(content_id))
    } else {
        value.to_owned()
    }
}

fn normalize_content_id(value: &str) -> String {
    let value = value.trim();
    let value = strip_prefix_ignore_ascii_case(value, "cid:").unwrap_or(value);
    value.trim_matches(|c| matches!(c, '<' | '>')).to_ascii_lowercase()
}

fn resolve_resource_reference(base: Option<&str>, value: &str) -> String {
    let value = canonical_reference(value);
    if value.is_empty() || value.starts_with("cid:") || is_absolute_uri(&value) {
        return value;
    }
    let Some(base) = base else {
        return value;
    };
    if value.starts_with("//") {
        return base
            .split_once(':')
            .map(|(scheme, _)| format!("{scheme}:{value}"))
            .unwrap_or(value);
    }
    let Some((scheme, rest)) = base.split_once("://") else {
        return value;
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let base_with_suffix = if authority_end < rest.len() { &rest[authority_end..] } else { "/" };
    let (base_path, _) = split_path_suffix(base_with_suffix);
    let (reference_path, suffix) = split_path_suffix(&value);
    if reference_path.is_empty() {
        return format!("{scheme}://{authority}{base_path}{suffix}");
    }
    let joined = if reference_path.starts_with('/') {
        reference_path.to_owned()
    } else {
        let directory_end = base_path.rfind('/').map_or(0, |index| index + 1);
        format!("{}{}", &base_path[..directory_end], reference_path)
    };
    format!("{scheme}://{authority}{}{suffix}", normalize_url_path(&joined))
}

fn split_path_suffix(value: &str) -> (&str, &str) {
    let query = value.find('?');
    let fragment = value.find('#');
    let index = match (query, fragment) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    };
    index.map_or((value, ""), |index| value.split_at(index))
}

fn resource_lookup_key(reference: &str) -> &str {
    reference.split_once('#').map_or(reference, |(resource, _)| resource)
}

fn normalize_url_path(path: &str) -> String {
    let trailing_slash = path.ends_with('/');
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            _ => segments.push(segment),
        }
    }
    let mut normalized = String::from("/");
    normalized.push_str(&segments.join("/"));
    if trailing_slash && !normalized.ends_with('/') {
        normalized.push('/');
    }
    normalized
}

struct MhtmlCtx {
    image_assets: HashMap<String, AssetId>,
    resource_base: Option<String>,
}

impl HtmlCtx for MhtmlCtx {
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
        let reference = resolve_resource_reference(self.resource_base.as_deref(), src);
        let lookup = resource_lookup_key(&reference);
        if let Some(&asset_id) = self.image_assets.get(lookup) {
            return Ok(Some(ImageSource::Asset(asset_id)));
        }
        if reference.starts_with("cid:") {
            return Ok(Some(ImageSource::Unavailable));
        }
        Ok((is_absolute_uri(&reference) || reference.starts_with("//"))
            .then_some(ImageSource::External(reference)))
    }

    fn anchor_id(&self, raw: &str) -> AnchorId {
        raw.to_owned()
    }
}
