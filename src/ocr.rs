//! Optional local OCR engine initialization.
//!
//! Model loading stays byte-oriented so the same path works in native and
//! browser WebAssembly builds. Only operators used by the standard ocrs models
//! are registered, keeping compile time and linked binary size bounded.

use std::fmt;

use ocrs::{OcrEngine, OcrEngineParams};
use rten::{Model, ModelOptions, OpRegistry, op_registry};

const MAX_MODEL_BYTES: usize = 64 * 1024 * 1024;

pub(crate) struct Engine {
    // Retained for the image and PDF inference paths added in the next phases.
    _inner: OcrEngine,
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
        Ok(Self { _inner: inner })
    }
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
    use super::{Engine, MAX_MODEL_BYTES, OcrInitError, validate_model_size};

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

    #[test]
    fn oversized_recognition_model_is_rejected_before_detection_is_parsed() {
        let invalid_detection = vec![0u8, 1, 2, 3];
        let oversized_recognition = vec![0u8; MAX_MODEL_BYTES + 1];
        let result = Engine::from_bytes(invalid_detection, oversized_recognition);
        assert!(matches!(result, Err(OcrInitError::ModelTooLarge { model: "recognition", .. })));
    }
}
