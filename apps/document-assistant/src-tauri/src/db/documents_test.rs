use std::path::PathBuf;

use super::{DocumentRecord, DocumentRepository};
use crate::tasks::{PageCheckpoint, TaskRecord, TaskRepository, TaskStage};

fn sample_document(id: &str, sha256: &str, source_path: PathBuf) -> DocumentRecord {
    DocumentRecord {
        id: id.into(),
        source_path,
        file_name: "季度 报告.docx".into(),
        format: "docx".into(),
        sha256: sha256.into(),
        status: "parsed".into(),
        markdown_path: None,
        created_at: 1_723_700_000,
        updated_at: 1_723_700_001,
    }
}

#[test]
fn upsert_by_hash_deduplicates_documents() {
    let repo = DocumentRepository::open_in_memory().expect("repository opens");
    repo.upsert_document(&sample_document(
        "doc-1",
        "same-hash",
        PathBuf::from(r"D:\项目 文档\季度 报告.docx"),
    ))
    .expect("first insert");
    repo.upsert_document(&sample_document(
        "doc-2",
        "same-hash",
        PathBuf::from(r"D:\另一个目录\副本.docx"),
    ))
    .expect("second insert");

    let documents = repo.list_documents().expect("documents list");
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].source_path, PathBuf::from(r"D:\另一个目录\副本.docx"));
}

#[test]
fn deleting_history_does_not_delete_source_file() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("原始 文档.docx");
    std::fs::write(&source, b"original bytes").expect("source fixture writes");
    let repo = DocumentRepository::open_in_memory().expect("repository opens");
    repo.upsert_document(&sample_document("doc-1", "hash-1", source.clone()))
        .expect("document inserts");

    repo.delete_cache_record("doc-1").expect("cache record deletes");

    assert!(source.exists());
    assert!(repo.get_document("doc-1").expect("lookup succeeds").is_none());
}

#[test]
fn persists_success_and_failure_for_individual_page_analyses() {
    let repo = DocumentRepository::open_in_memory().expect("repository opens");
    repo.upsert_document(&sample_document("doc-1", "hash-1", PathBuf::from("slides.pptx")))
        .expect("document inserts");

    repo.save_page_analysis("doc-1", 1, r#"{"summary":"ok"}"#).expect("analysis saves");
    repo.save_page_analysis_failure("doc-1", 2, "not-json", "schema mismatch")
        .expect("failure saves");
    repo.set_visual_content_analyzed("doc-1", false).expect("capability saves");
    repo.save_document_summary("doc-1", r#"{"schemaVersion":1}"#).expect("summary saves");
    let records = repo.list_page_analyses("doc-1").expect("analyses list");

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].status, "completed");
    assert_eq!(records[1].raw_response.as_deref(), Some("not-json"));
    assert_eq!(repo.visual_content_analyzed("doc-1").expect("capability reads"), Some(false));
    assert_eq!(
        repo.document_summary("doc-1").expect("summary reads").as_deref(),
        Some(r#"{"schemaVersion":1}"#)
    );
}

#[test]
fn loads_rendered_pages_and_persists_remote_analysis_consent() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("assistant.db");
    let repo = DocumentRepository::open(&database).expect("repository opens");
    repo.upsert_document(&sample_document("doc-1", "hash-1", PathBuf::from("slides.pptx")))
        .expect("document inserts");
    let mut tasks = TaskRepository::open(&database).expect("task repository opens");
    tasks
        .create_task(&TaskRecord::new("task-1", "doc-1", TaskStage::Rendering, 1))
        .expect("task creates");
    tasks
        .checkpoint_page(
            "task-1",
            &PageCheckpoint {
                page_number: 1,
                image_path: Some(PathBuf::from("page-0001.png")),
                width: Some(1600),
                height: Some(900),
                markdown: Some("本页文本".into()),
            },
        )
        .expect("page saves");

    repo.record_analysis_consent("doc-1", Some("vision-primary"), "text-primary")
        .expect("consent saves");
    let pages = repo.list_pages("doc-1").expect("pages load");
    let consent = repo.analysis_consent("doc-1").expect("consent loads").expect("consent exists");

    assert_eq!(pages[0].markdown.as_deref(), Some("本页文本"));
    assert_eq!(consent.vision_profile_id.as_deref(), Some("vision-primary"));
    assert_eq!(consent.text_profile_id, "text-primary");
    assert!(consent.consent_at > 0);
}
