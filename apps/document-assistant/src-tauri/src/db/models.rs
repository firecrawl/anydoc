use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentRecord {
    pub id: String,
    pub source_path: PathBuf,
    pub file_name: String,
    pub format: String,
    pub sha256: String,
    pub status: String,
    pub markdown_path: Option<PathBuf>,
    pub created_at: i64,
    pub updated_at: i64,
}
