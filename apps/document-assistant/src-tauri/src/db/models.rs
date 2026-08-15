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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageAnalysisRecord {
    pub document_id: String,
    pub page_number: u32,
    pub analysis_json: Option<String>,
    pub raw_response: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub updated_at: i64,
}
