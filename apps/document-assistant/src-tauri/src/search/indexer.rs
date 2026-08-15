use std::{path::Path, sync::Mutex};

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

const INITIAL_MIGRATION: &str = include_str!("../../migrations/0001_init.sql");
const SEARCH_TABLES: &str = "
CREATE TABLE IF NOT EXISTS search_pages (
    document_id TEXT NOT NULL,
    page_number INTEGER NOT NULL,
    heading TEXT,
    text TEXT NOT NULL,
    visual_summary TEXT,
    PRIMARY KEY(document_id, page_number)
);";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageIndexEntry {
    pub page_number: u32,
    pub heading: Option<String>,
    pub text: String,
    pub visual_summary: Option<String>,
}

impl PageIndexEntry {
    pub fn text(page_number: u32, text: impl Into<String>) -> Self {
        Self { page_number, heading: None, text: text.into(), visual_summary: None }
    }
}

pub struct SearchIndex {
    pub(super) connection: Mutex<Connection>,
}

impl SearchIndex {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)
            .with_context(|| format!("open search database {}", path.display()))?;
        connection.execute_batch(INITIAL_MIGRATION)?;
        connection.execute_batch(SEARCH_TABLES)?;
        Ok(Self { connection: Mutex::new(connection) })
    }

    pub fn index_document_pages(&self, document_id: &str, pages: &[PageIndexEntry]) -> Result<()> {
        let mut connection =
            self.connection.lock().map_err(|_| anyhow::anyhow!("search index lock poisoned"))?;
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM page_search WHERE document_id = ?1", [document_id])?;
        transaction.execute("DELETE FROM search_pages WHERE document_id = ?1", [document_id])?;
        for page in pages {
            transaction.execute(
                "INSERT INTO search_pages (document_id, page_number, heading, text, visual_summary)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    document_id,
                    page.page_number,
                    page.heading,
                    page.text,
                    page.visual_summary
                ],
            )?;
            let searchable = search_tokens(
                &[
                    page.heading.as_deref().unwrap_or_default(),
                    &page.text,
                    page.visual_summary.as_deref().unwrap_or_default(),
                ]
                .join(" "),
            );
            transaction.execute(
                "INSERT INTO page_search (document_id, page_number, content) VALUES (?1, ?2, ?3)",
                params![document_id, page.page_number, searchable],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }
}

pub(super) fn query_tokens(value: &str) -> Vec<String> {
    let normalized = value
        .chars()
        .map(
            |character| {
                if character.is_alphanumeric() || is_cjk(character) { character } else { ' ' }
            },
        )
        .collect::<String>();
    let mut tokens = normalized
        .split_whitespace()
        .flat_map(segment_tokens)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    tokens.sort();
    tokens.dedup();
    tokens
}

fn search_tokens(value: &str) -> String {
    let mut tokens = query_tokens(value);
    tokens.push(value.replace(['"', '\'', '(', ')', '*', ':'], " "));
    tokens.join(" ")
}

fn segment_tokens(segment: &str) -> Vec<String> {
    let chars = segment.chars().collect::<Vec<_>>();
    if chars.iter().any(|character| is_cjk(*character)) {
        if chars.len() < 2 {
            return vec![segment.to_owned()];
        }
        return chars.windows(2).map(|pair| pair.iter().collect()).collect();
    }
    vec![segment.to_lowercase()]
}

fn is_cjk(character: char) -> bool {
    matches!(character as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}
