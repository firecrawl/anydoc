//! Embedded image extraction for DocLang export.
//!
//! Enumerates the image XObjects a page draws, in `Do`-operator order
//! (following Form XObjects), and repackages their streams into payloads a
//! `.dclx` archive can carry directly: DCT/JPX streams pass through
//! unchanged, decoded Flate/LZW samples wrap into PNG, and CCITT fax data
//! wraps into a minimal TIFF container. Anything else (JBIG2, unsupported
//! color spaces) yields no raster — the `<picture>` still keeps its
//! bounding box.

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::HashSet;
use std::io::Write;

use super::FormCache;
use super::streams;
use crate::package::limits;

/// An image XObject a page draws, in content-stream order.
pub struct PlacedImage {
    /// The resource name the content stream uses (`Im1`); matches the name
    /// pdf-inspector embeds in `[Image: …]` placeholder items.
    pub name: String,
    /// The image stream's object id.
    pub id: ObjectId,
}

/// An archive-usable raster payload.
pub struct Raster {
    /// File extension without the dot (`png`, `jpg`, `jp2`, `tif`).
    pub ext: &'static str,
    /// Ready-to-store bytes.
    pub bytes: Vec<u8>,
}

/// Every image a page's content streams draw, in draw order. Form XObjects
/// are walked recursively with a depth cap and a per-path visited set
/// (malformed documents can cycle them); images and forms reused on the
/// page appear once per draw. The caller supplies the page's resources
/// and decoded content so the page stream is decoded once for both this
/// walk and the rule walk; `cache` shares decoded Form contents between
/// them.
pub fn placed_images(
    doc: &Document,
    resources: &[Dictionary],
    content: Option<&Content>,
    cache: &mut FormCache,
) -> Vec<PlacedImage> {
    let mut out = Vec::new();
    let Some(content) = content else { return out };
    let mut seen = HashSet::new();
    collect_drawn_images(doc, resources, content, &mut out, &mut seen, 0, cache);
    out
}

fn collect_drawn_images(
    doc: &Document,
    resources: &[Dictionary],
    content: &Content,
    out: &mut Vec<PlacedImage>,
    seen: &mut HashSet<ObjectId>,
    depth: usize,
    cache: &mut FormCache,
) {
    if depth > 8 {
        return;
    }
    for op in &content.operations {
        if op.operator != "Do" {
            continue;
        }
        let Some(name) = op.operands.first().and_then(|o| o.as_name().ok()) else {
            continue;
        };
        let Some(id) = resolve_xobject(doc, resources, name) else { continue };
        let Ok(stream) = doc.get_object(id).and_then(Object::as_stream) else { continue };
        match stream.dict.get(b"Subtype").and_then(Object::as_name) {
            Ok(b"Image") => {
                // Every drawn occurrence gets an entry; the same object may
                // be drawn more than once. Parts are deduplicated upstream.
                out.push(PlacedImage { name: String::from_utf8_lossy(name).to_string(), id })
            }
            Ok(b"Form") => {
                // `seen` tracks the current path only: cycles stop here, but
                // a Form reused elsewhere on the page still expands again.
                if !seen.insert(id) {
                    continue;
                }
                let mut form_resources = resources.to_vec();
                match stream.dict.get(b"Resources") {
                    Ok(Object::Dictionary(res)) => form_resources.insert(0, res.clone()),
                    Ok(Object::Reference(id)) => {
                        if let Ok(res) = doc.get_dictionary(*id) {
                            form_resources.insert(0, res.clone());
                        }
                    }
                    _ => {}
                }
                if let Some(inner) = cache.content(id, stream) {
                    collect_drawn_images(doc, &form_resources, &inner, out, seen, depth + 1, cache);
                }
                seen.remove(&id);
            }
            _ => {}
        }
    }
}

fn resolve_xobject(doc: &Document, resources: &[Dictionary], name: &[u8]) -> Option<ObjectId> {
    for dict in resources {
        let Ok(xobjects) = dict.get(b"XObject") else { continue };
        let xobjects = match xobjects {
            Object::Reference(id) => doc.get_dictionary(*id).ok()?,
            Object::Dictionary(d) => d,
            _ => continue,
        };
        let Ok(entry) = xobjects.get(name) else { continue };
        // Inline stream XObjects carry no id; they are not archive parts.
        return entry.as_reference().ok();
    }
    None
}

