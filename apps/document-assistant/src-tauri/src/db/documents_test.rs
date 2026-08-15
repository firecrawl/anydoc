use std::path::PathBuf;

use super::{DocumentRecord, DocumentRepository};

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
