use crate::analysis::{CitedFact, DocumentSummary, EvidenceRef};

use super::{ExportMetadata, ExportPayload, render_enhanced_markdown, render_json};

fn sample_export() -> ExportPayload {
    ExportPayload {
        schema_version: 1,
        metadata: ExportMetadata {
            file_name: "季度报告.pptx".into(),
            format: "pptx".into(),
            analysis_at: 1_787_000_000,
            vision_model: Some("vision-model".into()),
            text_model: "text-model".into(),
        },
        original_markdown: "# 原始内容".into(),
        summary: DocumentSummary {
            schema_version: 1,
            theme: "经营复盘".into(),
            executive_summary: "收入增长".into(),
            logical_outline: vec![],
            key_facts: vec![CitedFact {
                text: "收入增长".into(),
                evidence: vec![EvidenceRef {
                    page_number: 8, excerpt: Some("同比增长".into())
                }],
            }],
            risks: vec![],
            action_items: vec![],
            analysis_limitations: vec!["第 13 页视觉分析失败".into()],
            confidence: 0.8,
        },
        page_analyses: vec![],
        failed_pages: vec![13],
    }
}

#[test]
fn enhanced_markdown_includes_citations_failures_and_model_metadata() {
    let output = render_enhanced_markdown(&sample_export()).expect("markdown renders");
    assert!(output.contains("来源：第 8 页"));
    assert!(output.contains("失败页面：第 13 页"));
    assert!(output.contains("视觉模型：vision-model"));
}

#[test]
fn exported_json_never_contains_credentials() {
    let output = render_json(&sample_export()).expect("json renders");
    assert!(!output.to_lowercase().contains("api_key"));
    assert!(!output.contains("sk-test"));
    assert!(output.contains("\"schemaVersion\": 1"));
}
