# AnyDoc Assistant Chat, Export, and Windows Delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the product with source-page verification, continuous cited chat, Markdown/JSON export, robust recovery, and a Windows installer that runs without development tools.

**Architecture:** SQLite FTS5 retrieves relevant pages for long-document chat while short documents use full context. Result tabs share one selected-page state so citations navigate to cached page images. Export is generated in Rust from persisted typed data. Packaging includes required notices and optional/default LibreOffice and PDFium runtime components.

**Tech Stack:** React/TypeScript, Rust, Rusqlite FTS5, Tauri 2, Serde, Markdown generation, Vitest, Playwright, cargo tests, Windows installer tooling.

## Global Constraints

- Chat responses must cite pages and say `文档中未找到相关信息` when evidence is absent.
- First version uses SQLite FTS5, not a vector database or embedding model.
- Export includes model identity, analysis time, page references, failed pages, and limitations.
- Delete history never deletes source files.
- Installer must run on a Windows machine without Node, Rust, or Python.
- AnyDoc, LibreOffice, PDFium, and other redistributed dependency notices must be included.

---

### Task 1: Add page indexing and context retrieval

**Files:**
- Create: `apps/document-assistant/src-tauri/src/search/indexer.rs`
- Create: `apps/document-assistant/src-tauri/src/search/retriever.rs`
- Create: `apps/document-assistant/src-tauri/src/search/retriever_test.rs`
- Modify: `apps/document-assistant/src-tauri/src/db/documents.rs`

**Interfaces:**
- Consumes: page text, page analysis summaries, section headings, SQLite FTS5 schema.
- Produces: `index_document_pages(document_id, pages)` and `retrieve_context(document_id, query, limit) -> Vec<ContextPage>`.

```rust
pub struct ContextPage {
    pub document_id: String,
    pub page_number: u32,
    pub heading: Option<String>,
    pub text: String,
    pub visual_summary: Option<String>,
    pub score: f64,
}
```

- [ ] **Step 1: Write failing Chinese retrieval and document-isolation tests**

```rust
#[test]
fn retrieves_the_page_containing_a_chinese_business_term() {
    let repo = indexed_repo([(1, "普通介绍"), (2, "西南区域转化率下降")]);
    let results = repo.retrieve_context("doc-1", "西南转化率", 5).unwrap();
    assert_eq!(results[0].page_number, 2);
}

#[test]
fn never_returns_pages_from_another_document() {
    let results = mixed_document_repo().retrieve_context("doc-a", "收入", 5).unwrap();
    assert!(results.iter().all(|page| page.document_id == "doc-a"));
}
```

- [ ] **Step 2: Verify failures**

Run search tests; expected missing indexer/retriever.

- [ ] **Step 3: Implement normalized FTS5 indexing**

Index page text, headings, and visual summaries with document ID and page number as unindexed metadata. Escape FTS query syntax and fall back to tokenized OR matching when a direct phrase query yields no rows. Always filter by document ID.

- [ ] **Step 4: Implement retrieval limits and neighboring context**

Return the top pages, then include at most one adjacent page on each side when budget permits. Deduplicate and sort final context by page number while retaining retrieval scores for diagnostics.

- [ ] **Step 5: Run tests and commit**

```powershell
cargo test --manifest-path apps/document-assistant/src-tauri/Cargo.toml search
git add apps/document-assistant/src-tauri
git commit -m "feat: retrieve relevant document pages"
```

### Task 2: Implement continuous cited document chat

**Files:**
- Create: `apps/document-assistant/src-tauri/src/chat/service.rs`
- Create: `apps/document-assistant/src-tauri/src/chat/prompts.rs`
- Create: `apps/document-assistant/src-tauri/src/chat/service_test.rs`
- Create: `apps/document-assistant/src-tauri/src/db/conversations.rs`
- Modify: `apps/document-assistant/src-tauri/src/commands.rs`
- Create: `apps/document-assistant/src/features/chat/DocumentChat.tsx`
- Create: `apps/document-assistant/src/features/chat/DocumentChat.test.tsx`

