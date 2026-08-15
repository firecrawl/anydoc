# AnyDoc Assistant Local Storage and Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist document history and resumable task state locally, protect API credentials, and render Office/PDF documents into page images with deterministic fallbacks.

**Architecture:** The Tauri Rust backend owns SQLite, Windows Credential Manager access, filesystem caches, Office/LibreOffice process execution, and page rendering. The React frontend communicates only through typed Tauri commands and progress events. Rendering failures degrade to text-only processing rather than destroying AnyDoc results.

**Tech Stack:** Tauri 2, Rust, Rusqlite with FTS5, Serde, Tokio, keyring, sha2, tempfile, PDFium renderer, PowerShell Office COM script, LibreOffice Headless.

## Global Constraints

- Runtime data lives under `%APPDATA%\AnyDoc Assistant`, never in the source tree.
- Original documents are not copied by default and delete-history never deletes the source file.
- API keys must not be stored in SQLite, logs, exports, or frontend state persistence.
- Rendering is Office to PDF first, LibreOffice fallback, then PDF to PNG.
- A missing renderer must leave local Markdown usable in text-only mode.
- Every task transition and completed page is persisted for restart recovery.

---

### Task 1: Add SQLite schema and document repository

**Files:**
- Create: `apps/document-assistant/src-tauri/migrations/0001_init.sql`
- Create: `apps/document-assistant/src-tauri/src/db/mod.rs`
- Create: `apps/document-assistant/src-tauri/src/db/models.rs`
- Create: `apps/document-assistant/src-tauri/src/db/documents.rs`
- Create: `apps/document-assistant/src-tauri/src/db/documents_test.rs`
- Modify: `apps/document-assistant/src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: application data directory from Plan 1 `AppInfo` setup.
- Produces: `DocumentRepository::upsert_document`, `get_document`, `list_documents`, `delete_cache_record`, and `DocumentRecord`.

```rust
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
```

- [ ] **Step 1: Write failing repository tests against a temporary database**

```rust
#[test]
fn upsert_by_hash_deduplicates_documents() {
    let repo = test_repository();
    repo.upsert_document(sample_document("same-hash")).unwrap();
    repo.upsert_document(sample_document("same-hash")).unwrap();
    assert_eq!(repo.list_documents().unwrap().len(), 1);
}

#[test]
fn deleting_history_does_not_delete_source_file() {
    let (repo, source) = repository_with_source_file();
    repo.delete_cache_record("doc-1").unwrap();
    assert!(source.exists());
}
```

- [ ] **Step 2: Verify tests fail**

Run: `cargo test --manifest-path apps/document-assistant/src-tauri/Cargo.toml db::documents`

Expected: FAIL because migrations and repository do not exist.

- [ ] **Step 3: Implement migrations and repository**

Create tables `documents`, `pages`, `tasks`, `model_profiles`, `conversations`, `messages`, and FTS5 virtual table `page_search`. Use `sha256` as a unique key. Enable foreign keys and WAL. Store paths as UTF-8 strings but convert at repository boundaries to `PathBuf`.

- [ ] **Step 4: Run repository and migration tests**

Run: `cargo test --manifest-path apps/document-assistant/src-tauri/Cargo.toml db::`

Expected: PASS, including Chinese and space-containing Windows paths.

- [ ] **Step 5: Commit local persistence**

```powershell
git add apps/document-assistant/src-tauri
git commit -m "feat: persist local document history"
```

### Task 2: Add cache layout, hashing, and history commands

**Files:**
- Create: `apps/document-assistant/src-tauri/src/storage.rs`
- Create: `apps/document-assistant/src-tauri/src/storage_test.rs`
- Modify: `apps/document-assistant/src-tauri/src/commands.rs`
- Create: `apps/document-assistant/src/features/history/historyApi.ts`
- Create: `apps/document-assistant/src/features/history/DocumentHistory.tsx`
- Create: `apps/document-assistant/src/features/history/DocumentHistory.test.tsx`

**Interfaces:**
- Consumes: `DocumentRepository` from Task 1.
- Produces: `compute_document_id(path) -> Result<DocumentIdentity>`, `cache_paths(document_id)`, Tauri commands `register_document`, `list_documents`, `delete_document_cache`, `clear_all_document_caches`.

- [ ] **Step 1: Write failing hashing and path safety tests**

```rust
#[test]
fn same_bytes_produce_same_document_id() {
    let first = write_temp_file("a.docx", b"same");
    let second = write_temp_file("b.docx", b"same");
    assert_eq!(compute_document_id(&first).unwrap().id, compute_document_id(&second).unwrap().id);
}

