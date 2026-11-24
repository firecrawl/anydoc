//! Bounded stream decompression. lopdf's `Stream::decompressed_content`
//! allocates its output without a limit, so a small Flate stream that
//! expands enormously can exhaust memory before any archive limit runs.
//! The byte filters lopdf supports (Flate, LZW, ASCII85) are decoded
//! here against a hard cap, with the same leniency and the same PNG
//! predictors lopdf applies.

use flate2::read::{DeflateDecoder, ZlibDecoder};
use lopdf::{Dictionary, Document, Object, Stream};
use std::io::{Read, Write};

/// Decode `stream` like `decompressed_content`, refusing output larger
/// than `limit` bytes. `None` on unsupported filters or an over-limit
/// stream — callers keep their undecoded fallback (skipped rules,
/// raster-less `<picture>`).
pub(crate) fn decompressed(doc: &Document, stream: &Stream, limit: u64) -> Option<Vec<u8>> {
    let filters = match stream.filters() {
        Ok(f) => f,
        // No `/Filter` key: the content is already decoded.
        Err(_) => {
            return (stream.content.len() as u64 <= limit).then(|| stream.content.clone());
        }
    };
    // An empty `/Filter` array is a valid no-op filter sequence.
    if filters.is_empty() {
        return (stream.content.len() as u64 <= limit).then(|| stream.content.clone());
    }
    // `/DecodeParms` is either one dictionary shared by every filter or
    // an array parallel to `/Filter`; entries may be indirect or null.
    let parms_obj = stream.dict.get(b"DecodeParms").ok();
    let params_at = |i: usize| -> Option<Dictionary> {
        let obj = match parms_obj? {
            Object::Array(a) => a.get(i)?,
            o => o,
        };
        doc.dereference(obj).ok()?.1.as_dict().ok().cloned()
    };
    let mut data = stream.content.as_slice();
    let mut out = Vec::new();
    for (i, f) in filters.iter().enumerate() {
        let params = params_at(i);
        out = match *f {
            b"FlateDecode" => predictor(zlib(data, limit)?, params.as_ref())?,
            b"LZWDecode" => predictor(lzw(data, limit, params.as_ref())?, params.as_ref())?,
            b"ASCII85Decode" => ascii85(data, limit)?,
            _ => return None,
        };
        data = &out;
    }
    Some(out)
}

/// zlib with lopdf's raw-deflate fallback, capped at `limit` bytes.
/// Decode errors keep the partial output, matching `decompress_zlib`.
fn zlib(input: &[u8], limit: u64) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let _ = ZlibDecoder::new(input).take(limit + 1).read_to_end(&mut out);
    if out.is_empty() && input.len() > 2 {
        let _ = DeflateDecoder::new(&input[2..]).take(limit + 1).read_to_end(&mut out);
    }
    (out.len() as u64 <= limit).then_some(out)
}

/// weezl streams into a `Write`; the cap turns the decoder's own write
/// into an error the moment `limit` is exceeded.
fn lzw(input: &[u8], limit: u64, params: Option<&Dictionary>) -> Option<Vec<u8>> {
    use weezl::{BitOrder, decode::Decoder};
    let early_change = params
        .and_then(|p| p.get(b"EarlyChange").ok())
        .and_then(|o| o.as_i64().ok())
        .map(|v| v != 0)
        .unwrap_or(true);
    let mut decoder = if early_change {
        Decoder::with_tiff_size_switch(BitOrder::Msb, 8)
    } else {
        Decoder::new(BitOrder::Msb, 8)
    };
    let mut out = Capped { inner: Vec::new(), left: limit + 1 };
    // Truncated LZW data keeps its partial output, matching lopdf.
    let _ = decoder.into_stream(&mut out).decode_all(input);
    (out.inner.len() as u64 <= limit).then_some(out.inner)
}

/// `Write` sink that fails once `left` bytes have passed through.
struct Capped {
    inner: Vec<u8>,
    left: u64,
}