**Interfaces:**
- Consumes: text model, retrieval service, document summary, last conversation messages.
- Produces: command `ask_document(document_id, conversation_id, question) -> CitedAnswer`.

```rust
pub struct CitedAnswer {
    pub answer: String,
    pub citations: Vec<EvidenceRef>,
    pub grounded: bool,
}
```

- [ ] **Step 1: Write failing grounding tests**

```rust
#[tokio::test]
async fn returns_not_found_when_retrieval_has_no_evidence() {
    let answer = chat_with_empty_retrieval().ask("董事长生日？").await.unwrap();
    assert_eq!(answer.answer, "文档中未找到相关信息");
    assert!(!answer.grounded);
}

#[tokio::test]
async fn rejects_citations_outside_supplied_context() {
    let result = chat_with_model_citing_page(99).ask("增长来源？").await;
    assert!(matches!(result, Err(ChatError::InvalidCitation(99))));
}
```

- [ ] **Step 2: Verify failures**

Run chat service tests; expected missing service.

- [ ] **Step 3: Implement adaptive chat context**

If the complete document context fits the profile budget, supply it. Otherwise retrieve pages with FTS5 and add the document summary. Include a bounded number of recent conversation turns. Require CitedAnswer JSON and validate citations are within supplied page numbers.

- [ ] **Step 4: Implement persistent chat UI**

Render user/assistant turns, pending state, retry, new conversation, and citation buttons. Pressing Enter sends; Shift+Enter adds a newline. Citation buttons call the shared selected-page action. Never show a citation button for invalid or missing pages.

- [ ] **Step 5: Run tests and commit**

```powershell
cargo test --manifest-path apps/document-assistant/src-tauri/Cargo.toml chat
npm test -- --run src/features/chat/DocumentChat.test.tsx
git add apps/document-assistant
git commit -m "feat: add cited document conversations"
```

### Task 3: Build source-page verification and synchronized result tabs

**Files:**
- Create: `apps/document-assistant/src/features/source/SourceViewer.tsx`
- Create: `apps/document-assistant/src/features/source/SourceViewer.test.tsx`
- Create: `apps/document-assistant/src/features/source/useSelectedPage.ts`
- Modify: `apps/document-assistant/src/features/results/InsightsView.tsx`
- Modify: `apps/document-assistant/src/features/results/ResultTabs.tsx`
- Modify: `apps/document-assistant/src/features/chat/DocumentChat.tsx`

**Interfaces:**
- Produces: `SelectedPageProvider`, `useSelectedPage(): {pageNumber, selectPage}`, and source view showing image, page text, visual analysis, uncertainty, and failure state.

- [ ] **Step 1: Write failing citation navigation test**

```tsx
it('opens the cited page in source verification view', async () => {
  render(<DocumentWorkspace document={sampleDocument} />);
  await userEvent.click(screen.getByRole('button', { name: '第 8 页' }));
  expect(screen.getByRole('tab', { name: '原页核对' })).toHaveAttribute('aria-selected', 'true');
  expect(screen.getByAltText('第 8 页')).toBeVisible();
});
```

- [ ] **Step 2: Verify failure**

Run source viewer tests; expected missing shared selection state.

- [ ] **Step 3: Implement synchronized tabs and viewer**

Keep the selected page at workspace scope. Source view has previous/next buttons, thumbnail rail, zoom, and toggles for page text and visual JSON. Missing render shows text-only explanation; failed vision shows retry action without hiding the page image.

- [ ] **Step 4: Run component and keyboard tests**

Expected: citation navigation, page buttons, and tabs work via keyboard and expose proper ARIA states.

- [ ] **Step 5: Commit source verification**

```powershell
git add apps/document-assistant/src
git commit -m "feat: verify insights against source pages"
```