/// Repackage the image stream `id` into an archive-usable raster payload.
/// `None` when the encoding cannot be carried as-is or decoded.
pub fn raster(doc: &Document, id: ObjectId) -> Option<Raster> {
    let stream = doc.get_object(id).and_then(Object::as_stream).ok()?;
    let filters: Vec<Vec<u8>> =
        stream.filters().unwrap_or_default().into_iter().map(|f| f.to_vec()).collect();

    // A trailing image-codec filter can keep its coded bytes when every
    // filter before it is a plain byte decoder (ASCII85, Flate, LZW): the
    // codec output is itself a file (JPEG, JPEG 2000) or a codestream that
    // wraps into one (CCITT fax).
    let (last, prefix) = filters.split_last().map(|(l, p)| (l.as_slice(), p)).unwrap_or((b"", &[]));
    let prefix_decodable = prefix
        .iter()
        .all(|f| matches!(f.as_slice(), b"FlateDecode" | b"LZWDecode" | b"ASCII85Decode"));

    match last {
        b"DCTDecode" if prefix_decodable => {
            decode_prefix(doc, stream, prefix.len()).map(|bytes| Raster { ext: "jpg", bytes })
        }
        b"JPXDecode" if prefix_decodable => {
            decode_prefix(doc, stream, prefix.len()).map(|bytes| Raster { ext: "jp2", bytes })
        }
        b"CCITTFaxDecode" if prefix_decodable => {
            let params = decode_parms(doc, stream, filters.len() - 1);
            let height =
                stream.dict.get(b"Height").ok().and_then(|o| o.as_i64().ok()).unwrap_or(0).max(0)
                    as u32;
            decode_prefix(doc, stream, prefix.len())
                .and_then(|coded| ccitt_to_tiff(&coded, params.as_ref(), height))
                .map(|bytes| Raster { ext: "tif", bytes })
        }
        _ => {
            // Every filter must be a byte decoder for the PNG path; an empty
            // filter list means raw samples.
            if !filters
                .iter()
                .all(|f| matches!(f.as_slice(), b"FlateDecode" | b"LZWDecode" | b"ASCII85Decode"))
            {
                return None;
            }
            // Bound the decode by the declared dimensions before any
            // decompression runs — a Flate stream cannot expand past it.
            let bound = pixel_bound(&stream.dict)?;
            let raw = streams::decompressed(stream, bound)?;
            image_to_png(doc, &stream.dict, &raw).map(|bytes| Raster { ext: "png", bytes })
        }
    }
}

/// Upper bound on decoded sample bytes, derived from the declared
/// dimensions: at most 4 components × 16 bits = 8 B/px, plus one
/// predictor filter byte per row and a little slack. `None` when the
/// dimensions are missing or exceed `MAX_PIXELS`.
fn pixel_bound(dict: &Dictionary) -> Option<u64> {
    let w = dict.get(b"Width").ok()?.as_i64().ok()?;
    let h = dict.get(b"Height").ok()?.as_i64().ok()?;
    if w <= 0 || h <= 0 || w as u64 * h as u64 > MAX_PIXELS {
        return None;
    }
    Some(((w as u64) * (h as u64) * 8 + h as u64 + 4_096).min(limits::MAX_ENTRY_BYTES))
}

/// Decode all but the last filter by masking it out of the stream dict, so
/// lopdf's own decompressor handles the decodable prefix.
fn decode_prefix(doc: &Document, stream: &Stream, kept: usize) -> Option<Vec<u8>> {
    if kept == 0 {
        return Some(stream.content.clone());
    }
    let mut stripped = stream.clone();
    truncate_list(doc, &mut stripped.dict, b"Filter", kept)?;
    truncate_list(doc, &mut stripped.dict, b"DecodeParms", kept)?;
    streams::decompressed(&stripped, limits::MAX_ENTRY_BYTES)
}

/// Keep the first `kept` entries of a stream-dict filter/params list. An
/// absent key or a single non-array value is left untouched (a lone `Name`
/// filter never reaches here — `kept` is zero for it); an indirect array is
/// resolved, truncated, and inlined back.
fn truncate_list(doc: &Document, dict: &mut Dictionary, key: &[u8], kept: usize) -> Option<()> {
    let resolved = match dict.get(key) {
        Err(_) => return Some(()),
        Ok(Object::Reference(id)) => doc.get_object(*id).ok()?.clone(),
        Ok(obj) => obj.clone(),
    };
    if let Object::Array(mut a) = resolved {
        a.truncate(kept);
        dict.set(key, Object::Array(a));
    }
    Some(())
}

