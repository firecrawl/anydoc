use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;

use crate::{
    models::{ModelClient, ModelError, ModelRequest, ModelResponse},
    search::{PageIndexEntry, SearchIndex},
};

use super::{ChatError, ChatService};

struct QueueModel {
    responses: Mutex<VecDeque<String>>,
    calls: Mutex<usize>,
}

#[async_trait]
impl ModelClient for QueueModel {
    async fn chat(&self, _request: ModelRequest) -> Result<ModelResponse, ModelError> {
        *self.calls.lock().expect("calls lock") += 1;
        Ok(ModelResponse {
            content: self.responses.lock().expect("responses lock").pop_front().expect("response"),
            model: None,
            usage: None,
        })
    }
}

fn model(response: &str) -> Arc<QueueModel> {
    Arc::new(QueueModel {
        responses: Mutex::new(VecDeque::from([response.to_owned()])),
        calls: Mutex::new(0),
    })
}

#[tokio::test]
async fn returns_not_found_when_retrieval_has_no_evidence() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let search = SearchIndex::open(&directory.path().join("chat.db")).expect("search opens");
    let model = model(r#"{"answer":"猜测","citations":[],"grounded":false}"#);
    let chat = ChatService::new(search, model.clone());

    let answer = chat.ask("doc-1", "董事长生日？", &[]).await.expect("answer returns");
    assert_eq!(answer.answer, "文档中未找到相关信息");
    assert!(!answer.grounded);
    assert_eq!(*model.calls.lock().expect("calls lock"), 0);
}

#[tokio::test]
async fn rejects_citations_outside_supplied_context() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let search = SearchIndex::open(&directory.path().join("chat.db")).expect("search opens");
    search
        .index_document_pages("doc-1", &[PageIndexEntry::text(2, "收入增长来自西南区域")])
        .expect("indexes");
    let model = model(
        r#"{"answer":"来自西南区域","citations":[{"pageNumber":99,"excerpt":"西南"}],"grounded":true}"#,
    );
    let chat = ChatService::new(search, model);

    let result = chat.ask("doc-1", "增长来源？", &[]).await;
    assert!(matches!(result, Err(ChatError::InvalidCitation(99))));
}
