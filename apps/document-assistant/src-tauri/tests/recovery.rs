use anydoc_assistant_lib::{
    db::{DocumentRecord, DocumentRepository},
    tasks::{PageCheckpoint, TaskRecord, TaskRepository, TaskStage},
};

#[test]
fn restart_offers_resume_without_repeating_completed_pages() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("recovery.db");
    let documents = DocumentRepository::open(&database).expect("documents open");
    documents
        .upsert_document(&DocumentRecord {
            id: "doc-1".into(),
            source_path: "sample.pdf".into(),
            file_name: "sample.pdf".into(),
            format: "pdf".into(),
            sha256: "hash".into(),
            status: "rendering".into(),
            markdown_path: None,
            created_at: 1,
            updated_at: 1,
        })
        .expect("document saves");
    let mut tasks = TaskRepository::open(&database).expect("tasks open");
    tasks
        .create_task(&TaskRecord::new("task-1", "doc-1", TaskStage::Rendering, 4))
        .expect("task creates");
    for page_number in [1, 2] {
        tasks
            .checkpoint_page(
                "task-1",
                &PageCheckpoint {
                    page_number,
                    image_path: None,
                    width: None,
                    height: None,
                    markdown: Some(format!("第 {page_number} 页")),
                },
            )
            .expect("page checkpoints");
    }
    let reopened = TaskRepository::open(&database).expect("tasks reopen");
    let pending = reopened.list_recoverable().expect("recoverable tasks");
    assert_eq!(pending[0].completed, 2);
    assert_eq!(reopened.completed_pages("task-1").expect("completed pages"), vec![1, 2]);
}
