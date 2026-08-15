use std::{path::PathBuf, sync::Arc};

use base64::{Engine, engine::general_purpose::STANDARD};
use thiserror::Error;
use tokio::task::JoinSet;

use crate::{
    db::ModelProfile,
    models::{ContentPart, ModelClient, ModelError, ModelMessage, ModelRequest},
};

use super::{
    prompts::{build_page_prompt, build_repair_prompt},
    schema::{page_analysis_schema, parse_page_analysis, validate_page_number},
    types::PageAnalysis,
};

const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 30_000_000;
const MAX_REPAIR_RAW_CHARS: usize = 64_000;

#[derive(Debug, Clone)]
pub struct PageAnalysisInput {
    pub page_number: u32,
    pub image_path: PathBuf,
    pub page_text: String,
    pub previous_heading: Option<String>,
    pub next_heading: Option<String>,
}

#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error("configured profile does not support visual input")]
    VisionUnsupported,
    #[error("page image is too large: {bytes} bytes (limit {limit})")]
    ImageTooLarge { bytes: u64, limit: u64 },
    #[error("page image has too many pixels: {pixels} (limit {limit})")]
    TooManyPixels { pixels: u64, limit: u64 },
    #[error("read page image: {0}")]
    Io(#[from] std::io::Error),
    #[error("decode page image: {0}")]
    Image(#[from] image::ImageError),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error("model JSON remained invalid after one repair: {reason}")]
    InvalidResponse { reason: String, raw_response: String },
    #[error("vision worker failed: {0}")]
    Worker(String),
}

#[derive(Clone)]
pub struct VisionAnalyzer {
    profile: ModelProfile,
    model: Arc<dyn ModelClient>,
}

impl VisionAnalyzer {
    pub fn new(profile: ModelProfile, model: Arc<dyn ModelClient>) -> Self {
        Self { profile, model }
    }

    pub async fn analyze_page(
        &self,
        input: PageAnalysisInput,
    ) -> Result<PageAnalysis, AnalysisError> {
        if !self.profile.supports_vision {
            return Err(AnalysisError::VisionUnsupported);
        }

        let metadata = std::fs::metadata(&input.image_path)?;
        if metadata.len() > MAX_IMAGE_BYTES {
            return Err(AnalysisError::ImageTooLarge {
                bytes: metadata.len(),
                limit: MAX_IMAGE_BYTES,
            });
        }
        let (width, height) = image::image_dimensions(&input.image_path)?;
        let pixels = u64::from(width) * u64::from(height);
        if pixels > MAX_IMAGE_PIXELS {
            return Err(AnalysisError::TooManyPixels { pixels, limit: MAX_IMAGE_PIXELS });
        }
        let image = std::fs::read(&input.image_path)?;
        let data_url = format!("data:image/png;base64,{}", STANDARD.encode(image));
        let response = self
            .model
            .chat(ModelRequest {
                messages: vec![ModelMessage {
                    role: "user".into(),
                    content: vec![
                        ContentPart::Text { text: build_page_prompt(&input) },
                        ContentPart::ImageUrl { url: data_url },
                    ],
                }],
                response_schema: Some(page_analysis_schema()),
            })
            .await?;

        match parse_and_validate(&response.content, input.page_number) {
            Ok(analysis) => Ok(analysis),
            Err(first_error) => {
                let raw = truncate_chars(&response.content, MAX_REPAIR_RAW_CHARS);
                let repair = self
                    .model
                    .chat(ModelRequest {
                        messages: vec![ModelMessage {
                            role: "user".into(),
                            content: vec![ContentPart::Text {
                                text: build_repair_prompt(
                                    input.page_number,
                                    &raw,
                                    &first_error.to_string(),
                                ),
                            }],
                        }],
                        response_schema: Some(page_analysis_schema()),
                    })
                    .await?;
                parse_and_validate(&repair.content, input.page_number).map_err(|error| {
                    AnalysisError::InvalidResponse {
                        reason: error.to_string(),
                        raw_response: truncate_chars(&repair.content, MAX_REPAIR_RAW_CHARS),
                    }
                })
            }
        }
    }

    pub async fn analyze_pages(
        &self,
        inputs: Vec<PageAnalysisInput>,
    ) -> Vec<(u32, Result<PageAnalysis, AnalysisError>)> {
        let concurrency = self.profile.max_concurrency.clamp(1, 8) as usize;
        let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
        let mut workers = JoinSet::new();
        for input in inputs {
            let analyzer = self.clone();
            let semaphore = semaphore.clone();
            workers.spawn(async move {
                let page_number = input.page_number;
                let permit = semaphore
                    .acquire_owned()
                    .await
                    .map_err(|error| AnalysisError::Worker(error.to_string()))?;
                let result = analyzer.analyze_page(input).await;
                drop(permit);
                Ok::<_, AnalysisError>((page_number, result))
            });
        }

        let mut results = Vec::new();
        while let Some(worker) = workers.join_next().await {
            match worker {
                Ok(Ok(result)) => results.push(result),
                Ok(Err(error)) => results.push((0, Err(error))),
                Err(error) => results.push((0, Err(AnalysisError::Worker(error.to_string())))),
            }
        }
        results.sort_by_key(|(page_number, _)| *page_number);
        results
    }
}

fn parse_and_validate(raw: &str, page_number: u32) -> anyhow::Result<PageAnalysis> {
    let analysis = parse_page_analysis(raw)?;
    validate_page_number(&analysis, page_number)?;
    Ok(analysis)
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}
