//! Standalone raster images, read with local OCR.
//!
//! An image document carries no text layer, so it converts only when the
//! converter has an OCR engine; the crate reports it as unsupported
//! otherwise. Decoding is bounded before any pixel buffer exists: the header
//! alone decides whether the image is within the fixed dimension and area
//! limits below. Recognized text follows the same output policy as
//! recognized PDF pages.
//!
//! EXIF orientation is not applied, so an image stored rotated is recognized
//! in the orientation it is stored in.

use crate::error::ConvertError;
use crate::ocr::PageOcr;
use image::error::{ImageError, LimitErrorKind};
use image::{ImageReader, Limits};
use std::io::Cursor;

/// Widest and tallest image accepted, in pixels.
const MAX_DIMENSION: u32 = 16_384;

/// Largest image area accepted, in pixels.
const MAX_PIXELS: u64 = 25_000_000;

pub fn to_markdown_with_ocr(
    bytes: &[u8],
    engine: &crate::ocr::Engine,
) -> Result<String, ConvertError> {
    to_markdown_with_page_ocr(bytes, engine)
}

fn to_markdown_with_page_ocr<O: PageOcr>(bytes: &[u8], ocr: &O) -> Result<String, ConvertError> {
    // Check the header first, so a huge image is rejected before to decode
    // anything.
    let (width, height) = reader(bytes)?.into_dimensions().map_err(map_error)?;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .filter(|&pixels| pixels <= MAX_PIXELS)
        .ok_or_else(|| ConvertError::ResourceLimit {
            limit: "max_image_pixels",
            detail: format!("{width}x{height} exceeds {MAX_PIXELS} pixels"),
        })?;
    log::debug!("recognizing a {width}x{height} image ({pixels} pixels)");

    let decoded = reader(bytes)?.decode().map_err(map_error)?.into_rgba8();
    let (width, height) = (decoded.width(), decoded.height());
    let recognized = ocr
        .recognize_page(width, height, decoded.as_raw())
        .map_err(|error| ConvertError::Ocr { reason: error.to_string() })?;

    let lines = super::pdf::recognized_lines(&recognized);
    if lines.is_empty() {
        log::debug!("no text was recognized in the image");
        return Ok(String::new());
    }
    let mut markdown = lines.join("\n");
    markdown.push('\n');
    Ok(markdown)
}

/// A reader with the codec guessed from the signature and the limits set.
fn reader(bytes: &[u8]) -> Result<ImageReader<Cursor<&[u8]>>, ConvertError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| ConvertError::malformed(format!("unreadable image: {error}")))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    reader.limits(limits);
    Ok(reader)
}

