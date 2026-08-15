use std::{collections::HashSet, sync::Arc};

use serde_json::json;
use thiserror::Error;

use crate::{
    analysis::CitedAnswer,
    models::{ContentPart, ModelClient, ModelError, ModelMessage, ModelRequest},
    search::{ContextPage, SearchIndex},
};

use super::prompts::build_chat_prompt;

const FULL_CONTEXT_CHAR_LIMIT: usize = 30_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Error)]
pub enum ChatError {
    #[error(transparent)]
    Search(#[from] anyhow::Error),
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error("invalid chat response: {0}")]
    InvalidResponse(String),
    #[error("model cited page {0}, which was not supplied as context")]
    InvalidCitation(u32),
}

pub struct ChatService {
    search: SearchIndex,
    model: Arc<dyn ModelClient>,
}

impl ChatService {
    pub fn new(search: SearchIndex, model: Arc<dyn ModelClient>) -> Self {
        Self { search, model }
    }

    pub async fn ask(
        &self,
        document_id: &str,
        question: &str,
        recent: &[ChatTurn],
    ) -> Result<CitedAnswer, ChatError> {
        let all_pages = self.search.all_context(document_id)?;
        let full_size = all_pages.iter().map(|page| context_size(page)).sum::<usize>();
        let pages = if full_size <= FULL_CONTEXT_CHAR_LIMIT {
            all_pages
        } else {
            self.search.retrieve_context(document_id, question, 8)?
        };
        if pages.is_empty()
            || pages.iter().all(|page| {
                page.text.trim().is_empty()
                    && page.visual_summary.as_deref().unwrap_or_default().trim().is_empty()
            })
        {
            return Ok(not_found());
        }

        let supplied_pages = pages.iter().map(|page| page.page_number).collect::<HashSet<_>>();
        let response = self.model.chat(ModelRequest {
            messages: vec![ModelMessage { role: "user".into(), content: vec![ContentPart::Text {
                text: build_chat_prompt(question, &pages, recent),
            }] }],
            response_schema: Some(json!({
                "type": "object",
                "properties": {
                    "answer": {"type": "string"},
                    "citations": {"type": "array", "items": {
                        "type": "object",
                        "properties": {"pageNumber": {"type": "integer", "minimum": 1}, "excerpt": {"type": ["string", "null"]}},
                        "required": ["pageNumber", "excerpt"], "additionalProperties": false
                    }},
                    "grounded": {"type": "boolean"}
                },
                "required": ["answer", "citations", "grounded"],
                "additionalProperties": false
            })),
        }).await?;
        let answer: CitedAnswer = serde_json::from_str(&response.content)
            .map_err(|error| ChatError::InvalidResponse(error.to_string()))?;
        for citation in &answer.citations {
            if !supplied_pages.contains(&citation.page_number) {
                return Err(ChatError::InvalidCitation(citation.page_number));
            }
        }
        if !answer.grounded || answer.citations.is_empty() {
            return Ok(not_found());
        }
        Ok(answer)
    }
}

fn context_size(page: &ContextPage) -> usize {
    page.text.chars().count()
        + page.heading.as_deref().unwrap_or_default().chars().count()
        + page.visual_summary.as_deref().unwrap_or_default().chars().count()
}

fn not_found() -> CitedAnswer {
    CitedAnswer {
        answer: "文档中未找到相关信息".into(), citations: Vec::new(), grounded: false
    }
}