/// The `/DecodeParms` dict for filter `index`, resolved through references
/// (`DecodeParms` may be a single dict or an array paralleling the filter
/// list, and either may be indirect).
fn decode_parms(doc: &Document, stream: &Stream, index: usize) -> Option<Dictionary> {
    let resolved = match stream.dict.get(b"DecodeParms") {
        Ok(Object::Reference(id)) => doc.get_object(*id).ok()?.clone(),
        Ok(o) => o.clone(),
        Err(_) => return None,
    };
    match resolved {
        Object::Dictionary(d) => Some(d),
        Object::Array(a) => match a.get(index) {
            Some(Object::Reference(id)) => doc.get_object(*id).ok()?.as_dict().ok().cloned(),
            Some(o) => o.as_dict().ok().cloned(),
            None => None,
        },
        _ => None,
    }
}

enum Kind {
    Gray,
    Rgb,
    Cmyk,
}

impl Kind {
    fn components(&self) -> usize {
        match self {
            Kind::Gray => 1,
            Kind::Rgb => 3,
            Kind::Cmyk => 4,
        }
    }
}

/// Resolve `/ColorSpace` to a channel model plus, for `/Indexed`, the
/// palette as base-space samples.
fn color_space(doc: &Document, obj: Option<&Object>) -> Option<(Kind, Option<Vec<u8>>)> {
    match obj? {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"CalGray" | b"G" => Some((Kind::Gray, None)),
            b"DeviceRGB" | b"CalRGB" | b"RGB" => Some((Kind::Rgb, None)),
            b"DeviceCMYK" | b"CMYK" => Some((Kind::Cmyk, None)),
            _ => None,
        },
        Object::Reference(id) => color_space(doc, doc.get_object(*id).ok()),
        Object::Array(a) => {
            match a.first().and_then(|o| o.as_name().ok())? {
                b"ICCBased" => {
                    let id = a.get(1).and_then(|o| o.as_reference().ok())?;
                    let stream = doc.get_object(id).and_then(Object::as_stream).ok()?;
                    match stream.dict.get(b"N").ok().and_then(|o| o.as_i64().ok()) {
                        Some(1) | None => Some((Kind::Gray, None)),
                        Some(3) => Some((Kind::Rgb, None)),
                        Some(4) => Some((Kind::Cmyk, None)),
                        Some(_) => None,
                    }
                }
                b"Indexed" | b"I" => {
                    let (base, _) = color_space(doc, a.get(1))?;
                    let palette = match a.get(3)? {
                        Object::String(s, _) => s.clone(),
                        Object::Reference(id) => {
                            let stream = doc.get_object(*id).and_then(Object::as_stream).ok()?;
                            // A palette is at most a few KiB of base-space
                            // samples; cap the decode accordingly.
                            streams::decompressed(stream, 65_536)?
                        }
                        _ => return None,
                    };
                    if palette.len() < base.components() {
                        return None;
                    }
                    Some((base, Some(palette)))
                }
                // Separation/DeviceN/Pattern need tint or tiling
                // transforms we do not apply — no raster over wrong colors.
                b"Separation" | b"DeviceN" | b"Pattern" => None,
                _ => None,
            }
        }
        _ => None,
    }
}

/// Decoded-pixel cap: 32M pixels is 128 MiB of RGBA, matching the
/// archive asset cap.
const MAX_PIXELS: u64 = 32_000_000;

