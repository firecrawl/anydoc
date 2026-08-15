use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::Serialize;
use tauri::Manager;

use crate::{
    db::{DocumentRecord, DocumentRepository},
    storage::{cache_paths, compute_document_id, directory_size, remove_document_cache},
};

pub(crate) struct AppState {
    repository: Mutex<DocumentRepository>,
    app_data_dir: PathBuf,
}

impl AppState {
    pub(crate) fn open(app_data_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&app_data_dir)
            .with_context(|| format!("create app data directory {}", app_data_dir.display()))?;
        let repository = DocumentRepository::open(&app_data_dir.join("anydoc-assistant.db"))?;
        Ok(Self {
            repository: Mutex::new(repository),
            app_data_dir,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSummary {
    pub id: String,
    pub file_name: String,
    pub format: String,
    pub status: String,
    pub updated_at: i64,
    pub cache_size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub app_data_dir: String,
}

pub(crate) fn app_info_for_test(app_data_dir: String) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        app_data_dir,
    }
}

#[tauri::command]
pub fn get_app_info(app: tauri::AppHandle) -> Result<AppInfo, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    Ok(app_info_for_test(app_data_dir.to_string_lossy().into_owned()))
}

pub(crate) fn register_document_at(state: &AppState, source: &Path) -> Result<DocumentSummary> {
    let identity = compute_document_id(source)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let file_name = source
        .file_name()
        .context("document path has no file name")?
        .to_string_lossy()
        .into_owned();
    let format = source
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| "unknown".into());
    let cache = cache_paths(&state.app_data_dir, &identity.id)?;
    let document = DocumentRecord {
        id: identity.id,
        source_path: source.to_path_buf(),
        file_name,
        format,
        sha256: identity.sha256,
        status: "registered".into(),
        markdown_path: Some(cache.markdown),
        created_at: now,
        updated_at: now,
    };
    state
        .repository
        .lock()
        .map_err(|_| anyhow::anyhow!("document repository lock poisoned"))?
        .upsert_document(&document)?;
    summary_from_record(state, &document)
}

fn document_summaries(state: &AppState) -> Result<Vec<DocumentSummary>> {
    let documents = state
        .repository
        .lock()
        .map_err(|_| anyhow::anyhow!("document repository lock poisoned"))?
        .list_documents()?;
    documents
        .iter()
        .map(|document| summary_from_record(state, document))
        .collect()
}

fn summary_from_record(state: &AppState, document: &DocumentRecord) -> Result<DocumentSummary> {
    let cache = cache_paths(&state.app_data_dir, &document.id)?;
    Ok(DocumentSummary {
        id: document.id.clone(),
        file_name: document.file_name.clone(),
        format: document.format.clone(),
        status: document.status.clone(),
        updated_at: document.updated_at,
        cache_size: directory_size(&cache.root)?,
    })
}

pub(crate) fn delete_document_cache_for_test(
    state: &AppState,
    document_id: &str,
) -> Result<()> {
    remove_document_cache(&state.app_data_dir, document_id)?;
    state
        .repository
        .lock()
        .map_err(|_| anyhow::anyhow!("document repository lock poisoned"))?
        .delete_cache_record(document_id)?;
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn register_document(
    state: tauri::State<'_, AppState>,
    source_path: String,
) -> Result<DocumentSummary, String> {
    register_document_at(&state, Path::new(&source_path)).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn list_documents(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<DocumentSummary>, String> {
    document_summaries(&state).map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_document_cache(
    state: tauri::State<'_, AppState>,
    document_id: String,
) -> Result<(), String> {
    delete_document_cache_for_test(&state, &document_id).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn clear_all_document_caches(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let document_ids = document_summaries(&state)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|document| document.id)
        .collect::<Vec<_>>();
    for document_id in document_ids {
        delete_document_cache_for_test(&state, &document_id)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        app_info_for_test, delete_document_cache_for_test, register_document_at, AppState,
    };
    use crate::storage::cache_paths;

    #[test]
    fn app_info_contains_a_version_and_camel_case_data_dir() {
        let info = app_info_for_test("C:\\Data".into());
        assert!(!info.version.is_empty());

        let json = serde_json::to_value(info).expect("app info serializes");
        assert_eq!(json["appDataDir"], "C:\\Data");
    }

    #[test]
    fn register_and_delete_cache_preserves_the_source() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let app_data = directory.path().join("AnyDoc Assistant");
        let source = directory.path().join("原始 演示.pptx");
        std::fs::write(&source, b"presentation bytes").expect("source writes");
        let state = AppState::open(app_data.clone()).expect("state opens");

        let registered = register_document_at(&state, &source).expect("document registers");
        let cache = cache_paths(&app_data, &registered.id).expect("cache paths");
        std::fs::write(cache.pages.join("page-0001.png"), b"derived").expect("cache writes");

        delete_document_cache_for_test(&state, &registered.id).expect("cache deletes");

        assert!(source.exists());
        assert!(!cache.root.exists());
    }
}
