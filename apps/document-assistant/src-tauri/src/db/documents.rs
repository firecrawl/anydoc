use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};

use super::{ANALYSIS_MIGRATION, DocumentRecord, PageAnalysisRecord};

const INITIAL_MIGRATION: &str = include_str!("../../migrations/0001_init.sql");

pub struct DocumentRepository {
    connection: Connection,
}

impl DocumentRepository {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create database directory {}", parent.display()))?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("open document database {}", path.display()))?;
        Self::from_connection(connection)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(connection: Connection) -> Result<Self> {
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.execute_batch(INITIAL_MIGRATION)?;
        connection.execute_batch(ANALYSIS_MIGRATION)?;
        Ok(Self { connection })
    }

    pub fn upsert_document(&self, document: &DocumentRecord) -> Result<()> {
        self.connection.execute(
            "INSERT INTO documents (
                id, source_path, file_name, format, sha256, status,
                markdown_path, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(sha256) DO UPDATE SET
                source_path = excluded.source_path,
                file_name = excluded.file_name,
                format = excluded.format,
                status = excluded.status,
                markdown_path = excluded.markdown_path,
                updated_at = excluded.updated_at",
            params![
                document.id,
                path_string(&document.source_path),
                document.file_name,
                document.format,
                document.sha256,
                document.status,
                document.markdown_path.as_deref().map(path_string),
                document.created_at,
                document.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_document(&self, id: &str) -> Result<Option<DocumentRecord>> {
        self.connection
            .query_row(
                "SELECT id, source_path, file_name, format, sha256, status,
                        markdown_path, created_at, updated_at
                 FROM documents WHERE id = ?1",
                [id],
                document_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_documents(&self) -> Result<Vec<DocumentRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT id, source_path, file_name, format, sha256, status,
                    markdown_path, created_at, updated_at
             FROM documents ORDER BY updated_at DESC, id ASC",
        )?;
        let records =
            statement.query_map([], document_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(records)
    }

    pub fn delete_cache_record(&self, id: &str) -> Result<()> {
        self.connection.execute("DELETE FROM page_search WHERE document_id = ?1", [id])?;
        self.connection.execute("DELETE FROM documents WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn save_page_analysis(
        &self,
        document_id: &str,
        page_number: u32,
        analysis_json: &str,
    ) -> Result<()> {
        self.connection.execute(
            "INSERT INTO page_analyses (
                document_id, page_number, analysis_json, raw_response, status, error, updated_at
             ) VALUES (?1, ?2, ?3, NULL, 'completed', NULL, ?4)
             ON CONFLICT(document_id, page_number) DO UPDATE SET
                analysis_json = excluded.analysis_json,
                raw_response = NULL,
                status = 'completed',
                error = NULL,
                updated_at = excluded.updated_at",
            params![document_id, page_number, analysis_json, now()],
        )?;
        Ok(())
    }

    pub fn save_page_analysis_failure(
        &self,
        document_id: &str,
        page_number: u32,
        raw_response: &str,
        error: &str,
    ) -> Result<()> {
        self.connection.execute(
            "INSERT INTO page_analyses (
                document_id, page_number, analysis_json, raw_response, status, error, updated_at
             ) VALUES (?1, ?2, NULL, ?3, 'failed', ?4, ?5)
             ON CONFLICT(document_id, page_number) DO UPDATE SET
                analysis_json = NULL,
                raw_response = excluded.raw_response,
                status = 'failed',
                error = excluded.error,
                updated_at = excluded.updated_at",
            params![document_id, page_number, raw_response, error, now()],
        )?;
        Ok(())
    }

    pub fn list_page_analyses(&self, document_id: &str) -> Result<Vec<PageAnalysisRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT document_id, page_number, analysis_json, raw_response, status, error, updated_at
             FROM page_analyses WHERE document_id = ?1 ORDER BY page_number",
        )?;
        Ok(statement
            .query_map([document_id], |row| {
                Ok(PageAnalysisRecord {
                    document_id: row.get(0)?,
                    page_number: row.get(1)?,
                    analysis_json: row.get(2)?,
                    raw_response: row.get(3)?,
                    status: row.get(4)?,
                    error: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_visual_content_analyzed(&self, document_id: &str, analyzed: bool) -> Result<()> {
        self.connection.execute(
            "INSERT INTO document_analysis (document_id, visual_content_analyzed, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(document_id) DO UPDATE SET
                visual_content_analyzed = excluded.visual_content_analyzed,
                updated_at = excluded.updated_at",
            params![document_id, analyzed, now()],
        )?;
        Ok(())
    }

    pub fn visual_content_analyzed(&self, document_id: &str) -> Result<Option<bool>> {
        self.connection
            .query_row(
                "SELECT visual_content_analyzed FROM document_analysis WHERE document_id = ?1",
                [document_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn save_document_summary(&self, document_id: &str, summary_json: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO document_analysis (document_id, summary_json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(document_id) DO UPDATE SET
                summary_json = excluded.summary_json,
                updated_at = excluded.updated_at",
            params![document_id, summary_json, now()],
        )?;
        Ok(())
    }

    pub fn document_summary(&self, document_id: &str) -> Result<Option<String>> {
        self.connection
            .query_row(
                "SELECT summary_json FROM document_analysis WHERE document_id = ?1",
                [document_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
        as i64
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn document_from_row(row: &Row<'_>) -> rusqlite::Result<DocumentRecord> {
    let markdown_path: Option<String> = row.get(6)?;
    Ok(DocumentRecord {
        id: row.get(0)?,
        source_path: PathBuf::from(row.get::<_, String>(1)?),
        file_name: row.get(2)?,
        format: row.get(3)?,
        sha256: row.get(4)?,
        status: row.get(5)?,
        markdown_path: markdown_path.map(PathBuf::from),
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}
