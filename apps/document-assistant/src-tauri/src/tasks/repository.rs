use std::{path::Path, str::FromStr};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};

use super::model::{PageCheckpoint, TaskRecord, TaskStage, unix_timestamp};

const INITIAL_MIGRATION: &str = include_str!("../../migrations/0001_init.sql");

pub struct TaskRepository {
    connection: Connection,
}

impl TaskRepository {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)
            .with_context(|| format!("open task database {}", path.display()))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;",
        )?;
        connection.execute_batch(INITIAL_MIGRATION)?;
        Ok(Self { connection })
    }

    pub fn create_task(&mut self, task: &TaskRecord) -> Result<()> {
        self.connection.execute(
            "INSERT INTO tasks (
                id, document_id, stage, completed, total, message, error, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                task.id,
                task.document_id,
                task.stage.to_string(),
                task.completed,
                task.total,
                task.message,
                task.error,
                task.created_at,
                task.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_task(&self, task_id: &str) -> Result<Option<TaskRecord>> {
        self.connection
            .query_row(
                "SELECT t.id, t.document_id, t.stage, t.completed, t.total,
                        t.message, t.error, t.created_at, t.updated_at,
                        (SELECT COUNT(*) FROM pages p
                         WHERE p.document_id = t.document_id AND p.status = 'failed')
                 FROM tasks t WHERE t.id = ?1",
                [task_id],
                task_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_recoverable(&self) -> Result<Vec<TaskRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT t.id, t.document_id, t.stage, t.completed, t.total,
                    t.message, t.error, t.created_at, t.updated_at,
                    (SELECT COUNT(*) FROM pages p
                     WHERE p.document_id = t.document_id AND p.status = 'failed')
             FROM tasks t
             WHERE t.stage NOT IN ('completed', 'cancelled')
             ORDER BY t.updated_at ASC",
        )?;
        Ok(statement.query_map([], task_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_stage(
        &mut self,
        task_id: &str,
        stage: TaskStage,
        message: impl AsRef<str>,
    ) -> Result<TaskRecord> {
        let current =
            self.get_task(task_id)?.with_context(|| format!("task not found: {task_id}"))?;
        current.stage.transition_to(stage)?;
        self.connection.execute(
            "UPDATE tasks SET stage = ?2, message = ?3, error = NULL, updated_at = ?4
             WHERE id = ?1",
            params![task_id, stage.to_string(), message.as_ref(), unix_timestamp()],
        )?;
        self.get_task(task_id)?.context("task disappeared after stage update")
    }

    pub fn fail_task(&mut self, task_id: &str, error: &str) -> Result<TaskRecord> {
        let current =
            self.get_task(task_id)?.with_context(|| format!("task not found: {task_id}"))?;
        current.stage.transition_to(TaskStage::Failed)?;
        self.connection.execute(
            "UPDATE tasks SET stage = 'failed', message = '处理失败', error = ?2, updated_at = ?3
             WHERE id = ?1",
            params![task_id, error, unix_timestamp()],
        )?;
        self.get_task(task_id)?.context("task disappeared after failure update")
    }

    pub fn set_total(&mut self, task_id: &str, total: u32) -> Result<TaskRecord> {
        self.connection.execute(
            "UPDATE tasks SET total = ?2, updated_at = ?3 WHERE id = ?1",
            params![task_id, total, unix_timestamp()],
        )?;
        self.get_task(task_id)?.context("task disappeared after total update")
    }

    pub fn completed_pages(&self, task_id: &str) -> Result<Vec<u32>> {
        let mut statement = self.connection.prepare(
            "SELECT p.page_number
             FROM pages p JOIN tasks t ON t.document_id = p.document_id
             WHERE t.id = ?1 AND p.status = 'completed'
             ORDER BY p.page_number",
        )?;
        Ok(statement
            .query_map([task_id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn checkpoint_page(&mut self, task_id: &str, page: &PageCheckpoint) -> Result<TaskRecord> {
        let task = self.get_task(task_id)?.with_context(|| format!("task not found: {task_id}"))?;
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO pages (
                document_id, page_number, image_path, width, height, markdown, status
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'completed')
             ON CONFLICT(document_id, page_number) DO UPDATE SET
                image_path = excluded.image_path,
                width = excluded.width,
                height = excluded.height,
                markdown = COALESCE(excluded.markdown, pages.markdown),
                status = 'completed'",
            params![
                task.document_id,
                page.page_number,
                page.image_path.as_ref().map(|path| path.to_string_lossy().into_owned()),
                page.width,
                page.height,
                page.markdown,
            ],
        )?;
        let completed: u32 = transaction.query_row(
            "SELECT COUNT(*) FROM pages WHERE document_id = ?1 AND status = 'completed'",
            [&task.document_id],
            |row| row.get(0),
        )?;
        transaction.execute(
            "UPDATE tasks SET completed = ?2, message = ?3, updated_at = ?4 WHERE id = ?1",
            params![
                task_id,
                completed,
                format!("已完成第 {} 页", page.page_number),
                unix_timestamp(),
            ],
        )?;
        transaction.commit()?;
        self.get_task(task_id)?.context("task disappeared after page checkpoint")
    }

    pub fn mark_page_failed(&mut self, task_id: &str, page_number: u32) -> Result<()> {
        let task = self.get_task(task_id)?.with_context(|| format!("task not found: {task_id}"))?;
        self.connection.execute(
            "INSERT INTO pages (document_id, page_number, status)
             VALUES (?1, ?2, 'failed')
             ON CONFLICT(document_id, page_number) DO UPDATE SET status = 'failed'",
            params![task.document_id, page_number],
        )?;
        Ok(())
    }

    pub fn reset_failed_pages(&mut self, task_id: &str) -> Result<TaskRecord> {
        let task = self.get_task(task_id)?.with_context(|| format!("task not found: {task_id}"))?;
        self.connection.execute(
            "UPDATE pages SET status = 'pending'
             WHERE document_id = ?1 AND status = 'failed'",
            [&task.document_id],
        )?;
        self.set_stage(task_id, TaskStage::Queued, "准备重试失败页")
    }
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRecord> {
    let stage: String = row.get(2)?;
    Ok(TaskRecord {
        id: row.get(0)?,
        document_id: row.get(1)?,
        stage: TaskStage::from_str(&stage).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::other(error.to_string())),
            )
        })?,
        completed: row.get(3)?,
        total: row.get(4)?,
        message: row.get(5)?,
        error: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        failed_pages: row.get(9)?,
    })
}
