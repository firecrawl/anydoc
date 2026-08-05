//! Optional local OCR engine initialization.
//!
//! Model loading stays byte-oriented so the same path works in native and
//! browser WebAssembly builds. Only operators used by the standard ocrs models
//! are registered, keeping compile time and linked binary size bounded.

use std::fmt;

use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::{Model, ModelOptions, OpRegistry, op_registry};

const MAX_MODEL_BYTES: usize = 64 * 1024 * 1024;

pub(crate) struct Engine {
    inner: OcrEngine,
}

impl Engine {
    pub(crate) fn from_bytes(
        detection_model: Vec<u8>,
        recognition_model: Vec<u8>,
    ) -> Result<Self, OcrInitError> {
        validate_model_size("detection", detection_model.len())?;
        validate_model_size("recognition", recognition_model.len())?;
        let detection_model = load_model("detection", detection_model)?;
        let recognition_model = load_model("recognition", recognition_model)?;
        let inner = OcrEngine::new(OcrEngineParams {
            detection_model: Some(detection_model),
            recognition_model: Some(recognition_model),
            ..Default::default()
        })
        .map_err(|error| OcrInitError::Engine { detail: error.to_string() })?;
        Ok(Self { inner })
    }

    /// One string per recognized line, in reading order, blank ones dropped.
    /// `rgba` is row-major RGBA8, so it has to be `width * height * 4` long.
    pub(crate) fn recognize_rgba_page(
        &self,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<Vec<String>, OcrPageError> {
        let expected = rgba_len(width, height).ok_or(OcrPageError::PageSize {
            width,
            height,
            bytes: rgba.len(),
        })?;
        if rgba.len() != expected {
            return Err(OcrPageError::PageSize { width, height, bytes: rgba.len() });
        }
        let source = ImageSource::from_bytes(rgba, (width, height))
            .map_err(|error| OcrPageError::Recognition { detail: error.to_string() })?;
        let input = self
            .inner
            .prepare_input(source)
            .map_err(|error| OcrPageError::Recognition { detail: error.to_string() })?;
        let words = self
            .inner
            .detect_words(&input)
            .map_err(|error| OcrPageError::Recognition { detail: error.to_string() })?;
        let lines = self.inner.find_text_lines(&input, &words);
        let recognized = self
            .inner
            .recognize_text(&input, &lines)
            .map_err(|error| OcrPageError::Recognition { detail: error.to_string() })?;
        Ok(recognized
            .into_iter()
            .flatten()
            .map(|line| line.to_string())
            .filter(|line| !line.trim().is_empty())
            .collect())
    }
}

/// Recognition of one page, so tests can stand in for the real engine.
pub(crate) trait PageOcr {
    /// See [`Engine::recognize_rgba_page`].
    fn recognize_page(
        &self,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<Vec<String>, OcrPageError>;
}

impl PageOcr for Engine {
    fn recognize_page(
        &self,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<Vec<String>, OcrPageError> {
        self.recognize_rgba_page(width, height, rgba)
    }
}

/// Why one page could not be recognized.
#[derive(Debug)]
pub(crate) enum OcrPageError {
    /// The buffer does not describe a non-empty RGBA8 image of that size,
    /// which is an internal error and never bad input.
    PageSize { width: u32, height: u32, bytes: usize },
    /// The OCR pipeline rejected the page.
    Recognition { detail: String },
}

impl fmt::Display for OcrPageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OcrPageError::PageSize { width, height, bytes } => {
                write!(f, "{bytes} pixel bytes do not describe a {width}x{height} RGBA image")
            }
            OcrPageError::Recognition { detail } => write!(f, "recognition failed: {detail}"),
        }
    }
}

/// Byte length of a non-empty RGBA8 buffer this size, or `None` when the
/// dimensions are empty or overflow the `u32` the image source multiplies in.
fn rgba_len(width: u32, height: u32) -> Option<usize> {
    let pixels = width.checked_mul(height).filter(|&pixels| pixels != 0)?;
    usize::try_from(pixels).ok()?.checked_mul(4)
}

