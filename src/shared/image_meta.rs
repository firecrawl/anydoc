//! Fork (sznb): header-only image metadata — pixel dimensions and a file
//! extension, read from the first bytes of the payload without decoding.
//!
//! Covers what office documents embed: PNG, JPEG, GIF, BMP, WEBP, TIFF and
//! the Windows metafiles (EMF, placeable WMF). Anything unreadable yields
//! `None`, which callers treat as "size unknown" (the image is kept).

fn u16_le(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as u32)
}

fn u16_be(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 2).map(|s| u16::from_be_bytes([s[0], s[1]]) as u32)
}

fn u32_le(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn u32_be(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn i32_le(b: &[u8], at: usize) -> Option<i64> {
    u32_le(b, at).map(|v| v as i32 as i64)
}

fn i16_le(b: &[u8], at: usize) -> Option<i64> {
    u16_le(b, at).map(|v| v as u16 as i16 as i64)
}

fn u24_le(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 3).map(|s| s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16)
}

/// The image kinds the header sniffer recognizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    /// Portable Network Graphics.
    Png,
    /// JPEG / JFIF / EXIF.
    Jpeg,
    /// GIF 87a / 89a.
    Gif,
    /// Windows bitmap.
    Bmp,
    /// WebP (lossy, lossless or extended).
    Webp,
    /// TIFF, either byte order.
    Tiff,
    /// Enhanced metafile.
    Emf,
    /// Windows metafile (placeable or bare).
    Wmf,
}

impl ImageKind {
    /// The conventional file extension, with its leading dot.
    pub fn ext(self) -> &'static str {
        match self {
            ImageKind::Png => ".png",
            ImageKind::Jpeg => ".jpg",
            ImageKind::Gif => ".gif",
            ImageKind::Bmp => ".bmp",
            ImageKind::Webp => ".webp",
            ImageKind::Tiff => ".tif",
            ImageKind::Emf => ".emf",
            ImageKind::Wmf => ".wmf",
        }
    }

    /// The canonical MIME type.
    pub fn media_type(self) -> &'static str {
        match self {
            ImageKind::Png => "image/png",
            ImageKind::Jpeg => "image/jpeg",
            ImageKind::Gif => "image/gif",
            ImageKind::Bmp => "image/bmp",
            ImageKind::Webp => "image/webp",
            ImageKind::Tiff => "image/tiff",
            ImageKind::Emf => "image/x-emf",
            ImageKind::Wmf => "image/x-wmf",
        }
    }
}

const WMF_PLACEABLE: u32 = 0x9AC6_CDD7;

/// Identify the image kind from its signature.
pub fn sniff(b: &[u8]) -> Option<ImageKind> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageKind::Png)
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(ImageKind::Jpeg)
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some(ImageKind::Gif)
    } else if b.starts_with(b"BM") && b.len() >= 26 {
        Some(ImageKind::Bmp)
    } else if b.starts_with(b"RIFF") && b.get(8..12) == Some(b"WEBP") {
        Some(ImageKind::Webp)
    } else if b.starts_with(b"II*\0") || b.starts_with(b"MM\0*") {
        Some(ImageKind::Tiff)
    } else if u32_le(b, 0) == Some(1) && b.get(40..44) == Some(b" EMF") {
        Some(ImageKind::Emf)
    } else if u32_le(b, 0) == Some(WMF_PLACEABLE)
        || (matches!(u16_le(b, 0), Some(1 | 2)) && u16_le(b, 2) == Some(9))
    {
        Some(ImageKind::Wmf)
    } else {
        None
    }
}

/// Pixel dimensions `(width, height)` read from the header, or `None` when
/// the format is unrecognized or the header is truncated / implausible.
/// Metafiles report their bounds in device pixels (EMF) or at 96 dpi
/// (placeable WMF); bare WMF carries no extent.
pub fn dimensions(b: &[u8]) -> Option<(u32, u32)> {
    let dims = match sniff(b)? {
        ImageKind::Png => (u32_be(b, 16)?, u32_be(b, 20)?),
        ImageKind::Jpeg => jpeg(b)?,
        ImageKind::Gif => (u16_le(b, 6)?, u16_le(b, 8)?),
        ImageKind::Bmp => {
            if u32_le(b, 14)? == 12 {
                (u16_le(b, 18)?, u16_le(b, 20)?)
            } else {
                (i32_le(b, 18)?.unsigned_abs() as u32, i32_le(b, 22)?.unsigned_abs() as u32)
            }
        }
        ImageKind::Webp => webp(b)?,
        ImageKind::Tiff => tiff(b)?,
        ImageKind::Emf => emf(b)?,
        ImageKind::Wmf => wmf(b)?,
    };
    (dims.0 > 0 && dims.1 > 0).then_some(dims)
}

