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

use crate::error::ConvertError;
use layout::{Piece, Roles};
use lopdf::{Document as LopdfDocument, Object, ObjectId};
use package::Part;
use pdf_inspector::TextItem;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

/// Export PDF bytes to a `.dclx` (OPC ZIP) DocLang archive.
pub(crate) fn to_doclang(bytes: &[u8]) -> Result<Vec<u8>, ConvertError> {
    let detection = pdf_inspector::detect_pdf_mem(bytes).map_err(super::map_error)?;
    let items = pdf_inspector::extract_text_with_positions_mem(bytes).map_err(super::map_error)?;
    let roles = structure_roles(bytes);
    let tagged = !roles.is_empty();

    let doc = LopdfDocument::load_mem(bytes)
        .map_err(|e| ConvertError::malformed(format!("cannot parse PDF objects: {e}")))?;
    let pages = doc.get_pages();

    let mut bodies: Vec<String> = Vec::with_capacity(pages.len());
    let mut page_sizes: Vec<(f32, f32)> = Vec::with_capacity(pages.len());
    let mut parts: Vec<Part> = Vec::new();
    let mut raster_paths: HashMap<ObjectId, String> = HashMap::new();

    for (page_no, page_id) in &pages {
        let geom = page_geometry(&doc, *page_id);
        page_sizes.push((geom.x1 - geom.x0, geom.y1 - geom.y0));

        let page_items: Vec<TextItem> =
            items.iter().filter(|i| i.page == *page_no).cloned().collect();
        let links = layout::link_rects(&page_items);
        let mut pieces = layout::layout(page_items, &roles, tagged);
        mark_page_pictures(&mut pieces, geom_area(&geom));

        let placed = images::placed_images(&doc, *page_id);
        let mut used = vec![false; placed.len()];
        let mut page_slot_free = true;
        let mut srcs: HashMap<String, String> = HashMap::new();
        for piece in &pieces {
            let Piece::Picture { item: _, name, full_page } = piece else { continue };
            let Some(idx) = placed
                .iter()
                .enumerate()
                .position(|(i, p)| !used[i] && p.name == *name)
                .or_else(|| used.iter().position(|u| !*u))
            else {
                continue;
            };
            used[idx] = true;
            if let Some(path) = raster_part(
                &doc,
                placed[idx].id,
                *page_no,
                *full_page,
                &mut page_slot_free,
                &mut raster_paths,
                &mut parts,
            ) {
                srcs.insert(name.clone(), path);
            }
        }

        let mut body = String::new();
        markup::emit_page(&mut body, &pieces, &geom, &links, &srcs);
        bodies.push(body);
    }

    let meta =
        markup::DocMeta { title: detection.title, sha256: hex(&Sha256::digest(bytes)), page_sizes };
    let document_xml = markup::emit_document(&meta, &bodies);
    package::pack(document_xml.as_bytes(), &parts, Some(bytes))
}

/// Convert one image XObject to a stored raster part (deduplicated by
/// object id) and return its archive URI.
fn raster_part(
    doc: &LopdfDocument,
    id: ObjectId,
    page_no: u32,
    full_page: bool,
    page_slot_free: &mut bool,
    raster_paths: &mut HashMap<ObjectId, String>,
    parts: &mut Vec<Part>,
) -> Option<String> {
    if let Some(path) = raster_paths.get(&id) {
        return Some(path.clone());
    }
    let raster = images::raster(doc, id)?;
    // `pages/` is reserved for whole-page review rasters — one slot per
    // page, web formats only; anything else lives under `assets/`.
    let in_pages = full_page && *page_slot_free && matches!(raster.ext, "png" | "jpg" | "webp");
    if in_pages {
        *page_slot_free = false;
    }
    let path = if in_pages {
        format!("pages/{page_no}.{}", raster.ext)
    } else {
        format!("assets/img{}.{}", parts.len(), raster.ext)
    };
    raster_paths.insert(id, path.clone());
    parts.push(Part { path: path.clone(), bytes: raster.bytes });
    Some(path)
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

/// `(page, mcid) → role` from the structure tree. Untagged or malformed
/// trees simply yield an empty map (the layout heuristics take over).
fn structure_roles(bytes: &[u8]) -> Roles {
    match pdf_inspector::extract_structure_elements_mem(bytes, None) {
        Ok(elements) => elements.into_iter().map(|e| ((e.page, e.mcid), e.role)).collect(),
        Err(e) => {
            log::debug!("structure tree unavailable ({e:?}); using layout heuristics");
            HashMap::new()
        }
    }
}

/// Crop box and rotation of a page, resolving `CropBox`/`MediaBox`/`Rotate`
/// inheritance up the `/Parent` chain.
fn page_geometry(doc: &LopdfDocument, page_id: ObjectId) -> markup::PageGeom {
    let mut crop = None;
    let mut media = None;
    let mut rotate = 0i64;
    let mut id = Some(page_id);
    let mut seen = HashSet::new();
    while let Some(pid) = id {
        if !seen.insert(pid) {
            break;
        }
        let Ok(dict) = doc.get_dictionary(pid) else { break };
        if crop.is_none() {
            crop = dict.get(b"CropBox").ok().and_then(as_rect);
        }
        if media.is_none() {
            media = dict.get(b"MediaBox").ok().and_then(as_rect);
        }
        if rotate == 0 {
            rotate = dict.get(b"Rotate").ok().and_then(|o| o.as_i64().ok()).unwrap_or(0);
        }
        id = dict.get(b"Parent").ok().and_then(|o| o.as_reference().ok());
    }
    let [x0, y0, x1, y1] = crop.or(media).unwrap_or([0.0, 0.0, 612.0, 792.0]);
    markup::PageGeom { x0, y0, x1, y1, rotated: rotate % 360 != 0 }
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