### Task 4: Generate Markdown and JSON exports

**Files:**
- Create: `apps/document-assistant/src-tauri/src/export/mod.rs`
- Create: `apps/document-assistant/src-tauri/src/export/markdown.rs`
- Create: `apps/document-assistant/src-tauri/src/export/json.rs`
- Create: `apps/document-assistant/src-tauri/src/export/export_test.rs`
- Modify: `apps/document-assistant/src-tauri/src/commands.rs`
- Create: `apps/document-assistant/src/features/export/ExportButtons.tsx`
- Create: `apps/document-assistant/src/features/export/ExportButtons.test.tsx`

**Interfaces:**
- Produces: `export_document(document_id, format, destination) -> ExportReceipt`, formats `original_markdown | enhanced_markdown | structured_json`.

- [ ] **Step 1: Write failing export snapshot tests**

```rust
#[test]
fn enhanced_markdown_includes_citations_failures_and_model_metadata() {
    let output = render_enhanced_markdown(&sample_export()).unwrap();
    assert!(output.contains("来源：第 8 页"));
    assert!(output.contains("失败页面：第 13 页"));
    assert!(output.contains("视觉模型"));
}

#[test]
fn exported_json_never_contains_credentials() {
    let output = render_json(&sample_export()).unwrap();
    assert!(!output.to_lowercase().contains("api_key"));
    assert!(!output.contains("sk-test"));
}
```

- [ ] **Step 2: Verify failures**

Run export tests; expected missing exporter.

- [ ] **Step 3: Implement deterministic export schemas**

Use persisted data only; do not call a model during export. Enhanced Markdown sections are metadata, summary, outline, key facts, risks, actions, page analyses, limitations, and failed pages. JSON uses a versioned root field `schemaVersion: 1`.

- [ ] **Step 4: Implement atomic save and frontend controls**

Open a Tauri save dialog, write to a temporary sibling, flush, then rename. Prevent overwrite without explicit confirmation. Show destination and byte count on success.

- [ ] **Step 5: Run tests and commit**

```powershell
cargo test --manifest-path apps/document-assistant/src-tauri/Cargo.toml export
npm test -- --run src/features/export/ExportButtons.test.tsx
git add apps/document-assistant
git commit -m "feat: export analyzed documents"
```

### Task 5: Complete recovery, error UX, and security regression tests

**Files:**
- Create: `apps/document-assistant/src/features/errors/ErrorNotice.tsx`
- Create: `apps/document-assistant/src/features/errors/ErrorNotice.test.tsx`
- Create: `apps/document-assistant/src/features/tasks/RecoveryDialog.tsx`
- Create: `apps/document-assistant/src-tauri/tests/recovery.rs`
- Create: `apps/document-assistant/src-tauri/tests/security.rs`
- Modify: `apps/document-assistant/src-tauri/src/tasks/runner.rs`

**Interfaces:**
- Consumes: task error codes and persisted incomplete tasks.
- Produces: recovery confirmation and actionable error mapping for encrypted, malformed, unsupported, renderer, auth, rate-limit, network, invalid-JSON, and partial-page failures.

- [ ] **Step 1: Write failing restart and secret-scan tests**

```rust
#[tokio::test]
async fn restart_offers_resume_without_repeating_completed_pages() {
    let app = test_app_with_interrupted_task([1, 2]);
    let pending = app.recoverable_tasks().await.unwrap();
    assert_eq!(pending[0].remaining_pages, vec![3, 4]);
}

#[test]
fn persisted_files_and_logs_do_not_contain_secret() {
    let fixture = run_failed_request_with_secret("sk-regression-secret");
    assert!(!fixture.all_persisted_text().contains("sk-regression-secret"));
}
```

- [ ] **Step 2: Verify failures**

Run integration tests; expected missing recovery API/error mapping.

- [ ] **Step 3: Implement recovery dialog and explicit choices**