fn jpeg(b: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 3 < b.len() {
        if b[i] != 0xFF {
            // Not at a marker: the stream is corrupt for our purposes.
            return None;
        }
        // Fill bytes.
        while i < b.len() && b[i] == 0xFF {
            i += 1;
        }
        let marker = *b.get(i)?;
        i += 1;
        // Standalone markers carry no length.
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            continue;
        }
        let len = u16_be(b, i)? as usize;
        if len < 2 {
            return None;
        }
        // SOFn (baseline, progressive, lossless, ...) but not DHT/JPG/DAC.
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let h = u16_be(b, i + 3)?;
            let w = u16_be(b, i + 5)?;
            return Some((w, h));
        }
        if marker == 0xDA {
            // Start of scan before any frame header.
            return None;
        }
        i += len;
    }
    None
}

fn webp(b: &[u8]) -> Option<(u32, u32)> {
    match b.get(12..16)? {
        b"VP8 " => {
            if b.get(23..26)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            Some((u16_le(b, 26)? & 0x3FFF, u16_le(b, 28)? & 0x3FFF))
        }
        b"VP8L" => {
            if *b.get(20)? != 0x2F {
                return None;
            }
            let bits = u32_le(b, 21)?;
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        b"VP8X" => Some((u24_le(b, 24)? + 1, u24_le(b, 27)? + 1)),
        _ => None,
    }
}

fn tiff(b: &[u8]) -> Option<(u32, u32)> {
    let le = b.starts_with(b"II");
    let r16 = |at: usize| if le { u16_le(b, at) } else { u16_be(b, at) };
    let r32 = |at: usize| if le { u32_le(b, at) } else { u32_be(b, at) };
    let ifd = r32(4)? as usize;
    let count = r16(ifd)? as usize;
    let (mut w, mut h) = (None, None);
    for n in 0..count.min(512) {
        let e = ifd + 2 + n * 12;
        let tag = r16(e)?;
        if tag != 256 && tag != 257 {
            continue;
        }
        let value = match r16(e + 2)? {
            3 => r16(e + 8)?,
            4 => r32(e + 8)?,
            _ => continue,
        };
        if tag == 256 {
            w = Some(value);
        } else {
            h = Some(value);
        }
        if w.is_some() && h.is_some() {
            break;
        }
    }
    Some((w?, h?))
}

fn emf(b: &[u8]) -> Option<(u32, u32)> {
    // EMR_HEADER.rclBounds: inclusive device-pixel rectangle.
    let (l, t, r, btm) = (i32_le(b, 8)?, i32_le(b, 12)?, i32_le(b, 16)?, i32_le(b, 20)?);
    let (w, h) = (r - l + 1, btm - t + 1);
    if w > 1 && h > 1 {
        return Some((w as u32, h as u32));
    }
    // Empty bounds: fall back to rclFrame (0.01 mm) scaled by the reference
    // device's pixels-per-millimetre.
    let (fl, ft, fr, fb) = (i32_le(b, 24)?, i32_le(b, 28)?, i32_le(b, 32)?, i32_le(b, 36)?);
    let (dev_w, dev_h) = (i32_le(b, 72)?, i32_le(b, 76)?);
    let (mm_w, mm_h) = (i32_le(b, 80)?, i32_le(b, 84)?);
    if mm_w <= 0 || mm_h <= 0 {
        return None;
    }
    let w = (fr - fl) * dev_w / (mm_w * 100);
    let h = (fb - ft) * dev_h / (mm_h * 100);
    (w > 0 && h > 0).then(|| (w as u32, h as u32))
}

fn wmf(b: &[u8]) -> Option<(u32, u32)> {
    if u32_le(b, 0)? != WMF_PLACEABLE {
        // A bare WMF has no extent in its header.
        return None;
    }
    let (l, t, r, btm) = (i16_le(b, 6)?, i16_le(b, 8)?, i16_le(b, 10)?, i16_le(b, 12)?);
    let inch = u16_le(b, 14)? as i64;
    if inch == 0 {
        return None;
    }
    let w = (r - l).abs() * 96 / inch;
    let h = (btm - t).abs() * 96 / inch;
    Some((w as u32, h as u32))
}

/// The file extension (with its dot) for an image payload: sniffed from the
/// bytes first, then the declared MIME type, then the source part's own
/// extension, finally `.bin`.
pub fn ext_for(bytes: &[u8], media_type: &str, origin_part: &str) -> String {
    if let Some(kind) = sniff(bytes) {
        return kind.ext().to_string();
    }
    let from_mime = match media_type.to_ascii_lowercase().as_str() {
        "image/png" => Some(".png"),
        "image/jpeg" | "image/jpg" | "image/pjpeg" => Some(".jpg"),
        "image/gif" => Some(".gif"),
        "image/bmp" | "image/x-bmp" | "image/x-ms-bmp" => Some(".bmp"),
        "image/webp" => Some(".webp"),
        "image/tiff" | "image/tif" => Some(".tif"),
        "image/x-emf" | "image/emf" => Some(".emf"),
        "image/x-wmf" | "image/wmf" => Some(".wmf"),
        "image/svg+xml" => Some(".svg"),
        _ => None,
    };
    if let Some(ext) = from_mime {
        return ext.to_string();
    }
    let tail = origin_part.rsplit(['/', '\\']).next().unwrap_or("");
    if let Some((_, ext)) = tail.rsplit_once('.') {
        if !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            return format!(".{}", ext.to_ascii_lowercase());
        }
    }
    ".bin".to_string()
}

