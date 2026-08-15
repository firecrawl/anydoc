use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::{
    credentials::{SecretStore, WindowsSecretStore},
    db::{DocumentRecord, DocumentRepository, ModelProfile, ModelProfileRepository},
    rendering::{
        DocumentRenderer, RenderBackend, Renderer, libreoffice::LibreOfficeConverter,
        office::MicrosoftOfficeConverter, pdf::PdfiumPageRenderer,
    },
    storage::{cache_paths, compute_document_id, directory_size, remove_document_cache},
    tasks::{PageCheckpoint, ProgressSink, TaskRecord, TaskRepository, TaskStage},
};

pub(crate) struct AppState {
    repository: Mutex<DocumentRepository>,
    model_profiles: Mutex<ModelProfileRepository>,
    task_repository: Mutex<TaskRepository>,
    secret_store: Box<dyn SecretStore>,
    renderer: Arc<dyn DocumentRenderer>,
    running_tasks: Arc<Mutex<HashSet<String>>>,
    app_data_dir: PathBuf,
    database_path: PathBuf,
}

impl AppState {
    pub(crate) fn open(app_data_dir: PathBuf) -> Result<Self> {
        Self::open_with_secret_store(app_data_dir, Box::new(WindowsSecretStore))
    }

    pub(crate) fn open_with_secret_store(
        app_data_dir: PathBuf,
        secret_store: Box<dyn SecretStore>,
    ) -> Result<Self> {
        Self::open_with_services(app_data_dir, secret_store, default_renderer())
    }

    pub(crate) fn open_with_services(
        app_data_dir: PathBuf,
        secret_store: Box<dyn SecretStore>,
        renderer: Arc<dyn DocumentRenderer>,
    ) -> Result<Self> {
        std::fs::create_dir_all(&app_data_dir)
            .with_context(|| format!("create app data directory {}", app_data_dir.display()))?;
        let database_path = app_data_dir.join("anydoc-assistant.db");
        let repository = DocumentRepository::open(&database_path)?;
        let model_profiles = ModelProfileRepository::open(&database_path)?;
        let task_repository = TaskRepository::open(&database_path)?;
        Ok(Self {
            repository: Mutex::new(repository),
            model_profiles: Mutex::new(model_profiles),
            task_repository: Mutex::new(task_repository),
            secret_store,
            renderer,
            running_tasks: Arc::new(Mutex::new(HashSet::new())),
            app_data_dir,
            database_path,
        })
    }
}