/// PNG-encode decoded image samples, resolving the color space and an
/// optional soft mask.
fn image_to_png(doc: &Document, dict: &Dictionary, raw: &[u8]) -> Option<Vec<u8>> {
    let w = dict.get(b"Width").ok()?.as_i64().ok()? as u32;
    let h = dict.get(b"Height").ok()?.as_i64().ok()? as u32;
    // Bound decoded memory before allocation.
    if w == 0 || h == 0 || w as u64 * h as u64 > MAX_PIXELS {
        return None;
    }
    let bpc = dict.get(b"BitsPerComponent").ok().and_then(|o| o.as_i64().ok()).unwrap_or(8);
    let (kind, palette) = color_space(doc, dict.get(b"ColorSpace").ok())?;
    let decode = decode_ranges(dict);

    let samples = match palette {
        Some(palette) => {
            let mut indices = unpack_indices(raw, w, h, bpc)?;
            if let Some(ranges) = &decode {
                // For Indexed the Decode pair remaps the index itself,
                // in index units (0..2^bpc−1).
                let (dmin, dmax) = ranges[0];
                let max = ((1u32 << bpc.max(1)) - 1) as f32;
                for i in &mut indices {
                    *i = (dmin + (*i as f32 / max) * (dmax - dmin)).round().clamp(0.0, max) as u8;
                }
            }
            let comps = kind.components();
            let mut out = Vec::with_capacity((w * h) as usize * comps);
            for idx in indices {
                let off = idx as usize * comps;
                if off + comps <= palette.len() {
                    out.extend_from_slice(&palette[off..off + comps]);
                } else {
                    out.resize(out.len() + comps, 0);
                }
            }
            out
        }
        None => {
            let mut samples = expand_bpc(raw, w, h, bpc, kind.components())?;
            if let Some(ranges) = &decode {
                apply_decode(&mut samples, ranges, kind.components());
            }
            samples
        }
    };

    let (mut color_type, mut pixels) = match kind {
        Kind::Gray => (0u8, samples),
        Kind::Rgb => (2u8, samples),
        Kind::Cmyk => (2u8, cmyk_to_rgb(&samples)),
    };
    if let Some(alpha) = soft_mask_alpha(doc, dict, w, h) {
        match color_type {
            0 => {
                color_type = 4;
                pixels = interleave_alpha(&pixels, &alpha, 1);
            }
            _ => {
                color_type = 6;
                pixels = interleave_alpha(&pixels, &alpha, 3);
            }
        }
    }
    png(w, h, color_type, &pixels)
}

/// Expand 8/16-bit and packed 1/2/4-bit samples to 8-bit per component,
/// scaling sub-byte values to the full range. Row stride is byte-aligned.
fn expand_bpc(raw: &[u8], w: u32, h: u32, bpc: i64, comps: usize) -> Option<Vec<u8>> {
    match bpc {
        8 => {
            let need = (w * h) as usize * comps;
            (raw.len() >= need).then(|| raw[..need].to_vec())
        }
        16 => {
            let need = (w * h) as usize * comps * 2;
            (raw.len() >= need).then(|| raw[..need].iter().step_by(2).copied().collect())
        }
        1 | 2 | 4 if comps == 1 => unpack_indices(raw, w, h, bpc).map(|v| {
            let max = (1u32 << bpc) - 1;
            v.into_iter().map(|i| (i as u32 * 255 / max) as u8).collect()
        }),
        _ => None,
    }
}

/// Unpack packed sub-byte samples into per-pixel index values.
fn unpack_indices(raw: &[u8], w: u32, h: u32, bpc: i64) -> Option<Vec<u8>> {
    let bpc = bpc as usize;
    if !(1..=8).contains(&bpc) {
        return None;
    }
    let stride = (w as usize * bpc).div_ceil(8);
    if raw.len() < stride * h as usize {
        return None;
    }
    let mask = (1u8 << bpc) - 1;
    let mut out = Vec::with_capacity((w * h) as usize);
    for row in 0..h as usize {
        let mut produced = 0usize;
        'row: for &byte in &raw[row * stride..row * stride + stride] {
            for off in (0..8).step_by(bpc) {
                if produced >= w as usize {
                    break 'row;
                }
                out.push((byte >> (8 - bpc - off)) & mask);
                produced += 1;
            }
        }
    }
    Some(out)
}

fn cmyk_to_rgb(cmyk: &[u8]) -> Vec<u8> {
    cmyk.chunks(4)
        .flat_map(|c| {
            let k = c[3] as u16;
            [
                (255u16 - (c[0] as u16 + k).min(255)) as u8,
                (255u16 - (c[1] as u16 + k).min(255)) as u8,
                (255u16 - (c[2] as u16 + k).min(255)) as u8,
            ]
        })
        .collect()
}