fn map_error(error: ImageError) -> ConvertError {
    match error {
        ImageError::Limits(error) => ConvertError::ResourceLimit {
            limit: match error.kind() {
                LimitErrorKind::DimensionError => "max_image_dimension",
                _ => "max_image_alloc",
            },
            detail: format!("image exceeds the decoder limits: {error}"),
        },
        ImageError::Unsupported(error) => {
            ConvertError::Unsupported(format!("image codec: {error}"))
        }
        // Truncated or corrupt bytes arrive as a decoding or IO error, and
        // either way the input is at fault, not the environment.
        error => ConvertError::malformed(format!("undecodable image: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_DIMENSION, to_markdown_with_page_ocr};
    use crate::error::ConvertError;
    use crate::ocr::{OcrPageError, PageOcr};
    use image::{DynamicImage, ImageFormat, RgbaImage};
    use std::cell::RefCell;
    use std::io::Cursor;

    /// Every codec the `ocr` feature enables.
    const CODECS: [ImageFormat; 5] = [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::WebP,
        ImageFormat::Tiff,
        ImageFormat::Bmp,
    ];

    /// Records what it was handed and replays a scripted result.
    struct FakeOcr {
        result: Result<Vec<String>, ()>,
        images: RefCell<Vec<(u32, u32)>>,
    }

    impl FakeOcr {
        fn recognizing(lines: &[&str]) -> Self {
            let lines = lines.iter().map(|line| (*line).to_string()).collect();
            Self { result: Ok(lines), images: RefCell::new(Vec::new()) }
        }

        fn failing() -> Self {
            Self { result: Err(()), images: RefCell::new(Vec::new()) }
        }

        fn images(&self) -> Vec<(u32, u32)> {
            self.images.borrow().clone()
        }
    }

    impl PageOcr for FakeOcr {
        fn recognize_page(
            &self,
            width: u32,
            height: u32,
            rgba: &[u8],
        ) -> Result<Vec<String>, OcrPageError> {
            assert_eq!(rgba.len(), width as usize * height as usize * 4);
            self.images.borrow_mut().push((width, height));
            self.result
                .clone()
                .map_err(|()| OcrPageError::Recognition { detail: "fake failure".into() })
        }
    }

    /// A 3x2 image with a distinct pixel per position, encoded with `format`.
    fn sample_image(format: ImageFormat) -> Vec<u8> {
        let mut pixels = RgbaImage::new(3, 2);
        for (index, pixel) in pixels.pixels_mut().enumerate() {
            let step = index as u8 * 40;
            *pixel = image::Rgba([step, 255 - step, 128 + step / 2, 255]);
        }
        let sample = match format {
            // JPEG has no alpha channel.
            ImageFormat::Jpeg => {
                DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(pixels).to_rgb8())
            }
            _ => DynamicImage::ImageRgba8(pixels),
        };
        let mut encoded = Cursor::new(Vec::new());
        sample.write_to(&mut encoded, format).expect("encode the sample image");
        encoded.into_inner()
    }

    #[test]
    fn every_enabled_codec_reaches_the_engine_with_its_decoded_pixels() {
        for format in CODECS {
            let ocr = FakeOcr::recognizing(&["Recognized line"]);
            let markdown = to_markdown_with_page_ocr(&sample_image(format), &ocr)
                .unwrap_or_else(|error| panic!("{format:?} failed: {error}"));

            assert_eq!(ocr.images(), [(3, 2)], "{format:?}");
            assert_eq!(markdown, "Recognized line\n", "{format:?}");
        }
    }

    /// Every codec this crate decodes is also detected from its signature, so
    /// no image document depends on its file extension.
    #[test]
    fn every_enabled_codec_is_detected_from_its_bytes() {
        for format in CODECS {
            assert_eq!(
                crate::Format::from_bytes(&sample_image(format)),
                Some(crate::Format::Image),
                "{format:?}"
            );
        }
    }

    #[test]
    fn recognized_text_follows_the_shared_output_policy() {
        let ocr = FakeOcr::recognizing(&["# not a heading", "", "1. not a list"]);
        let markdown = to_markdown_with_page_ocr(&sample_image(ImageFormat::Png), &ocr).unwrap();

        assert_eq!(markdown, "\\# not a heading\n1\\. not a list\n");
    }

    #[test]
    fn an_image_without_text_converts_to_nothing() {
        let ocr = FakeOcr::recognizing(&[]);
        let markdown = to_markdown_with_page_ocr(&sample_image(ImageFormat::Png), &ocr).unwrap();

        assert_eq!(markdown, "");
    }

    #[test]
    fn a_failed_recognition_is_terminal_for_a_single_page_document() {
        let ocr = FakeOcr::failing();
        let result = to_markdown_with_page_ocr(&sample_image(ImageFormat::Png), &ocr);

        assert!(matches!(result, Err(ConvertError::Ocr { .. })), "{result:?}");
    }

    #[test]
    fn malformed_bytes_are_a_typed_error() {
        let ocr = FakeOcr::recognizing(&["unexpected"]);
        let mut truncated = sample_image(ImageFormat::Png);
        truncated.truncate(20);
        let result = to_markdown_with_page_ocr(&truncated, &ocr);

        assert!(matches!(result, Err(ConvertError::Malformed { .. })), "{result:?}");
        assert!(ocr.images().is_empty());
    }

    /// The declared dimensions are rejected from the header alone, so no
    /// buffer for 30000x30000 pixels is ever requested.
    #[test]
    fn oversized_dimensions_are_rejected_before_decoding() {
        let ocr = FakeOcr::recognizing(&["unexpected"]);
        let result = to_markdown_with_page_ocr(&png_declaring(30_000, 30_000), &ocr);

        let Err(ConvertError::ResourceLimit { limit, .. }) = result else {
            panic!("an oversized image was accepted: {result:?}");
        };
        assert_eq!(limit, "max_image_dimension");
        assert!(ocr.images().is_empty());
    }

    /// Within the per-side limit, the area limit still rejects the image.
    #[test]
    fn oversized_area_is_rejected_before_decoding() {
        let ocr = FakeOcr::recognizing(&["unexpected"]);
        let result = to_markdown_with_page_ocr(&png_declaring(MAX_DIMENSION, MAX_DIMENSION), &ocr);

        let Err(ConvertError::ResourceLimit { limit, .. }) = result else {
            panic!("an oversized image was accepted: {result:?}");
        };
        assert_eq!(limit, "max_image_pixels");
        assert!(ocr.images().is_empty());
    }

    /// A PNG whose header declares this size over an empty image body. The
    /// limits act on the header, so the declared size costs nothing to build.
    fn png_declaring(width: u32, height: u32) -> Vec<u8> {
        let mut ihdr = Vec::from(b"IHDR".as_slice());
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        // Bit depth 8, colour type 6 (RGBA), deflate, no filter, no interlace.
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);

        let mut png = Vec::from(b"\x89PNG\r\n\x1a\n".as_slice());
        push_chunk(&mut png, &ihdr);
        // An empty deflate stream; nothing here is ever inflated.
        push_chunk(&mut png, b"IDAT\x78\x01\x03\x00\x00\x00\x00\x01");
        push_chunk(&mut png, b"IEND");
        png
    }

    /// Append a PNG chunk: its length, the type and data, then the CRC.
    fn push_chunk(png: &mut Vec<u8>, chunk: &[u8]) {
        let data_len = chunk.len() - b"IHDR".len();
        png.extend_from_slice(&(data_len as u32).to_be_bytes());
        png.extend_from_slice(chunk);
        png.extend_from_slice(&crc32(chunk).to_be_bytes());
    }

    /// PNG chunk CRC-32 (IEEE, reflected), computed directly so the fixture
    /// needs no dependency.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }
}
