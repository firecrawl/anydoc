use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::ANALYSIS_MIGRATION;

const INITIAL_MIGRATION: &str = include_str!("../../migrations/0001_init.sql");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelRole {
    Vision,
    Text,
}

impl ModelRole {
    fn as_str(self) -> &'static str {
        match self {
            Self::Vision => "vision",
            Self::Text => "text",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProfile {
    pub id: String,
    pub role: ModelRole,
    pub base_url: String,
    pub model: String,
    pub supports_vision: bool,
    pub timeout_secs: u32,
    pub max_concurrency: u32,
}

pub struct ModelProfileRepository {
    connection: Connection,
}

impl ModelProfileRepository {
    pub fn open(path: &Path) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(INITIAL_MIGRATION)?;
        connection.execute_batch(ANALYSIS_MIGRATION)?;
        Ok(Self { connection })
    }

    pub fn upsert(&self, profile: &ModelProfile) -> Result<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        self.connection.execute(
            "INSERT INTO model_profiles (
                id, role, base_url, model, supports_vision, timeout_secs,
                max_concurrency, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
             ON CONFLICT(id) DO UPDATE SET
                role = excluded.role,
                base_url = excluded.base_url,
                model = excluded.model,
                supports_vision = excluded.supports_vision,
                timeout_secs = excluded.timeout_secs,
                max_concurrency = excluded.max_concurrency,
                updated_at = excluded.updated_at",
            params![
                profile.id,
                profile.role.as_str(),
                profile.base_url,
                profile.model,
                profile.supports_vision,
                profile.timeout_secs,
                profile.max_concurrency,
                now,
            ],
        )?;
        self.connection
            .execute("DELETE FROM model_capability_tests WHERE profile_id = ?1", [&profile.id])?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<ModelProfile>> {
        let mut statement = self.connection.prepare(
            "SELECT id, role, base_url, model, supports_vision, timeout_secs, max_concurrency
             FROM model_profiles ORDER BY role, id",
        )?;
        let profiles = statement
            .query_map([], |row| {
                let role: String = row.get(1)?;
                Ok(ModelProfile {
                    id: row.get(0)?,
                    role: if role == "vision" { ModelRole::Vision } else { ModelRole::Text },
                    base_url: row.get(2)?,
                    model: row.get(3)?,
                    supports_vision: row.get(4)?,
                    timeout_secs: row.get(5)?,
                    max_concurrency: row.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(profiles)
    }

    pub fn mark_capability_tested(&self, profile_id: &str, supports_vision: bool) -> Result<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        self.connection.execute(
            "INSERT INTO model_capability_tests (profile_id, supports_vision, tested_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(profile_id) DO UPDATE SET
                supports_vision = excluded.supports_vision,
                tested_at = excluded.tested_at",
            params![profile_id, supports_vision, now],
        )?;
        Ok(())
    }

    pub fn capability_tested(&self, profile_id: &str, supports_vision: bool) -> Result<bool> {
        self.connection
            .query_row(
                "SELECT 1 FROM model_capability_tests
             WHERE profile_id = ?1 AND supports_vision = ?2",
                params![profile_id, supports_vision],
                |_| Ok(true),
            )
            .optional()
            .map(|value| value.unwrap_or(false))
            .map_err(Into::into)
    }
}
