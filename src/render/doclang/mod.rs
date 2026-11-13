//! PDF → DocLang export.
//!
//! Unlike the Markdown path, this never errors on image-only pages: a
//! scanned page is a `<picture>` element backed by a `pages/{N}` or
//! `assets/` raster part, exactly what DocLang's `<src>` + `<location>`
//! model is for. Positioned extraction supplies text runs and image
//! placeholders; lopdf supplies page geometry and the image streams the
//! placeholders refer to.

mod images;
mod layout;
mod markup;
mod package;
mod streams;
mod tables;

use crate::error::ConvertError;
use crate::package::limits;
use layout::{Piece, Roles};
use lopdf::content::Content;
use lopdf::{Dictionary, Document as LopdfDocument, Object, ObjectId, Stream};
use package::Part;
use pdf_inspector::TextItem;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Export PDF bytes to a `.dclx` (OPC ZIP) DocLang archive. `hints` steer
/// structure detection (lists; tagged and ruled tables).
pub(crate) fn to_doclang(
    bytes: &[u8],
    hints: crate::DoclangHints,
) -> Result<Vec<u8>, ConvertError> {
    // The bytes are parsed twice at most: pdf-inspector's text extractor
    // takes a buffer (its public API offers no `&Document` variant), and
    // everything else — geometry, structure roles, tagged tables, title —
    // works off this single lopdf document.
    let items = pdf_inspector::extract_text_with_positions_mem(bytes)
        .map_err(crate::formats::pdf::map_error)?;
    let doc = LopdfDocument::load_mem(bytes)
        .map_err(|e| ConvertError::malformed(format!("cannot parse PDF objects: {e}")))?;
    let pages = doc.get_pages();

    let tree = pdf_inspector::structure_tree::StructTree::from_doc(&doc);
    let roles: Roles = tree
        .as_ref()
        .map(|t| {
            t.mcid_to_roles(&pages)
                .into_iter()
                .flat_map(|(page, mcids)| {
                    mcids.into_iter().map(move |(mcid, r)| ((page, mcid), r.name().to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    let tagged = !roles.is_empty();

    // Tagged tables: `/Table`/`TR`/`TD` elements from the structure tree,
    // matched to items by mcid. Independent of `pages_with_tables`, which
    // only scopes the geometric ruled-table detector.
    let tagged_tables = if hints.tables {
        tree.as_ref().map(|t| t.extract_tables(&pages)).unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut items_by_page: HashMap<u32, Vec<TextItem>> = HashMap::new();
    for item in items {
        items_by_page.entry(item.page).or_default().push(item);
    }

    let mut bodies: Vec<String> = Vec::with_capacity(pages.len());
    let mut page_sizes: Vec<(f32, f32)> = Vec::with_capacity(pages.len());
    let mut rasters = RasterStore::default();

    for (page_no, page_id) in &pages {
        let geom = page_geometry(&doc, *page_id);
        page_sizes.push((geom.x1 - geom.x0, geom.y1 - geom.y0));

        let page_items = items_by_page.remove(page_no).unwrap_or_default();
        let links = layout::link_rects(&page_items);
        // Page resources + decoded content are shared by the rule walk
        // and the image walk so the page stream is decoded once.
        let resources = tables::page_resources(&doc, *page_id);
        let content = page_content(&doc, *page_id);
        let ruled = hints.tables
            && hints.pages_with_tables.as_deref().is_none_or(|pp| pp.contains(page_no));
        // Form XObjects decode once per page for both walks.
        let mut forms = FormCache::default();
        let rules = if ruled {
            content
                .as_ref()
                .map_or_else(Vec::new, |c| tables::page_rules(&doc, &resources, c, &mut forms))
        } else {
            Vec::new()
        };
        let table_ctx =
            tables::TableCtx { tagged: &tagged_tables, rules: &rules, ruled, page: *page_no };
        let mut pieces = layout::layout(page_items, &roles, tagged, &hints, &table_ctx);
        mark_page_pictures(&mut pieces, geom_area(&geom));

        let placed = images::placed_images(&doc, &resources, content.as_ref(), &mut forms);
        let mut used = vec![false; placed.len()];
        let mut page_slot_free = true;
        // XObject names are only unique within one resources dict —
        // nested Forms can each draw an `Im1`. Pictures that share a
        // name get occurrence-qualified keys so each picks up its own
        // matched raster instead of the last one written.
        let mut name_counts: HashMap<String, usize> = HashMap::new();
        for piece in &pieces {
            if let Piece::Picture { name, .. } = piece {
                *name_counts.entry(name.clone()).or_default() += 1;
            }
        }
        let mut occurrences: HashMap<String, usize> = HashMap::new();
        let mut srcs: HashMap<String, String> = HashMap::new();
        for piece in &mut pieces {
            let Piece::Picture { item: _, name, full_page } = piece else { continue };
            let orig = name.clone();
            let Some(idx) = placed
                .iter()
                .enumerate()
                .position(|(i, p)| !used[i] && p.name == orig)
                .or_else(|| used.iter().position(|u| !*u))
            else {
                continue;
            };
            used[idx] = true;
            if let Some(path) =
                rasters.part(&doc, placed[idx].id, *page_no, *full_page, &mut page_slot_free)?
            {
                if name_counts[&orig] > 1 {
                    let occ = occurrences.entry(orig.clone()).or_default();
                    *name = format!("{orig}\u{1}{occ}");
                    *occ += 1;
                }
                srcs.insert(name.clone(), path);
            }
        }

        let mut body = String::new();
        markup::emit_page(&mut body, &pieces, &geom, &links, &srcs);
        bodies.push(body);
    }

    let meta = markup::DocMeta {
        title: document_title(&doc),
        sha256: hex(&Sha256::digest(bytes)),
        page_sizes,
    };
    let document_xml = markup::emit_document(&meta, &bodies);
    package::pack(document_xml.as_bytes(), &rasters.parts, Some(bytes))
}

/// Raster allocation for the archive: deduplicates XObjects into parts and
/// bounds their count and total bytes like archive reads.
#[derive(Default)]
struct RasterStore {
    paths: HashMap<ObjectId, String>,
    parts: Vec<Part>,
    total: u64,
}

impl RasterStore {
    /// Convert one image XObject to a stored raster part (deduplicated by
    /// object id) and return its archive URI.
    fn part(
        &mut self,
        doc: &LopdfDocument,
        id: ObjectId,
        page_no: u32,
        full_page: bool,
        page_slot_free: &mut bool,
    ) -> Result<Option<String>, ConvertError> {
        if let Some(path) = self.paths.get(&id) {
            return Ok(Some(path.clone()));
        }
        let Some(raster) = images::raster(doc, id) else { return Ok(None) };
        // Reserve four entries for the OPC plumbing + `/source` the pack
        // step always adds, or the emitted archive exceeds the read cap.
        if self.parts.len() >= limits::MAX_ENTRY_COUNT - 4 {
            return Err(ConvertError::ResourceLimit {
                limit: "max_entry_count",
                detail: format!("DocLang archive would exceed {} parts", limits::MAX_ENTRY_COUNT),
            });
        }
        self.total += raster.bytes.len() as u64;
        if self.total > limits::MAX_ASSET_TOTAL_BYTES as u64 {
            return Err(ConvertError::ResourceLimit {
                limit: "max_asset_total_bytes",
                detail: format!("raster parts exceed {} bytes", limits::MAX_ASSET_TOTAL_BYTES),
            });
        }
        // `pages/` is reserved for whole-page review rasters — one slot per
        // page, web formats only; anything else lives under `assets/`.
        let in_pages = full_page && *page_slot_free && matches!(raster.ext, "png" | "jpg" | "webp");
        if in_pages {
            *page_slot_free = false;
        }
        let path = if in_pages {
            format!("pages/{page_no}.{}", raster.ext)
        } else {
            format!("assets/img{}.{}", self.parts.len(), raster.ext)
        };
        self.paths.insert(id, path.clone());
        self.parts.push(Part { path: path.clone(), bytes: raster.bytes });
        Ok(Some(path))
    }
}

/// Flag the picture that stands in for the page: any picture covering
/// ≥75% of the crop box, or — on image-only (scanned) pages — the largest
/// one, which is what a page raster semantically is.
fn mark_page_pictures(pieces: &mut [Piece], page_area: f32) {
    let image_only =
        !pieces.is_empty() && pieces.iter().all(|p| matches!(p, Piece::Picture { .. }));
    let largest = pieces
        .iter()
        .filter_map(|p| match p {
            Piece::Picture { item, .. } => Some(item.width * item.height),
            _ => None,
        })
        .fold(0.0_f32, f32::max);
    for piece in pieces {
        if let Piece::Picture { item, full_page, .. } = piece {
            let area = item.width * item.height;
            *full_page = area >= 0.75 * page_area || (image_only && area >= largest);
        }
    }
}

fn geom_area(geom: &markup::PageGeom) -> f32 {
    ((geom.x1 - geom.x0) * (geom.y1 - geom.y0)).max(0.0)
}

/// `/Info /Title`, honoring the UTF-16BE BOM convention and decoding the
/// fallback as PDFDocEncoding (Latin-1 superset — high bytes like `é`
/// survive); `None` when the entry is absent or not a string.
fn document_title(doc: &LopdfDocument) -> Option<String> {
    let info_ref = doc.trailer.get(b"Info").ok()?.as_reference().ok()?;
    let info = doc.get_dictionary(info_ref).ok()?;
    let Object::String(bytes, _) = info.get(b"Title").ok()? else { return None };
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let (pairs, _) = bytes[2..].as_chunks::<2>();
        let utf16: Vec<u16> = pairs.iter().map(|c| u16::from_be_bytes(*c)).collect();
        Some(String::from_utf16_lossy(&utf16))
    } else {
        Some(bytes.iter().map(|&b| b as char).collect())
    }
}

/// The page's concatenated content streams, each bounded-decoded. A
/// stream past `MAX_ENTRY_BYTES` (or with an unsupported filter) falls
/// back to its raw bytes, as `get_page_content` does — malformed or
/// oversized streams degrade the walk rather than abort the page.
fn page_content(doc: &LopdfDocument, page_id: ObjectId) -> Option<Content> {
    let mut bytes = Vec::new();
    let mut any = false;
    for id in doc.get_page_contents(page_id) {
        let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else { continue };
        let data = streams::decompressed(doc, stream, limits::MAX_ENTRY_BYTES)
            .unwrap_or_else(|| stream.content.clone());
        bytes.extend_from_slice(&data);
        bytes.push(b'\n');
        any = true;
    }
    if !any {
        return None;
    }
    Content::decode(&bytes).ok()
}

/// Decoded Form-XObject contents shared by the rule and image walks:
/// each form stream is decompressed and decoded once per page no matter
/// how many `Do` names (or which walk) reach it.
#[derive(Default)]
pub(crate) struct FormCache {
    map: HashMap<ObjectId, Option<Rc<Content>>>,
}

impl FormCache {
    pub(crate) fn content(
        &mut self,
        doc: &LopdfDocument,
        id: ObjectId,
        stream: &Stream,
    ) -> Option<Rc<Content>> {
        self.map
            .entry(id)
            .or_insert_with(|| {
                streams::decompressed(doc, stream, limits::MAX_ENTRY_BYTES)
                    .and_then(|b| Content::decode(&b).ok())
                    .map(Rc::new)
            })
            .clone()
    }
}

/// Crop box and rotation of a page, resolving `CropBox`/`MediaBox`/`Rotate`
/// inheritance up the `/Parent` chain.
fn page_geometry(doc: &LopdfDocument, page_id: ObjectId) -> markup::PageGeom {
    let mut crop = None;
    let mut media = None;
    let mut rotate: Option<i64> = None;
    let mut id = Some(page_id);
    let mut seen = HashSet::new();
    while let Some(pid) = id {
        if !seen.insert(pid) {
            break;
        }
        let Ok(dict) = doc.get_dictionary(pid) else { break };
        if crop.is_none() {
            crop = get_box(doc, dict, b"CropBox");
        }
        if media.is_none() {
            media = get_box(doc, dict, b"MediaBox");
        }
        // An explicit `/Rotate 0` overrides a rotated ancestor — track
        // whether the key was found, not just its value.
        if rotate.is_none() {
            rotate = dict.get(b"Rotate").ok().and_then(|o| o.as_i64().ok());
        }
        id = dict.get(b"Parent").ok().and_then(|o| o.as_reference().ok());
    }
    let [x0, y0, x1, y1] = crop.or(media).unwrap_or([0.0, 0.0, 612.0, 792.0]);
    markup::PageGeom { x0, y0, x1, y1, rotated: rotate.unwrap_or(0) % 360 != 0 }
}

/// A page box from a page dict, resolving an indirect array.
fn get_box(doc: &LopdfDocument, dict: &Dictionary, key: &[u8]) -> Option<[f32; 4]> {
    let obj = dict.get(key).ok()?;
    as_rect(obj).or_else(|| {
        obj.as_reference().ok().and_then(|id| doc.get_object(id).ok()).and_then(as_rect)
    })
}

fn as_rect(obj: &Object) -> Option<[f32; 4]> {
    let arr = obj.as_array().ok()?;
    if arr.len() != 4 {
        return None;
    }
    let mut out = [0.0f32; 4];
    for (i, o) in arr.iter().enumerate() {
        out[i] = match o {
            Object::Integer(n) => *n as f32,
            Object::Real(n) => *n,
            _ => return None,
        };
    }
    Some(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