/// Why local OCR initialization failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum OcrInitError {
    /// A model exceeded the fixed configuration-size limit.
    ModelTooLarge {
        /// Whether this was the `detection` or `recognition` model.
        model: &'static str,
        /// Supplied model size in bytes.
        bytes: usize,
        /// Maximum accepted model size in bytes.
        max_bytes: usize,
    },
    /// RTen could not parse or validate a model.
    InvalidModel {
        /// Whether this was the `detection` or `recognition` model.
        model: &'static str,
        /// Loader diagnostic.
        detail: String,
    },
    /// The loaded model pair could not initialize the OCR engine.
    Engine {
        /// Engine diagnostic.
        detail: String,
    },
}

impl fmt::Display for OcrInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OcrInitError::ModelTooLarge { model, bytes, max_bytes } => {
                write!(f, "OCR {model} model is too large: {bytes} bytes exceeds {max_bytes} bytes")
            }
            OcrInitError::InvalidModel { model, detail } => {
                write!(f, "invalid OCR {model} model: {detail}")
            }
            OcrInitError::Engine { detail } => write!(f, "could not initialize OCR: {detail}"),
        }
    }
}

impl std::error::Error for OcrInitError {}

fn load_model(model: &'static str, bytes: Vec<u8>) -> Result<Model, OcrInitError> {
    if bytes.is_empty() {
        return Err(OcrInitError::InvalidModel { model, detail: "model bytes are empty".into() });
    }
    ModelOptions::with_ops(ocr_ops())
        .load(bytes)
        .map_err(|error| OcrInitError::InvalidModel { model, detail: error.to_string() })
}

fn validate_model_size(model: &'static str, bytes: usize) -> Result<(), OcrInitError> {
    if bytes > MAX_MODEL_BYTES {
        return Err(OcrInitError::ModelTooLarge { model, bytes, max_bytes: MAX_MODEL_BYTES });
    }
    Ok(())
}

fn ocr_ops() -> OpRegistry {
    op_registry!(
        Add,
        AveragePool,
        Cast,
        Concat,
        ConstantOfShape,
        Conv,
        ConvTranspose,
        GRU,
        Gather,
        LogSoftmax,
        MatMul,
        MaxPool,
        Pad,
        Relu,
        Reshape,
        Shape,
        Sigmoid,
        Slice,
        Transpose,
        Unsqueeze
    )
}

#[cfg(test)]
mod tests {
    use super::{Engine, MAX_MODEL_BYTES, OcrInitError, rgba_len, validate_model_size};

    #[test]
    fn model_size_limit_is_fixed() {
        assert!(validate_model_size("detection", MAX_MODEL_BYTES).is_ok());
        assert!(matches!(
            validate_model_size("detection", MAX_MODEL_BYTES + 1),
            Err(OcrInitError::ModelTooLarge { model: "detection", .. })
        ));
    }

    #[test]
    fn engine_is_safe_to_share_between_conversions() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Engine>();
    }

    /// The page recognizer accepts a buffer only when it exactly describes the
    /// stated RGBA8 image, and computes that size without overflowing.
    #[test]
    fn rgba_length_is_checked_for_empty_and_oversized_pages() {
        assert_eq!(rgba_len(2, 3), Some(24));
        assert_eq!(rgba_len(0, 3), None);
        assert_eq!(rgba_len(3, 0), None);
        assert_eq!(rgba_len(u32::MAX, 2), None);
    }

    #[test]
    fn oversized_recognition_model_is_rejected_before_detection_is_parsed() {
        let invalid_detection = vec![0u8, 1, 2, 3];
        let oversized_recognition = vec![0u8; MAX_MODEL_BYTES + 1];
        let result = Engine::from_bytes(invalid_detection, oversized_recognition);
        assert!(matches!(result, Err(OcrInitError::ModelTooLarge { model: "recognition", .. })));
    }
}
