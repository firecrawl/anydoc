//! Bounded stream decompression. lopdf's `Stream::decompressed_content`
//! allocates its output without a limit, so a small Flate stream that
//! expands enormously can exhaust memory before any archive limit runs.
//! The byte filters lopdf supports (Flate, LZW, ASCII85) are decoded
//! here against a hard cap, with the same leniency and the same PNG
//! predictors lopdf applies.

use flate2::read::{DeflateDecoder, ZlibDecoder};
use lopdf::{Dictionary, Stream};
use std::io::{Read, Write};

/// Decode `stream` like `decompressed_content`, refusing output larger
/// than `limit` bytes. `None` on unsupported filters or an over-limit
/// stream — callers keep their undecoded fallback (skipped rules,
/// raster-less `<picture>`).
pub(crate) fn decompressed(stream: &Stream, limit: u64) -> Option<Vec<u8>> {
    let params = stream.dict.get(b"DecodeParms").ok().and_then(|o| o.as_dict().ok());
    let filters = match stream.filters() {
        Ok(f) => f,
        // No `/Filter` key: the content is already decoded.
        Err(_) => {
            return (stream.content.len() as u64 <= limit).then(|| stream.content.clone());
        }
    };
    let mut data = stream.content.as_slice();
    let mut out = Vec::new();
    for f in filters {
        out = match f {
            b"FlateDecode" => predictor(zlib(data, limit)?, params)?,
            b"LZWDecode" => predictor(lzw(data, limit, params)?, params)?,
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
        buffer = buffer.checked_mul(85)? + (ch - b'!') as u32;
        count += 1;
        if count == 5 {
            push(&mut out, &buffer.to_be_bytes())?;
            buffer = 0;
            count = 0;
        }
    }
    if count > 0 {
        for _ in count..5 {
            buffer = buffer.checked_mul(85)? + 84;
        }
        let bytes = buffer.to_be_bytes();
        push(&mut out, &bytes[..count - 1])?;
    }
    Some(out)
}

/// PNG/TIFF predictors, matching lopdf's `decompress_predictor` (only
/// predictors 10–15 are applied; DecodeParms as an array is ignored,
/// also like lopdf).
fn predictor(data: Vec<u8>, params: Option<&Dictionary>) -> Option<Vec<u8>> {
    let params = params?;
    let predictor = params.get(b"Predictor").ok().and_then(|o| o.as_i64().ok()).unwrap_or(1);
    if !(10..=15).contains(&predictor) {
        return Some(data);
    }
    let get = |key: &[u8], default: i64| {
        params.get(key).ok().and_then(|o| o.as_i64().ok()).unwrap_or(default)
    };
    let cols = get(b"Columns", 1).max(1) as usize;
    let colors = get(b"Colors", 1).max(1) as usize;
    let bits = get(b"BitsPerComponent", 8).max(8) as usize;
    lopdf::filters::png::decode_frame(&data, colors * bits / 8, cols).ok()
}
