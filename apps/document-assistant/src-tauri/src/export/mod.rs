mod json;
mod markdown;

use serde::{Deserialize, Serialize};

use crate::analysis::{DocumentSummary, PageAnalysis};

pub use json::render_json;
pub use markdown::render_enhanced_markdown;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportMetadata {
    pub file_name: String,
    pub format: String,
    pub analysis_at: i64,
    pub vision_model: Option<String>,
    pub text_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPayload {
    pub schema_version: u32,
    pub metadata: ExportMetadata,
    pub original_markdown: String,
    pub summary: DocumentSummary,
    pub page_analyses: Vec<PageAnalysis>,
    pub failed_pages: Vec<u32>,
}

#[cfg(test)]
mod export_test;
