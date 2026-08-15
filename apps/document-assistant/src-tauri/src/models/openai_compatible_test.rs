use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

use crate::db::{ModelProfile, ModelRole};

use super::{
    OpenAiCompatibleClient,
    types::{ContentPart, ModelClient, ModelMessage, ModelRequest},
};

fn sample_request() -> ModelRequest {
    ModelRequest {
        messages: vec![ModelMessage {
            role: "user".into(),
            content: vec![ContentPart::Text { text: "Return JSON".into() }],
        }],
        response_schema: Some(json!({"type": "object"})),
    }
}

fn profile(server: &MockServer) -> ModelProfile {
    ModelProfile {
        id: "text-primary".into(),
        role: ModelRole::Text,
        base_url: format!("{}/v1/", server.uri()),
        model: "compatible-model".into(),
        supports_vision: false,
        timeout_secs: 5,
        max_concurrency: 2,
    }
}

#[tokio::test]
async fn sends_openai_compatible_chat_request_without_leaking_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header("authorization", "Bearer secret-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "compatible-model",
            "choices": [{"message": {"content": "{\"ok\":true}"}}]
        })))
        .mount(&server)
        .await;
    let client =
        OpenAiCompatibleClient::new(profile(&server), "secret-key".into()).expect("client builds");

    let response = client.chat(sample_request()).await.expect("chat succeeds");

    assert_eq!(response.content, r#"{"ok":true}"#);
    assert!(!format!("{response:?}").contains("secret-key"));
}

#[tokio::test]
async fn does_not_retry_an_authorization_failure_or_expose_the_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string("key secret-key rejected"))
        .expect(1)
        .mount(&server)
        .await;
    let client =
        OpenAiCompatibleClient::new(profile(&server), "secret-key".into()).expect("client builds");

    let error = client.chat(sample_request()).await.expect_err("401 fails");

    assert!(error.to_string().contains("401"));
    assert!(!format!("{error:?}").contains("secret-key"));
}

#[tokio::test]
async fn retries_a_rate_limit_with_zero_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"content": "ok"}}]
        })))
        .mount(&server)
        .await;
    let client =
        OpenAiCompatibleClient::new(profile(&server), "secret-key".into()).expect("client builds");

    let response = client.chat(sample_request()).await.expect("retry succeeds");

    assert_eq!(response.content, "ok");
    assert_eq!(server.received_requests().await.expect("requests").len(), 2);
}

#[tokio::test]
async fn stops_after_three_transient_server_failures() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("temporary failure"))
        .expect(3)
        .mount(&server)
        .await;
    let client =
        OpenAiCompatibleClient::new(profile(&server), "secret-key".into()).expect("client builds");

    let error = client.chat(sample_request()).await.expect_err("retry budget is bounded");

    assert!(error.to_string().contains("500"));
    assert_eq!(server.received_requests().await.expect("requests").len(), 3);
}
