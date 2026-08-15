use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};

use super::{
    model::{PageCheckpoint, TaskRecord, TaskStage},
    repository::TaskRepository,
};

pub trait PageProcessor: Send + Sync {
    fn process(&self, task: &TaskRecord, page_number: u32) -> Result<PageCheckpoint>;
}

pub trait ProgressSink: Send + Sync {
    fn emit(&self, task: &TaskRecord) -> Result<()>;
}

pub struct TaskRunner {
    repository: Mutex<TaskRepository>,
    processor: Arc<dyn PageProcessor>,
    progress: Arc<dyn ProgressSink>,
}

impl TaskRunner {
    pub fn new(
        repository: TaskRepository,
        processor: Arc<dyn PageProcessor>,
        progress: Arc<dyn ProgressSink>,
    ) -> Self {
        Self { repository: Mutex::new(repository), processor, progress }
    }

    pub fn resume(&self, task_id: &str) -> Result<TaskRecord> {
        let mut task = self.task(task_id)?;
        if task.stage == TaskStage::Paused {
            let resume_stage =
                if task.total == 0 { TaskStage::Parsing } else { TaskStage::Rendering };
            task = self.set_stage(task_id, resume_stage, "继续处理")?;
            self.progress.emit(&task)?;
        }
        if task.stage == TaskStage::Queued {
            task = self.set_stage(task_id, TaskStage::Parsing, "正在准备文档")?;
            self.progress.emit(&task)?;
        }
        if task.stage == TaskStage::Parsing {
            task = self.set_stage(task_id, TaskStage::Rendering, "正在处理页面")?;
            self.progress.emit(&task)?;
        }
        let completed = self
            .repository
            .lock()
            .map_err(|_| anyhow::anyhow!("task repository lock poisoned"))?
            .completed_pages(task_id)?
            .into_iter()
            .collect::<HashSet<_>>();

        for page_number in 1..=task.total {
            if completed.contains(&page_number) {
                continue;
            }
            task = self.task(task_id)?;
            if matches!(task.stage, TaskStage::Paused | TaskStage::Cancelled) {
                return Ok(task);
            }

            match self.processor.process(&task, page_number) {
                Ok(page) => {
                    task = self
                        .repository
                        .lock()
                        .map_err(|_| anyhow::anyhow!("task repository lock poisoned"))?
                        .checkpoint_page(task_id, &page)?;
                    self.progress.emit(&task)?;
                }
                Err(error) => {
                    let mut repository = self
                        .repository
                        .lock()
                        .map_err(|_| anyhow::anyhow!("task repository lock poisoned"))?;
                    repository.mark_page_failed(task_id, page_number)?;
                    let failed = repository.fail_task(task_id, &error.to_string())?;
                    self.progress.emit(&failed)?;
                    return Err(error).with_context(|| format!("process page {page_number}"));
                }
            }
        }

        task = self.set_stage(task_id, TaskStage::Completed, "处理完成")?;
        self.progress.emit(&task)?;
        Ok(task)
    }

    pub fn resume_pending(&self) -> Result<Vec<TaskRecord>> {
        let task_ids = self
            .repository
            .lock()
            .map_err(|_| anyhow::anyhow!("task repository lock poisoned"))?
            .list_recoverable()?
            .into_iter()
            .filter(|task| !matches!(task.stage, TaskStage::Failed | TaskStage::Paused))
            .map(|task| task.id)
            .collect::<Vec<_>>();
        task_ids.into_iter().map(|task_id| self.resume(&task_id)).collect()
    }

    pub fn task(&self, task_id: &str) -> Result<TaskRecord> {
        self.repository
            .lock()
            .map_err(|_| anyhow::anyhow!("task repository lock poisoned"))?
            .get_task(task_id)?
            .with_context(|| format!("task not found: {task_id}"))
    }

    fn set_stage(&self, task_id: &str, stage: TaskStage, message: &str) -> Result<TaskRecord> {
        self.repository
            .lock()
            .map_err(|_| anyhow::anyhow!("task repository lock poisoned"))?
            .set_stage(task_id, stage, message)
    }
}