On startup, list recoverable tasks with file name, stage, completed pages, and actions `继续`, `稍后`, `取消任务`. Do not resume network calls before user confirmation.

- [ ] **Step 4: Implement actionable errors**

Map backend codes to user messages and actions. For 401/403 show `检查模型配置`; for 429 show next retry time; for encrypted files show `请先解除文件密码保护`; for renderer absence show `继续纯文本分析`.

- [ ] **Step 5: Run full regression suite and commit**

```powershell
npm test -- --run
cargo test --manifest-path apps/document-assistant/src-tauri/Cargo.toml
git add apps/document-assistant
git commit -m "feat: recover tasks and explain failures"
```

### Task 6: Package and verify the Windows installer

**Files:**
- Modify: `apps/document-assistant/src-tauri/tauri.conf.json`
- Create: `apps/document-assistant/src-tauri/resources/THIRD_PARTY_NOTICES.md`
- Create: `apps/document-assistant/src-tauri/resources/licenses/anydoc-MIT.txt`
- Create: `apps/document-assistant/src-tauri/resources/licenses/libreoffice-MPL-2.0.txt`
- Create: `apps/document-assistant/src-tauri/resources/licenses/pdfium-BSD.txt`
- Create: `apps/document-assistant/scripts/verify-installer.ps1`
- Create: `apps/document-assistant/e2e/document-flow.spec.ts`
- Create: `apps/document-assistant/playwright.config.ts`

**Interfaces:**
- Produces: signed-or-unsigned local Windows installer artifact, runtime dependency manifest, and repeatable clean-machine verification script.

- [ ] **Step 1: Write failing packaged-resource checks**

```powershell
$required = @(
  'resources\licenses\anydoc-MIT.txt',
  'resources\licenses\libreoffice-MPL-2.0.txt',
  'resources\licenses\pdfium-BSD.txt'
)
$required | ForEach-Object {
  if (-not (Test-Path -LiteralPath $_)) { throw "Missing packaged notice: $_" }
}
```

- [ ] **Step 2: Verify the packaging check fails**

Run: `powershell -ExecutionPolicy Bypass -File scripts/verify-installer.ps1`

Expected: FAIL because notices and installer are not packaged.

- [ ] **Step 3: Configure runtime resources and installer options**

Bundle PDFium. Make the LibreOffice Headless component selected by default but removable; record the selected runtime in diagnostics. Include all licenses in the About screen and installed resources. Do not bundle user model keys or test `.env` files.

- [ ] **Step 4: Add the end-to-end document flow**

The Playwright/Tauri-compatible test imports a fixture, waits for local conversion, injects mock model responses, verifies insights, asks a cited question, navigates to the cited page, and exports Markdown/JSON.

- [ ] **Step 5: Build and verify**

Run:

```powershell
Set-Location apps\document-assistant
npm test -- --run
cargo test --manifest-path src-tauri\Cargo.toml
npm run build
npm run tauri build
powershell -ExecutionPolicy Bypass -File scripts\verify-installer.ps1
```

Expected: installer exists, resource checks pass, and no development runtime is required.

- [ ] **Step 6: Perform a clean Windows smoke test**

On a Windows VM or separate clean user profile without Node, Rust, or Python: install, launch, import DOCX/PPTX/PDF samples, run mocked or real configured model analysis, restart during a task, resume, chat, and export. Record results in `docs/testing/windows-smoke-test.md`.

- [ ] **Step 7: Commit Windows delivery**

```powershell
git add apps/document-assistant docs/testing
git commit -m "build: package AnyDoc Assistant for Windows"
```

## Final Completion Gate

Run the complete upstream AnyDoc tests appropriate to the machine, the document-assistant Rust and frontend suites, E2E flow, production build, installer verification, and clean-Windows smoke test. The release is complete only when source parsing, page rendering or explicit text-only fallback, dual-model analysis, cited continuous chat, recovery, exports, credential protection, license notices, and Windows installation all pass.