/// `/Decode` min/max pairs (`None` when the key is absent — the default
/// mapping is then the identity for every color space we emit).
fn decode_ranges(dict: &Dictionary) -> Option<Vec<(f32, f32)>> {
    let Object::Array(a) = dict.get(b"Decode").ok()? else { return None };
    let mut ranges = Vec::with_capacity(a.len() / 2);
    for pair in a.as_chunks::<2>().0 {
        ranges.push((as_f32(&pair[0])?, as_f32(&pair[1])?));
    }
    (!ranges.is_empty()).then_some(ranges)
}

fn as_f32(o: &Object) -> Option<f32> {
    match o {
        Object::Real(v) => Some(*v),
        Object::Integer(v) => Some(*v as f32),
        _ => None,
    }
}

/// Apply per-component `/Decode` ranges to 8-bit samples:
/// `out = dmin + s/255·(dmax − dmin)`, clamped back to a byte.
fn apply_decode(samples: &mut [u8], ranges: &[(f32, f32)], comps: usize) {
    for (c, &(dmin, dmax)) in ranges.iter().enumerate().take(comps) {
        for s in samples.iter_mut().skip(c).step_by(comps) {
            let v = dmin + (*s as f32 / 255.0) * (dmax - dmin);
            *s = (v * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Decode the `/SMask` soft mask to per-pixel alpha, if present and usable.
fn soft_mask_alpha(doc: &Document, dict: &Dictionary, w: u32, h: u32) -> Option<Vec<u8>> {
    let id = dict.get(b"SMask").ok()?.as_reference().ok()?;
    let stream = doc.get_object(id).and_then(Object::as_stream).ok()?;
    let bound = ((w as u64) * (h as u64) * 2 + h as u64 + 4_096).min(limits::MAX_ENTRY_BYTES);
    let raw = streams::decompressed(stream, bound)?;
    let bpc = stream.dict.get(b"BitsPerComponent").ok().and_then(|o| o.as_i64().ok()).unwrap_or(8);
    let mut alpha = expand_bpc(&raw, w, h, bpc, 1)?;
    if let Some(ranges) = decode_ranges(&stream.dict) {
        apply_decode(&mut alpha, &ranges, 1);
    }
    Some(alpha)
}

fn interleave_alpha(pixels: &[u8], alpha: &[u8], comps: usize) -> Vec<u8> {
    pixels
        .chunks(comps)
        .zip(alpha.iter())
        .flat_map(|(px, &a)| px.iter().copied().chain([a]))
        .collect()
}

/// Minimal PNG encoder: gray/RGB(+alpha) at 8-bit, filter-0 rows.
fn png(w: u32, h: u32, color_type: u8, pixels: &[u8]) -> Option<Vec<u8>> {
    let bpp = match color_type {
        0 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return None,
    };
    let stride = w as usize * bpp;
    if pixels.len() < stride * h as usize {
        return None;
    }
    let mut scan = Vec::with_capacity((stride + 1) * h as usize);
    for row in 0..h as usize {
        scan.push(0);
        scan.extend_from_slice(&pixels[row * stride..row * stride + stride]);
    }
    let mut idat = Vec::new();
    flate2::write::ZlibEncoder::new(&mut idat, flate2::Compression::fast())
        .write_all(&scan)
        .ok()?;

    let mut out = Vec::with_capacity(8 + 25 + idat.len() + 12);
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, color_type, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);
    Some(out)
}

fn chunk(out: &mut Vec<u8>, name: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(data);
    let crc = crc32(&[name.as_slice(), data].concat());
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Wrap raw CCITT fax data in a minimal single-strip TIFF so any image
/// reader can decode it. `image_height` fills in when the optional
/// `/DecodeParms /Rows` is absent — the XObject `/Height` is reliable.
fn ccitt_to_tiff(coded: &[u8], params: Option<&Dictionary>, image_height: u32) -> Option<Vec<u8>> {
    let get = |key: &[u8], default: i64| {
        params.and_then(|p| p.get(key).ok()).and_then(|o| o.as_i64().ok()).unwrap_or(default)
    };
    let k = get(b"K", 0);
    let width = get(b"Columns", 1728).max(1) as u32;
    let mut height = get(b"Rows", 0).max(0) as u32;
    if height == 0 {
        height = image_height;
    }
    if height == 0 {
        // A stream whose row count is left to the decoder cannot be framed
        // as a single-strip TIFF without decoding the data.
        return None;
    }
    let black_is_1 = params
        .and_then(|p| p.get(b"BlackIs1").ok())
        .and_then(|o| o.as_bool().ok())
        .unwrap_or(false);

    // Photometric: PDF BlackIs1 means bit 1 = black ⇒ bit 0 is white
    // (WhiteIsZero = 0); the default (0 = black) maps to BlackIsZero = 1.
    let photometric: u16 = if black_is_1 { 0 } else { 1 };
    let (compression, t4_options): (u16, u32) =
        if k < 0 { (4, 0) } else { (3, if k > 0 { 1 } else { 0 }) };

    let mut entries: Vec<(u16, u16, u32, u32)> = vec![
        (256, 4, 1, width),              // ImageWidth, LONG
        (257, 4, 1, height),             // ImageLength, LONG
        (258, 3, 1, 1),                  // BitsPerSample, SHORT
        (259, 3, 1, compression as u32), // Compression, SHORT
        (262, 3, 1, photometric as u32), // PhotometricInterpretation, SHORT
        (273, 4, 1, 0),                  // StripOffsets, patched below
        (277, 3, 1, 1),                  // SamplesPerPixel, SHORT
        (278, 4, 1, height),             // RowsPerStrip, LONG
        (279, 4, 1, coded.len() as u32), // StripByteCounts, LONG
    ];
    if compression == 3 {
        entries.push((292, 4, 1, t4_options)); // T4Options
    } else {
        entries.push((293, 4, 1, 0)); // T6Options
    }
    entries.sort_by_key(|e| e.0);

    let data_offset = (8 + 2 + entries.len() * 12 + 4) as u32;
    for e in &mut entries {
        if e.0 == 273 {
            e.3 = data_offset;
        }
    }

    let mut out = Vec::with_capacity(data_offset as usize + coded.len());
    out.extend_from_slice(b"II\x2a\x00");
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, ty, count, value) in entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&ty.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&0u32.to_le_bytes()); // next IFD
    out.extend_from_slice(coded);
    Some(out)
}

/// CRC-32 (IEEE) for PNG chunks.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trips_gray() {
        let bytes = png(2, 1, 0, &[0, 255]).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert!(bytes.windows(4).any(|w| w == b"IDAT"));
    }

    #[test]
    fn indexed_expands_palette() {
        // 4 pixels of 1-bit indexed color, 2-entry gray palette.
        let mut dict = Dictionary::new();
        dict.set("Width", 4i64);
        dict.set("Height", 1i64);
        dict.set("BitsPerComponent", 1i64);
        dict.set(
            "ColorSpace",
            vec![
                Object::Name(b"Indexed".to_vec()),
                Object::Name(b"DeviceGray".to_vec()),
                Object::Integer(1),
                Object::String(vec![0, 255], lopdf::StringFormat::Literal),
            ],
        );
        let out = image_to_png(&Document::new(), &dict, &[0b1010_0000]).unwrap();
        assert_eq!(&out[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn ccitt_wraps_in_tiff() {
        let mut params = Dictionary::new();
        params.set("Columns", 8i64);
        params.set("Rows", 10i64);
        let tiff = ccitt_to_tiff(&[0xAA; 10], Some(&params), 10).unwrap();
        assert_eq!(&tiff[..4], b"II\x2a\x00");
        // A stream with no declared row count cannot be framed.
        assert!(ccitt_to_tiff(&[0xAA; 10], None, 0).is_none());
        // …unless the image XObject's /Height supplies it.
        assert!(ccitt_to_tiff(&[0xAA; 10], None, 10).is_some());
    }

    #[test]
    fn unpack_indices_reads_subbyte_samples() {
        assert_eq!(unpack_indices(&[0b1010_0000], 4, 1, 1), Some(vec![1, 0, 1, 0]));
        assert_eq!(unpack_indices(&[0b11_00_10_01], 4, 1, 2), Some(vec![3, 0, 2, 1]));
    }

    #[test]
    fn oversized_pixel_count_yields_no_png() {
        let mut dict = Dictionary::new();
        dict.set("Width", 8192i64);
        dict.set("Height", 8192i64);
        dict.set("BitsPerComponent", 8i64);
        dict.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
        // 67M pixels ≈ 200 MiB decoded — refused before allocation.
        assert!(image_to_png(&Document::new(), &dict, &[]).is_none());
    }
}
