use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::json;

use crate::{
    db::{ModelProfile, ModelRole},
    models::{ContentPart, ModelClient, ModelError, ModelRequest, ModelResponse},
};

use super::{
    prompts::build_page_prompt,
    vision::{AnalysisError, PageAnalysisInput, VisionAnalyzer},
};

#[derive(Default)]
struct RecordingModel {
    requests: Mutex<Vec<ModelRequest>>,
    responses: Mutex<VecDeque<String>>,
}

#[async_trait]
impl ModelClient for RecordingModel {
    async fn chat(&self, request: ModelRequest) -> Result<ModelResponse, ModelError> {
        self.requests.lock().expect("requests lock").push(request);
        let content =
            self.responses.lock().expect("responses lock").pop_front().expect("prepared response");
        Ok(ModelResponse { content, model: Some("test".into()), usage: None })
    }
}

fn profile(supports_vision: bool) -> ModelProfile {
    ModelProfile {
        id: "vision-primary".into(),
        role: ModelRole::Vision,
        base_url: "https://api.example.com/v1".into(),
        model: "vision-model".into(),
        supports_vision,
        timeout_secs: 60,
        max_concurrency: 2,
    }
}

fn input(image_path: PathBuf) -> PageAnalysisInput {
    PageAnalysisInput {
        page_number: 2,
        image_path,
        page_text: "收入同比增长 20%".into(),
        previous_heading: Some("经营情况".into()),
        next_heading: Some("风险提示".into()),
    }
}

fn valid_analysis() -> String {
    json!({
        "pageNumber": 2,
        "title": "经营情况",
        "summary": "收入同比增长。",
        "visualElements": [],
        "logicalRelations": [],
        "keyFacts": ["收入同比增长 20%"],
        "uncertainItems": [],
        "confidence": 0.9
    })
    .to_string()
}

#[tokio::test]
async fn refuses_to_send_images_to_text_only_profiles() {
    let model = Arc::new(RecordingModel::default());
    let analyzer = VisionAnalyzer::new(profile(false), model.clone());

    let error = analyzer
        .analyze_page(input(PathBuf::from("missing.png")))
        .await
        .expect_err("text-only profile is rejected");

    assert!(matches!(error, AnalysisError::VisionUnsupported));
    assert!(model.requests.lock().expect("requests lock").is_empty());
}

#[test]
fn prompt_requires_uncertainty_instead_of_guessing() {
    let prompt = build_page_prompt(&input(PathBuf::from("page.png")));
    assert!(prompt.contains("无法确定"));
    assert!(prompt.contains("uncertain_items"));
}

#[tokio::test]
async fn invalid_json_gets_one_text_only_repair_pass() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let image_path = directory.path().join("page.png");
    image::RgbaImage::new(2, 2).save(&image_path).expect("PNG saves");
    let model = Arc::new(RecordingModel {
        requests: Mutex::new(Vec::new()),
        responses: Mutex::new(VecDeque::from(["not-json".into(), valid_analysis()])),
    });
    let analyzer = VisionAnalyzer::new(profile(true), model.clone());

    let analysis = analyzer.analyze_page(input(image_path)).await.expect("repair succeeds");
    let requests = model.requests.lock().expect("requests lock");

    assert_eq!(analysis.page_number, 2);
    assert_eq!(requests.len(), 2);
    assert!(
        requests[0].messages[0]
            .content
            .iter()
            .any(|part| matches!(part, ContentPart::ImageUrl { .. }))
    );
    assert!(
        !requests[1].messages[0]
            .content
            .iter()
            .any(|part| matches!(part, ContentPart::ImageUrl { .. }))
    );
}
