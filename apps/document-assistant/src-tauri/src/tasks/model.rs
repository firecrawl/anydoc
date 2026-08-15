use std::{fmt, path::PathBuf, str::FromStr};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStage {
    Queued,
    Parsing,
    Rendering,
    VisionAnalysis,
    TextSynthesis,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStage {
    pub fn transition_to(self, next: Self) -> Result<()> {
        let legal = matches!(
            (self, next),
            (Self::Queued, Self::Parsing | Self::Paused | Self::Cancelled)
                | (Self::Parsing, Self::Rendering | Self::Paused | Self::Failed | Self::Cancelled)
                | (
                    Self::Rendering,
                    Self::VisionAnalysis
                        | Self::TextSynthesis
                        | Self::Completed
                        | Self::Paused
                        | Self::Failed
                        | Self::Cancelled
                )
                | (
                    Self::VisionAnalysis,
                    Self::TextSynthesis | Self::Paused | Self::Failed | Self::Cancelled
                )
                | (
                    Self::TextSynthesis,
                    Self::Completed | Self::Paused | Self::Failed | Self::Cancelled
                )
                | (
                    Self::Paused,
                    Self::Parsing
                        | Self::Rendering
                        | Self::VisionAnalysis
                        | Self::TextSynthesis
                        | Self::Cancelled
                )
                | (Self::Failed, Self::Queued)
        );
        if legal { Ok(()) } else { bail!("illegal task transition from {self} to {next}") }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled)
    }
}

impl fmt::Display for TaskStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Queued => "queued",
            Self::Parsing => "parsing",
            Self::Rendering => "rendering",
            Self::VisionAnalysis => "vision_analysis",
            Self::TextSynthesis => "text_synthesis",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        };
        formatter.write_str(value)
    }
}

impl FromStr for TaskStage {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "parsing" => Ok(Self::Parsing),
            "rendering" => Ok(Self::Rendering),
            "vision_analysis" => Ok(Self::VisionAnalysis),
            "text_synthesis" => Ok(Self::TextSynthesis),
            "paused" => Ok(Self::Paused),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => bail!("unknown task stage: {value}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    pub id: String,
    pub document_id: String,
    pub stage: TaskStage,
    pub completed: u32,
    pub total: u32,
    pub message: String,
    pub error: Option<String>,
    pub failed_pages: u32,
    pub created_at: i64,
    pub updated_at: i64,
}

impl TaskRecord {
    pub fn new(
        id: impl Into<String>,
        document_id: impl Into<String>,
        stage: TaskStage,
        total: u32,
    ) -> Self {
        Self {
            id: id.into(),
            document_id: document_id.into(),
            stage,
            completed: 0,
            total,
            message: String::new(),
            error: None,
            failed_pages: 0,
            created_at: unix_timestamp(),
            updated_at: unix_timestamp(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCheckpoint {
    pub page_number: u32,
    pub image_path: Option<PathBuf>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub markdown: Option<String>,
}

pub(crate) fn unix_timestamp() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
        as i64
}