impl Write for Capped {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.left == 0 {
            return Err(std::io::ErrorKind::OutOfMemory.into());
        }
        let n = self.inner.write(&buf[..buf.len().min(self.left as usize)])?;
        self.left -= n as u64;
        if n < buf.len() {
            return Err(std::io::ErrorKind::OutOfMemory.into());
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// ASCII85, matching lopdf's `decode_ascii85` (lenient EOD handling).
fn ascii85(input: &[u8], limit: u64) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut count = 0;
    let input = if input.len() >= 2 && &input[input.len() - 2..] == b"~>" {
        &input[..input.len() - 2]
    } else {
        input
    };
    let push = |out: &mut Vec<u8>, bytes: &[u8]| -> Option<()> {
        (out.len() as u64 + bytes.len() as u64 <= limit).then(|| out.extend_from_slice(bytes))
    };
    for &ch in input {
        if ch == b'z' {
            if count != 0 {
                return None;
            }
            push(&mut out, &[0; 4])?;
            continue;
        }
        if ch.is_ascii_whitespace() {
            continue;
        }
        if !(b'!'..=b'u').contains(&ch) {
            break;
        }
        buffer = buffer.checked_mul(85)?.checked_add((ch - b'!') as u32)?;
        count += 1;
        if count == 5 {
            push(&mut out, &buffer.to_be_bytes())?;
            buffer = 0;
            count = 0;
        }
    }
    if count > 0 {
        for _ in count..5 {
            buffer = buffer.checked_mul(85)?.checked_add(84)?;
        }
        let bytes = buffer.to_be_bytes();
        push(&mut out, &bytes[..count - 1])?;
    }
    Some(out)
}

/// Predictor post-processing. Absent `/DecodeParms` means predictor 1
/// (no transform) — never a decode failure. TIFF predictor 2 and PNG
/// predictors 10–15 reconstruct rows; anything else passes through.
fn predictor(data: Vec<u8>, params: Option<&Dictionary>) -> Option<Vec<u8>> {
    let Some(params) = params else { return Some(data) };
    let get = |key: &[u8], default: i64| {
        params.get(key).ok().and_then(|o| o.as_i64().ok()).unwrap_or(default)
    };
    let cols = get(b"Columns", 1);
    let colors = get(b"Colors", 1);
    let bits = get(b"BitsPerComponent", 8);
    match get(b"Predictor", 1) {
        2 => tiff_predictor(data, cols, colors, bits),
        10..=15 => png_predictor(data, cols, colors, bits),
        _ => Some(data),
    }
}

/// TIFF predictor 2: each sample is the difference to the sample
/// `Colors` positions earlier in the same row. Only 8- and 16-bit
/// components have byte-aligned samples; other depths are rejected
/// rather than corrupted.
fn tiff_predictor(mut data: Vec<u8>, cols: i64, colors: i64, bits: i64) -> Option<Vec<u8>> {
    if cols < 1 || colors < 1 {
        return None;
    }
    let bpc = match bits {
        8 => 1usize,
        16 => 2,
        _ => return None,
    };
    let comps = usize::try_from(colors).ok()?;
    let offset = comps.checked_mul(bpc)?;
    let row_len = usize::try_from(cols).ok()?.checked_mul(offset)?;
    if !data.len().is_multiple_of(row_len) {
        return None;
    }
    if bpc == 1 {
        for row in data.chunks_exact_mut(row_len) {
            for i in offset..row_len {
                row[i] = row[i].wrapping_add(row[i - offset]);
            }
        }
    } else {
        for row in data.chunks_exact_mut(row_len) {
            for i in (offset..row_len).step_by(2) {
                let prev = u16::from_be_bytes([row[i - offset], row[i - offset + 1]]);
                let v = u16::from_be_bytes([row[i], row[i + 1]]).wrapping_add(prev);
                row[i..i + 2].copy_from_slice(&v.to_be_bytes());
            }
        }
    }
    Some(data)
}

/// PNG predictors 10–15: a per-row filter byte then the row, using
/// `ceil(Colors*BitsPerComponent/8)` bytes per pixel so packed
/// sub-byte components stay aligned. Checked arithmetic rejects rows
/// that cannot fit the decoded input.
fn png_predictor(data: Vec<u8>, cols: i64, colors: i64, bits: i64) -> Option<Vec<u8>> {
    use lopdf::filters::png;
    if data.is_empty() {
        return Some(data);
    }
    if cols < 1 || colors < 1 || bits < 1 {
        return None;
    }
    let colors = usize::try_from(colors).ok()?;
    let bits = usize::try_from(bits).ok()?;
    let bpp = colors.checked_mul(bits)?.div_ceil(8).max(1);
    let row_len = usize::try_from(cols).ok()?.checked_mul(colors)?.checked_mul(bits)?.div_ceil(8);
    // `row_len >= data.len()` also rules out `row_len + 1` overflowing.
    if row_len == 0 || row_len >= data.len() || !data.len().is_multiple_of(row_len + 1) {
        return None;
    }
    let mut previous = vec![0u8; row_len];
    let mut current = vec![0u8; row_len];
    let mut out = Vec::with_capacity(data.len());
    let mut pos = 0;
    while pos < data.len() {
        let filter = png::FilterType::try_from(data[pos]).ok()?;
        pos += 1;
        current.copy_from_slice(&data[pos..pos + row_len]);
        pos += row_len;
        png::decode_row(filter, bpp, &previous, &mut current);
        out.extend_from_slice(&current);
        std::mem::swap(&mut previous, &mut current);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;

    fn doc() -> Document {
        Document::new()
    }

    fn flate_stream(raw: &[u8], params: Option<Object>) -> Stream {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::fast());
        e.write_all(raw).unwrap();
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        if let Some(p) = params {
            dict.set("DecodeParms", p);
        }
        Stream::new(dict, e.finish().unwrap())
    }

    #[test]
    fn flate_without_decodeparms_decodes() {
        // Missing /DecodeParms means predictor 1, not a failure.
        let s = flate_stream(b"BT /F1 12 Tf ET", None);
        assert_eq!(decompressed(&doc(), &s, 1_024).unwrap(), b"BT /F1 12 Tf ET");
    }

    #[test]
    fn empty_filter_array_returns_content() {
        let mut dict = Dictionary::new();
        dict.set("Filter", Object::Array(vec![]));
        let s = Stream::new(dict, b"raw".to_vec());
        assert_eq!(decompressed(&doc(), &s, 1_024).unwrap(), b"raw");
    }

    #[test]
    fn decodeparms_array_selects_per_filter_entry() {
        // Predictor 12 (Up) over 2 columns: second row is a delta.
        let mut params = Dictionary::new();
        params.set("Predictor", 12i64);
        params.set("Columns", 2i64);
        let raw = [0u8, 10, 20, 2, 1, 1]; // rows: none [10 20], up [+1 +1] → [11 21]
        let s = flate_stream(&raw, Some(Object::Array(vec![Object::Dictionary(params)])));
        assert_eq!(decompressed(&doc(), &s, 1_024).unwrap(), vec![10, 20, 11, 21]);
    }

    #[test]
    fn decodeparms_array_null_entry_means_defaults() {
        let s = flate_stream(b"abc", Some(Object::Array(vec![Object::Null])));
        assert_eq!(decompressed(&doc(), &s, 1_024).unwrap(), b"abc");
    }

    #[test]
    fn ascii85_rejects_wrapping_group() {
        // 'uuuuu' overflows a u32 accumulator.
        assert!(ascii85(b"uuuuu", 1_024).is_none());
    }

    #[test]
    fn tiff_predictor2_undoes_horizontal_differencing() {
        // Two rows, 3 px RGB: each sample = previous sample + delta.
        let encoded = [10u8, 20, 30, 1, 1, 1, 2, 2, 2, 5, 4, 3, 1, 1, 1, 1, 1, 1];
        let out = tiff_predictor(encoded.to_vec(), 3, 3, 8).unwrap();
        assert_eq!(&out[..6], &[10, 20, 30, 11, 21, 31]);
        assert_eq!(&out[12..], &[6, 5, 4, 7, 6, 5]);
        // Sub-byte depths are not byte-aligned: reject, don't corrupt.
        assert!(tiff_predictor(vec![0; 8], 4, 1, 4).is_none());
    }

    #[test]
    fn png_predictor_handles_packed_components() {
        // RGB at 4 bits/component over 2 columns: bpp = 2, row = 3
        // bytes. Row 2 uses Sub (1): deltas against the byte x-2.
        let encoded = [0u8, 0x12, 0x34, 0x56, 1, 0x11, 0x11, 0x11];
        let out = png_predictor(encoded.to_vec(), 2, 3, 4).unwrap();
        assert_eq!(out, vec![0x12, 0x34, 0x56, 0x11, 0x11, 0x22]);
    }

    #[test]
    fn png_predictor_rejects_malformed_dimensions() {
        // Column count overflows the checked row-size arithmetic.
        assert!(png_predictor(vec![0; 7], i64::MAX, 3, 8).is_none());
        // 11 bytes cannot tile 9-byte rows (each with a filter byte).
        assert!(png_predictor(vec![0; 11], 3, 3, 8).is_none());
    }
}
