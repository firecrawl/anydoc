use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;

use crate::models::{ModelClient, ModelError, ModelRequest, ModelResponse};

use super::synthesis::{ContextBudget, DocumentSynthesizer, SynthesisInput};

struct RecordingModel {
    calls: Mutex<Vec<ModelRequest>>,
    response: String,
}

#[async_trait]
impl ModelClient for RecordingModel {
    async fn chat(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.calls.lock().expect("calls lock").push(request);
        Ok(ModelResponse {
            content: self.response.clone(),
            model: Some("text-model".into()),
            usage: None,
        })
    }
}

fn valid_summary() -> String {
    json!({
        "schemaVersion": 1,
        "theme": "季度经营",
        "executiveSummary": "收入增长，但成本存在风险。",
        "logicalOutline": [{
            "heading": "经营结果",
            "summary": "收入和成本概况",
            "pageStart": 1,
            "pageEnd": 2
        }],
        "keyFacts": [{"text": "收入增长", "evidence": [{"pageNumber": 1, "excerpt": null}]}],
        "risks": [{"text": "成本上升", "evidence": [{"pageNumber": 2, "excerpt": null}]}],
        "actionItems": [],
        "analysisLimitations": [],
        "confidence": 0.8
    })
    .to_string()
}

fn input(markdown: String) -> SynthesisInput {
    SynthesisInput {
        markdown,
        page_analyses: Vec::new(),
        total_pages: 2,
        vision_enabled: true,
        failed_pages: Vec::new(),
    }
}

#[tokio::test]
async fn short_documents_use_one_synthesis_request() {
    let model =
        Arc::new(RecordingModel { calls: Mutex::new(Vec::new()), response: valid_summary() });
    let synthesizer = DocumentSynthesizer::new(model.clone(), ContextBudget::new(50_000));

    synthesizer.summarize(input("# 概览\n短文档".into())).await.expect("summary succeeds");

    assert_eq!(model.calls.lock().expect("calls lock").len(), 1);
}

#[tokio::test]
async fn long_documents_create_section_summaries_before_final_summary() {
    let model =
        Arc::new(RecordingModel { calls: Mutex::new(Vec::new()), response: valid_summary() });
    let synthesizer = DocumentSynthesizer::new(model.clone(), ContextBudget::new(2_000));
    let markdown =
        format!("# 第一部分\n{}\n# 第二部分\n{}", "内容".repeat(1800), "信息".repeat(1800));

    synthesizer.summarize(input(markdown)).await.expect("hierarchical summary succeeds");

    assert!(model.calls.lock().expect("calls lock").len() > 1);
}

#[tokio::test]
async fn discloses_disabled_vision_and_exact_failed_pages() {
    let model =
        Arc::new(RecordingModel { calls: Mutex::new(Vec::new()), response: valid_summary() });
    let synthesizer = DocumentSynthesizer::new(model, ContextBudget::new(50_000));
    let mut request = input("正文".into());
    request.vision_enabled = false;
    request.failed_pages = vec![2, 4];
    request.total_pages = 4;

    let summary = synthesizer.summarize(request).await.expect("summary succeeds");

    assert!(summary.analysis_limitations.iter().any(|item| item.contains("视觉内容未分析")));
    assert!(summary.analysis_limitations.iter().any(|item| item.contains("2、4")));
}
