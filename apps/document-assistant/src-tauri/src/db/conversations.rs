use std::path::Path;

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::{analysis::EvidenceRef, tasks::model::unix_timestamp};

const INITIAL_MIGRATION: &str = include_str!("../../migrations/0001_init.sql");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub citations: Vec<EvidenceRef>,
    pub created_at: i64,
}

pub struct ConversationRepository {
    connection: Connection,
}

impl ConversationRepository {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.execute_batch(INITIAL_MIGRATION)?;
        Ok(Self { connection })
    }

    pub fn ensure_conversation(&self, id: &str, document_id: &str, title: &str) -> Result<()> {
        let now = unix_timestamp();
        self.connection.execute(
            "INSERT INTO conversations (id, document_id, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at",
            params![id, document_id, title, now],
        )?;
        Ok(())
    }

    pub fn add_message(
        &self,
        conversation_id: &str,
        role: &str,
        content: &str,
        citations: &[EvidenceRef],
    ) -> Result<ConversationMessage> {
        let now = unix_timestamp();
        let id =
            format!("msg-{conversation_id}-{now}-{}", self.message_count(conversation_id)? + 1);
        self.connection.execute(
            "INSERT INTO messages (id, conversation_id, role, content, citations_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, conversation_id, role, content, serde_json::to_string(citations)?, now],
        )?;
        Ok(ConversationMessage {
            id,
            role: role.into(),
            content: content.into(),
            citations: citations.to_vec(),
            created_at: now,
        })
    }

    pub fn list_messages(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Result<Vec<ConversationMessage>> {
        let mut statement = self.connection.prepare(
            "SELECT id, role, content, citations_json, created_at FROM (
                SELECT id, role, content, citations_json, created_at
                FROM messages WHERE conversation_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2
             ) ORDER BY created_at, id",
        )?;
        Ok(statement
            .query_map(params![conversation_id, limit as u32], |row| {
                let citations_json: String = row.get(3)?;
                Ok(ConversationMessage {
                    id: row.get(0)?,
                    role: row.get(1)?,
                    content: row.get(2)?,
                    citations: serde_json::from_str(&citations_json).unwrap_or_default(),
                    created_at: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn message_count(&self, conversation_id: &str) -> Result<u64> {
        Ok(self.connection.query_row(
            "SELECT COUNT(*) FROM messages WHERE conversation_id = ?1",
            [conversation_id],
            |row| row.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::ConversationRepository;

    #[test]
    fn messages_round_trip_in_conversation_order() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = directory.path().join("chat.db");
        let documents = crate::db::DocumentRepository::open(&database).expect("documents open");
        documents
            .upsert_document(&crate::db::DocumentRecord {
                id: "doc-1".into(),
                source_path: "a.pdf".into(),
                file_name: "a.pdf".into(),
                format: "pdf".into(),
                sha256: "hash".into(),
                status: "parsed".into(),
                markdown_path: None,
                created_at: 1,
                updated_at: 1,
            })
            .expect("document inserts");
        let conversations = ConversationRepository::open(&database).expect("conversations open");
        conversations.ensure_conversation("chat-1", "doc-1", "收入").expect("conversation creates");
        conversations.add_message("chat-1", "user", "收入？", &[]).expect("user saves");
        conversations.add_message("chat-1", "assistant", "增长", &[]).expect("assistant saves");
        let messages = conversations.list_messages("chat-1", 10).expect("messages load");
        assert_eq!(
            messages.iter().map(|message| message.role.as_str()).collect::<Vec<_>>(),
            vec!["user", "assistant"]
        );
    }
}
