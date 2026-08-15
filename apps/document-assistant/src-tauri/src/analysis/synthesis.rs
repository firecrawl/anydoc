use std::sync::Arc;

use thiserror::Error;

use crate::models::{ContentPart, ModelClient, ModelError, ModelMessage, ModelRequest};

use super::{
    prompts::{build_document_prompt, build_section_prompt},
    schema::{document_summary_schema, validate_evidence_pages},
    types::{DocumentSummary, PageAnalysis},
};

#[derive(Debug, Clone, Copy)]
pub struct ContextBudget {
    maximum_tokens: usize,
}

impl ContextBudget {
    pub fn new(maximum_tokens: usize) -> Self {
        Self { maximum_tokens: maximum_tokens.max(512) }
    }

    fn fits(self, value: &str) -> bool {
        estimate_tokens(value) <= self.maximum_tokens
    }
}

#[derive(Debug, Clone)]
pub struct SynthesisInput {
    pub markdown: String,
    pub page_analyses: Vec<PageAnalysis>,
    pub total_pages: u32,
    pub vision_enabled: bool,
    pub failed_pages: Vec<u32>,
}

#[derive(Debug, Error)]
pub enum SynthesisError {
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error("invalid document summary: {0}")]
    InvalidSummary(String),
}

pub struct DocumentSynthesizer {
    model: Arc<dyn ModelClient>,
    budget: ContextBudget,
}

impl DocumentSynthesizer {
    pub fn new(model: Arc<dyn ModelClient>, budget: ContextBudget) -> Self {
        Self { model, budget }
    }

    pub async fn summarize(
        &self,
        input: SynthesisInput,
    ) -> Result<DocumentSummary, SynthesisError> {
        let page_json = serde_json::to_string(&input.page_analyses)
            .map_err(|error| SynthesisError::InvalidSummary(error.to_string()))?;
        let full_context =
            format!("## AnyDoc Markdown\n{}\n\n## Page analyses\n{}", input.markdown, page_json);
        let synthesis_context = if self.budget.fits(&full_context) {
            full_context
        } else {
            let sections = split_context(&full_context, self.budget.maximum_tokens / 2);
            let mut summaries = Vec::with_capacity(sections.len());
            for (index, section) in sections.iter().enumerate() {
                let response = self
                    .model
                    .chat(ModelRequest {
                        messages: vec![ModelMessage {
                            role: "user".into(),
                            content: vec![ContentPart::Text {
                                text: build_section_prompt(index + 1, section),
                            }],
                        }],
                        response_schema: None,
                    })
                    .await?;
                summaries.push(format!("### 连续片段 {}\n{}", index + 1, response.content));
            }
            summaries.join("\n\n")
        };

        let response = self
            .model
            .chat(ModelRequest {
                messages: vec![ModelMessage {
                    role: "user".into(),
                    content: vec![ContentPart::Text {
                        text: build_document_prompt(&synthesis_context, input.total_pages),
                    }],
                }],
                response_schema: Some(document_summary_schema()),
            })
            .await?;
        let mut summary = parse_document_summary(&response.content, input.total_pages)?;
        if !input.vision_enabled {
            summary
                .analysis_limitations
                .push("视觉内容未分析：未启用或未配置可用的视觉模型。".into());
        }
        if !input.failed_pages.is_empty() {
            let pages =
                input.failed_pages.iter().map(u32::to_string).collect::<Vec<_>>().join("、");
            summary.analysis_limitations.push(format!("视觉分析失败页：{pages}。"));
        }
        Ok(summary)
    }
}

fn parse_document_summary(raw: &str, total_pages: u32) -> Result<DocumentSummary, SynthesisError> {
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|error| SynthesisError::InvalidSummary(error.to_string()))?;
    let schema = document_summary_schema();
    let validator = jsonschema::validator_for(&schema)
        .map_err(|error| SynthesisError::InvalidSummary(error.to_string()))?;
    validator
        .validate(&value)
        .map_err(|error| SynthesisError::InvalidSummary(error.to_string()))?;
    let summary: DocumentSummary = serde_json::from_value(value)
        .map_err(|error| SynthesisError::InvalidSummary(error.to_string()))?;
    if summary.schema_version != 1 {
        return Err(SynthesisError::InvalidSummary("schemaVersion must equal 1".into()));
    }
    if summary.theme.trim().is_empty() || summary.executive_summary.trim().is_empty() {
        return Err(SynthesisError::InvalidSummary(
            "theme and executiveSummary cannot be empty".into(),
        ));
    }
    if !summary.confidence.is_finite() || !(0.0..=1.0).contains(&summary.confidence) {
        return Err(SynthesisError::InvalidSummary("confidence must be between 0 and 1".into()));
    }
    for item in
        summary.key_facts.iter().chain(summary.risks.iter()).chain(summary.action_items.iter())
    {
        validate_evidence_pages(&item.evidence, total_pages)
            .map_err(|error| SynthesisError::InvalidSummary(error.to_string()))?;
    }
    for item in &summary.logical_outline {
        if item.page_start == 0 || item.page_start > item.page_end || item.page_end > total_pages {
            return Err(SynthesisError::InvalidSummary(format!(
                "outline range {}..={} is invalid for {total_pages} pages",
                item.page_start, item.page_end
            )));
        }
    }
    Ok(summary)
}

fn estimate_tokens(value: &str) -> usize {
    value.chars().count()
}

fn split_context(value: &str, maximum_tokens: usize) -> Vec<String> {
    let limit = maximum_tokens.max(256);
    let mut sections = Vec::new();
    let mut current = String::new();
    for line in value.lines() {
        let line_size = estimate_tokens(line) + 1;
        if !current.is_empty() && estimate_tokens(&current) + line_size > limit {
            sections.extend(split_chars(&current, limit));
            current.clear();
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.is_empty() {
        sections.extend(split_chars(&current, limit));
    }
    sections
}

fn split_chars(value: &str, limit: usize) -> Vec<String> {
    let characters = value.chars().collect::<Vec<_>>();
    characters.chunks(limit).map(|chunk| chunk.iter().collect()).collect()
}
