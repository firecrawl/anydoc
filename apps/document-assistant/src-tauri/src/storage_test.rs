use std::path::Path;

use crate::storage::{cache_paths, compute_document_id, write_atomic};

#[test]
fn same_bytes_produce_same_document_id() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let first = directory.path().join("a.docx");
    let second = directory.path().join("副本 文档.docx");
    std::fs::write(&first, b"same bytes").expect("first fixture writes");
    std::fs::write(&second, b"same bytes").expect("second fixture writes");

    assert_eq!(
        compute_document_id(&first).expect("first identity").id,
        compute_document_id(&second).expect("second identity").id,
    );
}

#[test]
fn cache_paths_remain_under_app_data() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let app_data = directory.path().join("AnyDoc Assistant 数据");

    let paths = cache_paths(&app_data, "abc123").expect("cache paths");

    assert!(paths.root.starts_with(&app_data));
    assert!(paths.pages.starts_with(&paths.root));
    assert!(paths.assets.starts_with(&paths.root));
    assert!(Path::new(&paths.root).exists());
}

#[test]
fn cache_paths_reject_traversal_ids() {
    let directory = tempfile::tempdir().expect("temporary directory");
    assert!(cache_paths(directory.path(), "..\\outside").is_err());
    assert!(cache_paths(directory.path(), "../outside").is_err());
}

#[test]
fn atomic_write_replaces_a_derived_file() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let output = directory.path().join("document.md");
    std::fs::write(&output, b"old").expect("old output writes");

    write_atomic(&output, b"new markdown").expect("atomic write succeeds");

    assert_eq!(std::fs::read(&output).expect("output reads"), b"new markdown");
}