/// The MIME type for an image payload: sniffed, else the declared one.
pub fn media_type_for(bytes: &[u8], declared: &str) -> String {
    match sniff(bytes) {
        Some(kind) => kind.media_type().to_string(),
        None => declared.to_string(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal PNG header (signature + IHDR) of the given size.
    pub(crate) fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.extend_from_slice(&13u32.to_be_bytes());
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        b
    }

    #[test]
    fn png_header() {
        assert_eq!(dimensions(&png(640, 480)), Some((640, 480)));
        assert_eq!(ext_for(&png(1, 1), "", ""), ".png");
    }

    #[test]
    fn jpeg_skips_app_segments_to_the_frame_header() {
        let mut b = vec![0xFF, 0xD8];
        // APP0 with 14 payload bytes.
        b.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        b.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        // SOF2 (progressive): len, precision, height 300, width 200.
        b.extend_from_slice(&[0xFF, 0xC2, 0x00, 0x11, 0x08, 0x01, 0x2C, 0x00, 0xC8, 0x03]);
        b.extend_from_slice(&[0; 9]);
        assert_eq!(dimensions(&b), Some((200, 300)));
        assert_eq!(ext_for(&b, "image/png", "x.png"), ".jpg");
    }

    #[test]
    fn jpeg_dht_is_not_a_frame_header() {
        let mut b = vec![0xFF, 0xD8, 0xFF, 0xC4, 0x00, 0x04, 0xAA, 0xBB];
        b.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x20, 0x00, 0x40, 0x03]);
        b.extend_from_slice(&[0; 9]);
        assert_eq!(dimensions(&b), Some((64, 32)));
    }

    #[test]
    fn gif_bmp_webp_tiff() {
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&[10, 0, 20, 0, 0, 0, 0]);
        assert_eq!(dimensions(&gif), Some((10, 20)));

        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 12]);
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&120i32.to_le_bytes());
        bmp.extend_from_slice(&(-80i32).to_le_bytes()); // top-down
        assert_eq!(dimensions(&bmp), Some((120, 80)));

        let mut vp8x = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
        vp8x.extend_from_slice(&[0; 8]);
        vp8x.extend_from_slice(&[99, 0, 0, 49, 0, 0]);
        assert_eq!(dimensions(&vp8x), Some((100, 50)));

        let mut vp8l = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f".to_vec();
        let bits: u32 = (15 - 1) | ((25 - 1) << 14);
        vp8l.extend_from_slice(&bits.to_le_bytes());
        assert_eq!(dimensions(&vp8l), Some((15, 25)));

        // Little-endian TIFF: IFD at 8, two entries (SHORT width, LONG height).
        let mut tif = b"II*\0".to_vec();
        tif.extend_from_slice(&8u32.to_le_bytes());
        tif.extend_from_slice(&2u16.to_le_bytes());
        tif.extend_from_slice(&256u16.to_le_bytes());
        tif.extend_from_slice(&3u16.to_le_bytes());
        tif.extend_from_slice(&1u32.to_le_bytes());
        tif.extend_from_slice(&[0x20, 0x03, 0, 0]); // 800
        tif.extend_from_slice(&257u16.to_le_bytes());
        tif.extend_from_slice(&4u16.to_le_bytes());
        tif.extend_from_slice(&1u32.to_le_bytes());
        tif.extend_from_slice(&600u32.to_le_bytes());
        assert_eq!(dimensions(&tif), Some((800, 600)));
    }

    #[test]
    fn metafiles() {
        let mut emf = vec![0u8; 88];
        emf[0] = 1;
        emf[8..12].copy_from_slice(&0i32.to_le_bytes());
        emf[12..16].copy_from_slice(&0i32.to_le_bytes());
        emf[16..20].copy_from_slice(&299i32.to_le_bytes());
        emf[20..24].copy_from_slice(&149i32.to_le_bytes());
        emf[40..44].copy_from_slice(b" EMF");
        assert_eq!(dimensions(&emf), Some((300, 150)));
        assert_eq!(ext_for(&emf, "", ""), ".emf");

        let mut wmf = vec![0u8; 22];
        wmf[0..4].copy_from_slice(&WMF_PLACEABLE.to_le_bytes());
        wmf[10..12].copy_from_slice(&1440i16.to_le_bytes());
        wmf[12..14].copy_from_slice(&720i16.to_le_bytes());
        wmf[14..16].copy_from_slice(&1440u16.to_le_bytes());
        assert_eq!(dimensions(&wmf), Some((96, 48)));
    }

    #[test]
    fn unknown_or_truncated_is_none() {
        assert_eq!(dimensions(b"not an image"), None);
        assert_eq!(dimensions(&png(0, 10)), None);
        assert_eq!(dimensions(&b"\x89PNG\r\n\x1a\n"[..]), None);
        assert_eq!(ext_for(b"???", "application/x-foo", "word/media/image3.jpeg"), ".jpeg");
        assert_eq!(ext_for(b"???", "", ""), ".bin");
    }
}
