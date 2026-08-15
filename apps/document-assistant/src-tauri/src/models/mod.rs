pub mod openai_compatible;
pub mod retry;
pub mod types;

pub use openai_compatible::OpenAiCompatibleClient;
pub use types::{ContentPart, ModelClient, ModelError, ModelMessage, ModelRequest, ModelResponse};

#[cfg(test)]
mod openai_compatible_test;