fn default_renderer() -> Arc<dyn DocumentRenderer> {
    let pdfium = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/pdfium.dll");
    Arc::new(Renderer::new(
        vec![
            Arc::new(MicrosoftOfficeConverter::default()),
            Arc::new(LibreOfficeConverter::default()),
        ],
        Arc::new(PdfiumPageRenderer::new(Some(pdfium))),
    ))
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
pub struct ModelProfileStatus {
    #[serde(flatten)]
    pub profile: ModelProfile,
    pub has_api_key: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub app_data_dir: String,
}

pub(crate) fn app_info_for_test(app_data_dir: String) -> AppInfo {
    AppInfo { version: env!("CARGO_PKG_VERSION").to_owned(), app_data_dir }
}

#[tauri::command]
pub fn get_app_info(app: tauri::AppHandle) -> Result<AppInfo, String> {
    let app_data_dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    Ok(app_info_for_test(app_data_dir.to_string_lossy().into_owned()))
}

pub(crate) fn register_document_at(state: &AppState, source: &Path) -> Result<DocumentSummary> {
    let identity = compute_document_id(source)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
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
    documents.iter().map(|document| summary_from_record(state, document)).collect()
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

pub(crate) fn delete_document_cache_for_test(state: &AppState, document_id: &str) -> Result<()> {
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
pub fn list_documents(state: tauri::State<'_, AppState>) -> Result<Vec<DocumentSummary>, String> {
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
        delete_document_cache_for_test(&state, &document_id).map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub(crate) fn save_model_profile_for_test(state: &AppState, profile: &ModelProfile) -> Result<()> {
    state
        .model_profiles
        .lock()
        .map_err(|_| anyhow::anyhow!("model profile repository lock poisoned"))?
        .upsert(profile)
}

pub(crate) fn set_api_key_for_test(
    state: &AppState,
    profile_id: &str,
    api_key: &str,
) -> Result<()> {
    if api_key.trim().is_empty() {
        anyhow::bail!("API key cannot be empty");
    }
    state.secret_store.set(profile_id, api_key.trim())
}

pub(crate) fn list_model_profiles_for_test(state: &AppState) -> Result<Vec<ModelProfileStatus>> {
    state
        .model_profiles
        .lock()
        .map_err(|_| anyhow::anyhow!("model profile repository lock poisoned"))?
        .list()?
        .into_iter()
        .map(|profile| {
            let has_api_key = state.secret_store.get(&profile.id)?.is_some();
            Ok(ModelProfileStatus { profile, has_api_key })
        })
        .collect()
}

#[tauri::command(rename_all = "camelCase")]
pub fn save_model_profile(
    state: tauri::State<'_, AppState>,
    profile: ModelProfile,
) -> Result<(), String> {
    save_model_profile_for_test(&state, &profile).map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_api_key(
    state: tauri::State<'_, AppState>,
    profile_id: String,
    api_key: String,
) -> Result<(), String> {
    set_api_key_for_test(&state, &profile_id, &api_key).map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn has_api_key(state: tauri::State<'_, AppState>, profile_id: String) -> Result<bool, String> {
    state
        .secret_store
        .get(&profile_id)
        .map(|secret| secret.is_some())
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn delete_api_key(state: tauri::State<'_, AppState>, profile_id: String) -> Result<(), String> {
    state.secret_store.delete(&profile_id).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn list_model_profiles(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ModelProfileStatus>, String> {
    list_model_profiles_for_test(&state).map_err(|error| error.to_string())
}

pub(crate) fn create_document_task_for_test(
    state: &AppState,
    document_id: &str,
) -> Result<TaskRecord> {
    let document_exists = state
        .repository
        .lock()
        .map_err(|_| anyhow::anyhow!("document repository lock poisoned"))?
        .get_document(document_id)?
        .is_some();
    if !document_exists {
        anyhow::bail!("document not found: {document_id}");
    }

    let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let task =
        TaskRecord::new(format!("task-{document_id}-{unique}"), document_id, TaskStage::Queued, 0);
    state
        .task_repository
        .lock()
        .map_err(|_| anyhow::anyhow!("task repository lock poisoned"))?
        .create_task(&task)?;
    Ok(task)
}

pub(crate) fn set_task_stage_for_test(
    state: &AppState,
    task_id: &str,
    stage: TaskStage,
    message: &str,
) -> Result<TaskRecord> {
    state
        .task_repository
        .lock()
        .map_err(|_| anyhow::anyhow!("task repository lock poisoned"))?
        .set_stage(task_id, stage, message)
}

#[cfg(test)]
pub(crate) fn run_document_task_for_test(
    state: &AppState,
    task_id: &str,
    progress: &dyn ProgressSink,
) -> Result<TaskRecord> {
    let task = get_task_at(state, task_id)?;
    let document = state
        .repository
        .lock()
        .map_err(|_| anyhow::anyhow!("document repository lock poisoned"))?
        .get_document(&task.document_id)?
        .with_context(|| format!("document not found: {}", task.document_id))?;
    run_document_task(
        &state.database_path,
        &state.app_data_dir,
        state.renderer.as_ref(),
        &document,
        task_id,
        progress,
    )
}

fn run_document_task(
    database_path: &Path,
    app_data_dir: &Path,
    renderer: &dyn DocumentRenderer,
    document: &DocumentRecord,
    task_id: &str,
    progress: &dyn ProgressSink,
) -> Result<TaskRecord> {
    let mut repository = TaskRepository::open(database_path)?;
    let mut task =
        repository.get_task(task_id)?.with_context(|| format!("task not found: {task_id}"))?;

    if task.stage == TaskStage::Queued {
        task = repository.set_stage(task_id, TaskStage::Parsing, "正在准备文档")?;
        progress.emit(&task)?;
    }
    if task.stage == TaskStage::Paused {
        let resume_stage = if task.total == 0 { TaskStage::Parsing } else { TaskStage::Rendering };
        task = repository.set_stage(task_id, resume_stage, "继续处理")?;
        progress.emit(&task)?;
    }
    if task.stage == TaskStage::Parsing {
        task = repository.set_stage(task_id, TaskStage::Rendering, "正在生成逐页图像")?;
        progress.emit(&task)?;
    }
    if task.stage != TaskStage::Rendering {
        return Ok(task);
    }

    let cache = cache_paths(app_data_dir, &document.id)?;
    let manifest = renderer.render(&document.source_path, &cache.pages)?;
    repository.set_total(task_id, manifest.pages.len() as u32)?;
    if manifest.backend == RenderBackend::TextOnly {
        task = repository.set_stage(task_id, TaskStage::Completed, "已完成文本解析")?;
        progress.emit(&task)?;
        return Ok(task);
    }

    let completed_pages = repository.completed_pages(task_id)?.into_iter().collect::<HashSet<_>>();
    for page in manifest.pages {
        task =
            repository.get_task(task_id)?.with_context(|| format!("task not found: {task_id}"))?;
        if matches!(task.stage, TaskStage::Paused | TaskStage::Cancelled) {
            return Ok(task);
        }
        if completed_pages.contains(&page.page_number) {
            continue;
        }
        task = repository.checkpoint_page(
            task_id,
            &PageCheckpoint {
                page_number: page.page_number,
                image_path: Some(page.image_path),
                width: Some(page.width),
                height: Some(page.height),
                markdown: None,
            },
        )?;
        progress.emit(&task)?;
    }

    task = repository.set_stage(task_id, TaskStage::Completed, "逐页渲染完成")?;
    progress.emit(&task)?;
    Ok(task)
}

fn get_task_at(state: &AppState, task_id: &str) -> Result<TaskRecord> {
    state
        .task_repository
        .lock()
        .map_err(|_| anyhow::anyhow!("task repository lock poisoned"))?
        .get_task(task_id)?
        .with_context(|| format!("task not found: {task_id}"))
}

struct TauriProgressSink {
    app: tauri::AppHandle,
}

impl ProgressSink for TauriProgressSink {
    fn emit(&self, task: &TaskRecord) -> Result<()> {
        self.app.emit(crate::events::TASK_PROGRESS_EVENT, task).map_err(Into::into)
    }
}

fn spawn_document_task(app: tauri::AppHandle, state: &AppState, task: &TaskRecord) -> Result<()> {
    let document = state
        .repository
        .lock()
        .map_err(|_| anyhow::anyhow!("document repository lock poisoned"))?
        .get_document(&task.document_id)?
        .with_context(|| format!("document not found: {}", task.document_id))?;
    {
        let mut running = state
            .running_tasks
            .lock()
            .map_err(|_| anyhow::anyhow!("running task registry lock poisoned"))?;
        if !running.insert(task.id.clone()) {
            anyhow::bail!("task is already running: {}", task.id);
        }
    }

    let task_id = task.id.clone();
    let database_path = state.database_path.clone();
    let app_data_dir = state.app_data_dir.clone();
    let renderer = state.renderer.clone();
    let running_tasks = state.running_tasks.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let sink = TauriProgressSink { app };
        if let Err(error) = run_document_task(
            &database_path,
            &app_data_dir,
            renderer.as_ref(),
            &document,
            &task_id,
            &sink,
        ) && let Ok(mut repository) = TaskRepository::open(&database_path)
            && let Ok(Some(current)) = repository.get_task(&task_id)
            && !matches!(
                current.stage,
                TaskStage::Paused | TaskStage::Cancelled | TaskStage::Completed | TaskStage::Failed
            )
            && let Ok(failed) = repository.fail_task(&task_id, &error.to_string())
        {
            let _ = sink.emit(&failed);
        }
        if let Ok(mut running) = running_tasks.lock() {
            running.remove(&task_id);
        }
    });
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn start_document_task(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    document_id: String,
) -> Result<TaskRecord, String> {
    let task =
        create_document_task_for_test(&state, &document_id).map_err(|error| error.to_string())?;
    app.emit(crate::events::TASK_PROGRESS_EVENT, &task).map_err(|error| error.to_string())?;
    spawn_document_task(app, &state, &task).map_err(|error| error.to_string())?;
    Ok(task)
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_task(state: tauri::State<'_, AppState>, task_id: String) -> Result<TaskRecord, String> {
    get_task_at(&state, &task_id).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn list_recoverable_tasks(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<TaskRecord>, String> {
    state
        .task_repository
        .lock()
        .map_err(|_| "task repository lock poisoned".to_owned())?
        .list_recoverable()
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub fn pause_task(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<TaskRecord, String> {
    let task = set_task_stage_for_test(&state, &task_id, TaskStage::Paused, "用户已暂停")
        .map_err(|error| error.to_string())?;
    app.emit(crate::events::TASK_PROGRESS_EVENT, &task).map_err(|error| error.to_string())?;
    Ok(task)
}

#[tauri::command(rename_all = "camelCase")]
pub fn cancel_task(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<TaskRecord, String> {
    let task = set_task_stage_for_test(&state, &task_id, TaskStage::Cancelled, "用户已取消")
        .map_err(|error| error.to_string())?;
    app.emit(crate::events::TASK_PROGRESS_EVENT, &task).map_err(|error| error.to_string())?;
    Ok(task)
}

#[tauri::command(rename_all = "camelCase")]
pub fn resume_task(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<TaskRecord, String> {
    let task = get_task_at(&state, &task_id).map_err(|error| error.to_string())?;
    if !matches!(
        task.stage,
        TaskStage::Queued | TaskStage::Parsing | TaskStage::Rendering | TaskStage::Paused
    ) {
        return Err(format!("task cannot be resumed from {}", task.stage));
    }
    spawn_document_task(app, &state, &task).map_err(|error| error.to_string())?;
    Ok(task)
}

#[tauri::command(rename_all = "camelCase")]
pub fn retry_failed_pages(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    task_id: String,
) -> Result<TaskRecord, String> {
    let task = state
        .task_repository
        .lock()
        .map_err(|_| "task repository lock poisoned".to_owned())?
        .reset_failed_pages(&task_id)
        .map_err(|error| error.to_string())?;
    app.emit(crate::events::TASK_PROGRESS_EVENT, &task).map_err(|error| error.to_string())?;
    spawn_document_task(app, &state, &task).map_err(|error| error.to_string())?;
    Ok(task)
}

#[cfg(test)]
mod tests {
    use super::{
        AppState, app_info_for_test, create_document_task_for_test, delete_document_cache_for_test,
        list_model_profiles_for_test, register_document_at, run_document_task_for_test,
        save_model_profile_for_test, set_api_key_for_test, set_task_stage_for_test,
    };
    use crate::storage::cache_paths;
    use crate::{
        credentials::MemorySecretStore,
        db::{ModelProfile, ModelRole},
        rendering::{DocumentRenderer, RenderBackend, RenderManifest, RenderedPage},
        tasks::{ProgressSink, TaskRecord, TaskStage},
    };
    use std::{
        path::Path,
        sync::{Arc, Mutex},
    };

    struct FakeDocumentRenderer;

    impl DocumentRenderer for FakeDocumentRenderer {
        fn render(&self, _source: &Path, output_dir: &Path) -> anyhow::Result<RenderManifest> {
            Ok(RenderManifest {
                backend: RenderBackend::MicrosoftOffice,
                pages: (1..=2)
                    .map(|page_number| RenderedPage {
                        page_number,
                        image_path: output_dir.join(format!("page-{page_number:04}.png")),
                        width: 1600,
                        height: 900,
                    })
                    .collect(),
            })
        }
    }

    #[derive(Default)]
    struct RecordingTaskSink(Mutex<Vec<TaskRecord>>);

    impl ProgressSink for RecordingTaskSink {
        fn emit(&self, task: &TaskRecord) -> anyhow::Result<()> {
            self.0.lock().expect("event lock").push(task.clone());
            Ok(())
        }
    }

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

    #[test]
    fn model_profile_status_exposes_only_key_presence() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let state = AppState::open_with_secret_store(
            directory.path().join("AnyDoc Assistant"),
            Box::new(MemorySecretStore::default()),
        )
        .expect("state opens");
        let profile = ModelProfile {
            id: "vision-primary".into(),
            role: ModelRole::Vision,
            base_url: "https://api.example.com/v1".into(),
            model: "vision-model".into(),
            supports_vision: true,
            timeout_secs: 120,
            max_concurrency: 2,
        };

        save_model_profile_for_test(&state, &profile).expect("profile saves");
        set_api_key_for_test(&state, &profile.id, "sk-test").expect("key saves");
        let statuses = list_model_profiles_for_test(&state).expect("profiles list");
        let json = serde_json::to_string(&statuses).expect("statuses serialize");

        assert_eq!(statuses.len(), 1);
        assert!(statuses[0].has_api_key);
        assert!(!json.contains("sk-test"));
    }

    #[test]
    fn task_controls_persist_pause_and_cancel_states() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("sample.docx");
        std::fs::write(&source, b"document bytes").expect("source writes");
        let state = AppState::open(directory.path().join("AnyDoc Assistant")).expect("state opens");
        let document = register_document_at(&state, &source).expect("document registers");
        let queued = create_document_task_for_test(&state, &document.id).expect("task creates");

        let paused = set_task_stage_for_test(&state, &queued.id, TaskStage::Paused, "用户已暂停")
            .expect("task pauses");
        let cancelled =
            set_task_stage_for_test(&state, &queued.id, TaskStage::Cancelled, "用户已取消")
                .expect("task cancels");

        assert_eq!(paused.stage, TaskStage::Paused);
        assert_eq!(cancelled.stage, TaskStage::Cancelled);
    }

    #[test]
    fn processing_task_checkpoints_pages_before_completion_event() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("sample.pptx");
        std::fs::write(&source, b"presentation bytes").expect("source writes");
        let state = AppState::open_with_services(
            directory.path().join("AnyDoc Assistant"),
            Box::new(MemorySecretStore::default()),
            Arc::new(FakeDocumentRenderer),
        )
        .expect("state opens");
        let document = register_document_at(&state, &source).expect("document registers");
        let task = create_document_task_for_test(&state, &document.id).expect("task creates");
        let sink = RecordingTaskSink::default();

        let completed =
            run_document_task_for_test(&state, &task.id, &sink).expect("task processes");
        let events = sink.0.lock().expect("event lock");

        assert_eq!(completed.stage, TaskStage::Completed);
        assert_eq!(completed.completed, 2);
        assert_eq!(events[events.len() - 2].completed, 2);
        assert_eq!(events.last().expect("completion event").stage, TaskStage::Completed);
    }
}
