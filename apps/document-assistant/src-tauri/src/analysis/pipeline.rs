use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};

use crate::{
    db::{DocumentPageRecord, DocumentRepository},
    models::ModelClient,
};

use super::{
    synthesis::{ContextBudget, DocumentSynthesizer, SynthesisInput},
    types::{DocumentSummary, PageAnalysis},
    vision::{AnalysisError, PageAnalysisInput, VisionAnalyzer},
};

pub struct AnalysisPipelineInput {
    pub database_path: PathBuf,
    pub document_id: String,
    pub markdown: String,
    pub pages: Vec<DocumentPageRecord>,
    pub vision: Option<VisionAnalyzer>,
    pub text_model: Arc<dyn ModelClient>,
}

pub async fn run_analysis_pipeline(input: AnalysisPipelineInput) -> Result<DocumentSummary> {
    let total_pages = input.pages.iter().map(|page| page.page_number).max().unwrap_or(1);
    let vision_enabled = input.vision.is_some();
    let mut successful = Vec::<PageAnalysis>::new();
    let mut failed_pages = Vec::new();

    if let Some(analyzer) = input.vision {
        let mut vision_inputs = Vec::new();
        for page in &input.pages {
            match &page.image_path {
                Some(image_path) => vision_inputs.push(PageAnalysisInput {
                    page_number: page.page_number,
                    image_path: image_path.clone(),
                    page_text: page.markdown.clone().unwrap_or_default(),
                    previous_heading: None,
                    next_heading: None,
                }),
                None => {
                    failed_pages.push(page.page_number);
                    DocumentRepository::open(&input.database_path)?.save_page_analysis_failure(
                        &input.document_id,
                        page.page_number,
                        "",
                        "页面图像不存在",
                    )?;
                }
            }
        }

        for (page_number, result) in analyzer.analyze_pages(vision_inputs).await {
            let repository = DocumentRepository::open(&input.database_path)?;
            match result {
                Ok(analysis) => {
                    let json = serde_json::to_string(&analysis)?;
                    repository.save_page_analysis(&input.document_id, page_number, &json)?;
                    successful.push(analysis);
                }
                Err(error) => {
                    failed_pages.push(page_number);
                    let raw = match &error {
                        AnalysisError::InvalidResponse { raw_response, .. } => {
                            raw_response.as_str()
                        }
                        _ => "",
                    };
                    repository.save_page_analysis_failure(
                        &input.document_id,
                        page_number,
                        raw,
                        &error.to_string(),
                    )?;
                }
            }
        }
    }

    failed_pages.sort_unstable();
    failed_pages.dedup();
    DocumentRepository::open(&input.database_path)?
        .set_visual_content_analyzed(&input.document_id, vision_enabled)?;

    let synthesizer = DocumentSynthesizer::new(input.text_model, ContextBudget::new(50_000));
    let summary = synthesizer
        .summarize(SynthesisInput {
            markdown: input.markdown,
            page_analyses: successful,
            total_pages,
            vision_enabled,
            failed_pages,
        })
        .await
        .context("synthesize document analysis")?;
    let summary_json = serde_json::to_string(&summary)?;
    DocumentRepository::open(&input.database_path)?
        .save_document_summary(&input.document_id, &summary_json)?;
    Ok(summary)
}
