use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::Result;

use super::{
    model::{PageCheckpoint, TaskRecord, TaskStage},
    repository::TaskRepository,
    runner::{PageProcessor, ProgressSink, TaskRunner},
};
use crate::db::{DocumentRecord, DocumentRepository};

#[test]
fn rejects_skipping_from_queued_to_completed() {
    assert!(TaskStage::Queued.transition_to(TaskStage::Completed).is_err());
    assert!(TaskStage::Queued.transition_to(TaskStage::Parsing).is_ok());
}

#[derive(Default)]
struct RecordingProcessor {
    requested: Mutex<Vec<u32>>,
}

impl PageProcessor for RecordingProcessor {
    fn process(&self, _task: &TaskRecord, page_number: u32) -> Result<PageCheckpoint> {
        self.requested.lock().expect("requested pages lock").push(page_number);
        Ok(PageCheckpoint {
            page_number,
            image_path: Some(PathBuf::from(format!("page-{page_number:04}.png"))),
            width: Some(1600),
            height: Some(900),
            markdown: None,
        })
    }
}

#[derive(Default)]
struct RecordingSink {
    completed: Mutex<Vec<u32>>,
}

impl ProgressSink for RecordingSink {
    fn emit(&self, task: &TaskRecord) -> Result<()> {
        self.completed.lock().expect("progress lock").push(task.completed);
        Ok(())
    }
}

#[test]
fn resume_skips_pages_already_persisted() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("tasks.db");
    insert_document(&database);
    let mut repository = TaskRepository::open(&database).expect("task repository opens");
    repository
        .create_task(&TaskRecord::new("task-1", "doc-1", TaskStage::Rendering, 4))
        .expect("task creates");
    for page_number in [1, 2] {
        repository
            .checkpoint_page(
                "task-1",
                &PageCheckpoint {
                    page_number,
                    image_path: Some(PathBuf::from(format!("page-{page_number:04}.png"))),
                    width: Some(1600),
                    height: Some(900),
                    markdown: None,
                },
            )
            .expect("page checkpoint saves");
    }
    let processor = Arc::new(RecordingProcessor::default());
    let sink = Arc::new(RecordingSink::default());
    let runner = TaskRunner::new(repository, processor.clone(), sink.clone());

    let finished = runner.resume("task-1").expect("task resumes");

    assert_eq!(*processor.requested.lock().expect("requested pages lock"), vec![3, 4]);
    assert_eq!(finished.stage, TaskStage::Completed);
    assert_eq!(finished.completed, 4);
    assert_eq!(*sink.completed.lock().expect("progress lock"), vec![3, 4, 4]);
}

#[test]
fn resume_advances_a_queued_zero_page_task_through_legal_stages() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = directory.path().join("queued.db");
    insert_document(&database);
    let mut repository = TaskRepository::open(&database).expect("task repository opens");
    repository
        .create_task(&TaskRecord::new("task-empty", "doc-1", TaskStage::Queued, 0))
        .expect("task creates");
    let runner = TaskRunner::new(
        repository,
        Arc::new(RecordingProcessor::default()),
        Arc::new(RecordingSink::default()),
    );

    let finished = runner.resume("task-empty").expect("queued task resumes");

    assert_eq!(finished.stage, TaskStage::Completed);
    assert_eq!(finished.completed, 0);
}

fn insert_document(database: &Path) {
    let repository = DocumentRepository::open(database).expect("document repository opens");
    repository
        .upsert_document(&DocumentRecord {
            id: "doc-1".into(),
            source_path: PathBuf::from("sample.pptx"),
            file_name: "sample.pptx".into(),
            format: "pptx".into(),
            sha256: "same-hash".into(),
            status: "registered".into(),
            markdown_path: None,
            created_at: 1,
            updated_at: 1,
        })
        .expect("document inserts");
}
