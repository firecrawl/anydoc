use std::{fmt, time::Duration};

use async_trait::async_trait;
use reqwest::{StatusCode, Url, header::RETRY_AFTER};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::db::ModelProfile;

use super::{
    retry::RetryPolicy,
    types::{ContentPart, ModelClient, ModelError, ModelRequest, ModelResponse},
};

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

struct ApiCredential(String);

impl fmt::Debug for ApiCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApiCredential([REDACTED])")
    }
}

pub struct OpenAiCompatibleClient {
    http: reqwest::Client,
    endpoint: Url,
    profile: ModelProfile,
    api_key: ApiCredential,
    retry: RetryPolicy,
}

impl fmt::Debug for OpenAiCompatibleClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiCompatibleClient")
            .field("endpoint", &self.endpoint)
            .field("profile", &self.profile)
            .field("api_key", &self.api_key)
            .field("retry", &self.retry)
            .finish()
    }
}

impl OpenAiCompatibleClient {
    pub fn new(profile: ModelProfile, api_key: String) -> Result<Self, ModelError> {
        if api_key.trim().is_empty() {
            return Err(ModelError::InvalidProfile("API key is empty".into()));
        }
        if profile.model.trim().is_empty() {
            return Err(ModelError::InvalidProfile("model name is empty".into()));
        }
        let endpoint = chat_endpoint(&profile.base_url)?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(profile.timeout_secs.max(1) as u64))
            .build()
            .map_err(|error| ModelError::Request(error.to_string()))?;
        Ok(Self {
            http,
            endpoint,
            profile,
            api_key: ApiCredential(api_key.trim().to_owned()),
            retry: RetryPolicy::default(),
        })
    }

    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    async fn send_once(&self, payload: &Value) -> Result<ModelResponse, AttemptError> {
        let response = self
            .http
            .post(self.endpoint.clone())
            .bearer_auth(&self.api_key.0)
            .json(payload)
            .send()
            .await
            .map_err(AttemptError::Network)?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = read_limited(response).await.map_err(AttemptError::Terminal)?;

        if !status.is_success() {
            return Err(AttemptError::Status {
                status,
                retry_after,
                body: sanitize(&body, &self.api_key.0),
            });
        }

        let response: ChatResponse = serde_json::from_slice(&body).map_err(|error| {
            AttemptError::Terminal(ModelError::InvalidResponse(error.to_string()))
        })?;
        let content = response
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content)
            .filter(|content| !content.is_empty())
            .ok_or_else(|| {
                AttemptError::Terminal(ModelError::InvalidResponse(
                    "missing choices[0].message.content".into(),
                ))
            })?;
        Ok(ModelResponse { content, model: response.model, usage: response.usage })
    }
}

#[async_trait]
impl ModelClient for OpenAiCompatibleClient {
    async fn chat(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let has_image = request.messages.iter().any(|message| {
            message.content.iter().any(|part| matches!(part, ContentPart::ImageUrl { .. }))
        });
        if has_image && !self.profile.supports_vision {
            return Err(ModelError::VisionUnsupported);
        }
        let payload = request_payload(&self.profile.model, request);

        for attempt in 1..=self.retry.max_attempts {
            match self.send_once(&payload).await {
                Ok(response) => return Ok(response),
                Err(AttemptError::Network(error)) => {
                    if attempt == self.retry.max_attempts {
                        return Err(ModelError::Request(sanitize(
                            &error.to_string(),
                            &self.api_key.0,
                        )));
                    }
                    tokio::time::sleep(self.retry.delay(attempt, None)).await;
                }
                Err(AttemptError::Status { status, retry_after, body }) => {
                    if RetryPolicy::should_retry_status(status.as_u16())
                        && attempt < self.retry.max_attempts
                    {
                        tokio::time::sleep(self.retry.delay(attempt, retry_after.as_deref())).await;
                        continue;
                    }
                    return Err(ModelError::HttpStatus { status: status.as_u16(), message: body });
                }
                Err(AttemptError::Terminal(error)) => return Err(error),
            }
        }
        Err(ModelError::Request("retry policy exhausted".into()))
    }
}

fn chat_endpoint(base_url: &str) -> Result<Url, ModelError> {
    let trimmed = base_url.trim().trim_end_matches('/');
    let endpoint = if trimmed.ends_with("/chat/completions") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/chat/completions")
    };
    let url = Url::parse(&endpoint)
        .map_err(|error| ModelError::InvalidProfile(format!("invalid API Base URL: {error}")))?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        return Err(ModelError::InvalidProfile(
            "API Base URL must use HTTPS unless it is local".into(),
        ));
    }
    Ok(url)
}

fn request_payload(model: &str, request: ModelRequest) -> Value {
    let messages = request
        .messages
        .into_iter()
        .map(|message| {
            let content = message
                .content
                .into_iter()
                .map(|part| match part {
                    ContentPart::Text { text } => json!({"type": "text", "text": text}),
                    ContentPart::ImageUrl { url } => {
                        json!({"type": "image_url", "image_url": {"url": url}})
                    }
                })
                .collect::<Vec<_>>();
            json!({"role": message.role, "content": content})
        })
        .collect::<Vec<_>>();
    let mut payload = json!({"model": model, "messages": messages});
    if let Some(schema) = request.response_schema {
        payload["response_format"] = json!({
            "type": "json_schema",
            "json_schema": {
                "name": "structured_response",
                "strict": true,
                "schema": schema
            }
        });
    }
    payload
}

async fn read_limited(mut response: reqwest::Response) -> Result<Vec<u8>, ModelError> {
    if response.content_length().is_some_and(|length| length > MAX_RESPONSE_BYTES as u64) {
        return Err(ModelError::ResponseTooLarge { limit: MAX_RESPONSE_BYTES });
    }
    let mut body = Vec::new();
    while let Some(chunk) =
        response.chunk().await.map_err(|error| ModelError::Request(error.to_string()))?
    {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(ModelError::ResponseTooLarge { limit: MAX_RESPONSE_BYTES });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn sanitize(value: impl AsRef<[u8]>, secret: &str) -> String {
    String::from_utf8_lossy(value.as_ref()).replace(secret, "[REDACTED]")
}

enum AttemptError {
    Network(reqwest::Error),
    Status { status: StatusCode, retry_after: Option<String>, body: String },
    Terminal(ModelError),
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<Value>,
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: String,
}
