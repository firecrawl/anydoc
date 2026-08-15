use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};

use super::DocumentRecord;

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
        let records = statement
            .query_map([], document_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(records)
    }

    pub fn delete_cache_record(&self, id: &str) -> Result<()> {
        self.connection.execute(
            "DELETE FROM page_search WHERE document_id = ?1",
            [id],
        )?;
        self.connection
            .execute("DELETE FROM documents WHERE id = ?1", [id])?;
        Ok(())
    }
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