#[test]
fn cache_paths_remain_under_app_data() {
    let paths = cache_paths(&app_data(), "abc123").unwrap();
    assert!(paths.root.starts_with(app_data()));
}
```

- [ ] **Step 2: Run and verify failures**

Run: `cargo test --manifest-path apps/document-assistant/src-tauri/Cargo.toml storage`

Expected: FAIL because storage functions are absent.

- [ ] **Step 3: Implement streaming SHA-256 and cache directories**

Read files in bounded chunks, never load large documents only to hash them. Reject document IDs containing separators. Create `documents/{id}/pages` and `assets`. Write derived files atomically using a temporary sibling followed by rename.

- [ ] **Step 4: Implement history UI and source-safe deletion**

The history list shows name, format, last status, updated time, and cache size. Deletion confirmation text must say `只删除分析缓存，不删除原文件`. Add `清理全部缓存` with a second confirmation that lists total bytes and document count. After either deletion path, assert no source path was ever passed to a recursive delete function.

- [ ] **Step 5: Run Rust and React tests**

Run:

```powershell
cargo test --manifest-path apps/document-assistant/src-tauri/Cargo.toml storage
Set-Location apps\document-assistant
npm test -- --run src/features/history/DocumentHistory.test.tsx
```

Expected: PASS.

- [ ] **Step 6: Commit storage commands and history**

```powershell
git add apps/document-assistant
git commit -m "feat: manage document cache and history"
```

### Task 3: Store credentials and non-secret model profiles separately

**Files:**
- Create: `apps/document-assistant/src-tauri/src/credentials.rs`
- Create: `apps/document-assistant/src-tauri/src/credentials_test.rs`
- Create: `apps/document-assistant/src-tauri/src/db/model_profiles.rs`
- Modify: `apps/document-assistant/src-tauri/src/commands.rs`
- Create: `apps/document-assistant/src/features/settings/modelProfile.ts`
- Create: `apps/document-assistant/src/features/settings/ModelSettings.tsx`
- Create: `apps/document-assistant/src/features/settings/ModelSettings.test.tsx`

**Interfaces:**
- Produces: `ModelProfile { id, role, base_url, model, supports_vision, timeout_secs, max_concurrency }`, commands `save_model_profile`, `set_api_key`, `has_api_key`, `delete_api_key`.
- Credential service key format: `anydoc-assistant/{profile_id}`.

- [ ] **Step 1: Write failing secret-separation tests**

```rust
#[test]
fn serialized_profile_never_contains_api_key() {
    let json = serde_json::to_string(&sample_profile()).unwrap();
    assert!(!json.to_lowercase().contains("api_key"));
    assert!(!json.contains("sk-test"));
}
```

```tsx
it('never renders a saved key in plaintext', () => {
  render(<ModelSettings profile={savedProfile} hasApiKey />);
  expect(screen.queryByDisplayValue('sk-test')).not.toBeInTheDocument();
});
```

- [ ] **Step 2: Verify failures**

Run Rust credentials tests and the settings component test; expect missing modules.

- [ ] **Step 3: Implement Windows credential access behind a trait**

Define:

```rust
pub trait SecretStore: Send + Sync {
    fn set(&self, profile_id: &str, secret: &str) -> anyhow::Result<()>;
    fn get(&self, profile_id: &str) -> anyhow::Result<Option<String>>;
    fn delete(&self, profile_id: &str) -> anyhow::Result<()>;
}
```

Production uses `keyring`; tests use an in-memory implementation. Tauri command responses return only `has_api_key: bool`.

- [ ] **Step 4: Implement dual-role settings UI**

Allow roles `vision` and `text`, plus a `复用文本模型配置` toggle. Validate HTTPS unless host is `localhost` or `127.0.0.1`. Do not persist the API Key in React localStorage.

- [ ] **Step 5: Run tests and commit**

Run all Rust and frontend tests, then:

```powershell
git add apps/document-assistant
git commit -m "feat: configure models with protected credentials"
```

### Task 4: Implement the Office-to-PDF renderer chain

**Files:**
- Create: `apps/document-assistant/src-tauri/src/rendering/mod.rs`
- Create: `apps/document-assistant/src-tauri/src/rendering/office.rs`
- Create: `apps/document-assistant/src-tauri/src/rendering/libreoffice.rs`
- Create: `apps/document-assistant/src-tauri/src/rendering/pdf.rs`
- Create: `apps/document-assistant/src-tauri/src/rendering/render_office.ps1`
- Create: `apps/document-assistant/src-tauri/src/rendering/rendering_test.rs`

**Interfaces:**
- Produces: `Renderer::render(source, output_dir) -> Result<RenderManifest>`.

```rust
pub struct RenderManifest {
    pub backend: RenderBackend,
    pub pages: Vec<RenderedPage>,
}
pub struct RenderedPage { pub page_number: u32, pub image_path: PathBuf, pub width: u32, pub height: u32 }
pub enum RenderBackend { MicrosoftOffice, LibreOffice, PdfDirect, TextOnly }
```

- [ ] **Step 1: Write failing fallback-order tests with fake backends**

```rust
#[tokio::test]
async fn falls_back_to_libreoffice_when_office_fails() {
    let renderer = Renderer::new(failing_office(), successful_libreoffice(), fake_pdfium());
    let result = renderer.render(Path::new("slides.pptx"), temp_dir()).await.unwrap();
    assert_eq!(result.backend, RenderBackend::LibreOffice);
}
```

- [ ] **Step 2: Verify tests fail**

Run: `cargo test --manifest-path apps/document-assistant/src-tauri/Cargo.toml rendering`

Expected: FAIL because rendering traits are absent.

- [ ] **Step 3: Implement command-safe converters**

Pass source and output paths as separate process arguments; never build a shell command by string concatenation. The fixed PowerShell script opens Word or PowerPoint via COM, exports PDF, closes the document in `finally`, and quits the application object. LibreOffice uses `soffice --headless --convert-to pdf --outdir <dir> <source>` with timeout and captured stderr.

- [ ] **Step 4: Implement PDF page rendering**

Bind the packaged PDFium library, render pages at a configured scale, and write `page-0001.png` atomically. Return page dimensions in `RenderManifest`. Validate every output path remains under the document cache directory.

- [ ] **Step 5: Run unit tests and a Windows integration sample**

Run the fake-backend tests on every machine. On Windows, render one DOCX and one PPTX fixture, verify page count and non-empty PNG dimensions. If Office is absent, expected backend is LibreOffice.

- [ ] **Step 6: Commit rendering**

```powershell
git add apps/document-assistant/src-tauri
git commit -m "feat: render Office documents into page images"
```

### Task 5: Persist resumable task state and progress events

**Files:**
- Create: `apps/document-assistant/src-tauri/src/tasks/model.rs`
- Create: `apps/document-assistant/src-tauri/src/tasks/repository.rs`
- Create: `apps/document-assistant/src-tauri/src/tasks/runner.rs`
- Create: `apps/document-assistant/src-tauri/src/tasks/runner_test.rs`
- Modify: `apps/document-assistant/src-tauri/src/events.rs`
- Modify: `apps/document-assistant/src-tauri/src/commands.rs`
- Create: `apps/document-assistant/src/features/tasks/useTaskProgress.ts`
- Create: `apps/document-assistant/src/features/tasks/TaskProgress.tsx`

**Interfaces:**
- Produces: `TaskStage`, `TaskRunner::resume_pending`, commands `start_document_task`, `pause_task`, `resume_task`, `cancel_task`, `retry_failed_pages`.

```rust
pub enum TaskStage { Queued, Parsing, Rendering, VisionAnalysis, TextSynthesis, Completed, Failed }
```

- [ ] **Step 1: Write failing transition and restart tests**

```rust
#[test]
fn rejects_skipping_from_queued_to_completed() {
    assert!(TaskStage::Queued.transition_to(TaskStage::Completed).is_err());
}

#[tokio::test]
async fn resume_skips_pages_already_persisted() {
    let runner = runner_with_completed_pages([1, 2]);
    runner.resume("task-1").await.unwrap();
    assert_eq!(runner.requested_pages(), vec![3, 4]);
}
```

- [ ] **Step 2: Verify failures**

Run task tests; expected missing state machine.

- [ ] **Step 3: Implement legal transitions and per-page checkpoints**

Persist a page result before emitting its completion event. Cancellation stops scheduling new work but preserves completed cache. Startup calls `resume_pending` only after the user confirms recovery in the UI.

- [ ] **Step 4: Implement progress UI**

Display stage, completed/total, current message, pause/resume/cancel, and failed page count. The UI derives state from backend events plus a `get_task` refresh, not optimistic-only state.

- [ ] **Step 5: Run tests and commit**

Run all Rust/React tests, then:

```powershell
git add apps/document-assistant
git commit -m "feat: add resumable document tasks"
```

## Plan 2 Completion Gate

Pass criteria: a document is registered by hash, appears in local history, is rendered page by page when a renderer exists, safely falls back to text-only mode otherwise, stores no API key in SQLite, and resumes an interrupted task without repeating completed pages.
