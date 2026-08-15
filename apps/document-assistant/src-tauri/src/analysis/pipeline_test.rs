use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;

use crate::{
    db::{DocumentPageRecord, DocumentRecord, DocumentRepository, ModelProfile, ModelRole},
    models::{ModelClient, ModelError, ModelRequest, ModelResponse},
};

use super::{AnalysisPipelineInput, VisionAnalyzer, run_analysis_pipeline};

struct QueueModel(Mutex<VecDeque<String>>);

#[async_trait]
impl ModelClient for QueueModel {
    async fn chat(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
        let content = self.0.lock().expect("queue lock").pop_front().expect("response exists");
        Ok(ModelResponse { content, model: Some("fake".into()), usage: None })
    }
}

fn model(responses: impl IntoIterator<Item = &'static str>) -> Arc<dyn ModelClient> {
    Arc::new(QueueModel(Mutex::new(responses.into_iter().map(str::to_owned).collect())))
}

#[tokio::test]
async fn persists_each_page_before_returning_the_document_summary() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("assistant.db");
    let image_path = directory.path().join("page.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 255, 255, 255]))
        .save(&image_path)
        .expect("fixture saves");
    let repo = DocumentRepository::open(&database).expect("repository opens");
    repo.upsert_document(&DocumentRecord {
        id: "doc-1".into(),
        source_path: PathBuf::from("sample.pptx"),
        file_name: "sample.pptx".into(),
        format: "pptx".into(),
        sha256: "hash-1".into(),
        status: "rendered".into(),
        markdown_path: None,
        created_at: 1,
        updated_at: 1,
    })
    .expect("document inserts");
    let page_json = r#"{"pageNumber":1,"title":"标题","summary":"图表显示增长","visualElements":[],"logicalRelations":[],"keyFacts":["增长"],"uncertainItems":[],"confidence":0.9}"#;
    let summary_json = r#"{"schemaVersion":1,"theme":"经营报告","executiveSummary":"整体增长","logicalOutline":[{"heading":"结果","summary":"增长","pageStart":1,"pageEnd":1}],"keyFacts":[{"text":"增长","evidence":[{"pageNumber":1,"excerpt":"增长"}]}],"risks":[],"actionItems":[],"analysisLimitations":[],"confidence":0.9}"#;
    let profile = ModelProfile {
        id: "vision".into(),
        role: ModelRole::Vision,
        base_url: "https://example.com/v1".into(),
        model: "vision".into(),
        supports_vision: true,
        timeout_secs: 10,
        max_concurrency: 1,
    };

    let summary = run_analysis_pipeline(AnalysisPipelineInput {
        database_path: database.clone(),
        document_id: "doc-1".into(),
        markdown: "# 报告".into(),
        pages: vec![DocumentPageRecord {
            page_number: 1,
            image_path: Some(image_path),
            width: Some(1),
            height: Some(1),
            markdown: Some("增长".into()),
            status: "completed".into(),
        }],
        vision: Some(VisionAnalyzer::new(profile, model([page_json]))),
        text_model: model([summary_json]),
    })
    .await
    .expect("analysis succeeds");

    let repo = DocumentRepository::open(&database).expect("repository reopens");
    assert_eq!(summary.theme, "经营报告");
    assert_eq!(repo.list_page_analyses("doc-1").expect("page analyses").len(), 1);
    assert!(repo.document_summary("doc-1").expect("summary loads").is_some());
    assert_eq!(repo.visual_content_analyzed("doc-1").expect("vision flag"), Some(true));
}
