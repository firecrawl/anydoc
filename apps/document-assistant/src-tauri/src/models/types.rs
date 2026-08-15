use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: String,
    pub content: Vec<ContentPart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRequest {
    pub messages: Vec<ModelMessage>,
    pub response_schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelResponse {
    pub content: String,
    pub model: Option<String>,
    pub usage: Option<Value>,
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("invalid model profile: {0}")]
    InvalidProfile(String),
    #[error("vision input is not allowed for this profile")]
    VisionUnsupported,
    #[error("model request failed: {0}")]
    Request(String),
    #[error("model returned HTTP {status}: {message}")]
    HttpStatus { status: u16, message: String },
    #[error("model response exceeded {limit} bytes")]
    ResponseTooLarge { limit: usize },
    #[error("invalid model response: {0}")]
    InvalidResponse(String),
}

#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn chat(&self, request: ModelRequest) -> Result<ModelResponse, ModelError>;
}
