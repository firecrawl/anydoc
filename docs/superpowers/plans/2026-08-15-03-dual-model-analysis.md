# AnyDoc Assistant Dual-Model Analysis Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add configurable OpenAI-compatible vision and text providers, page-level structured visual analysis, document synthesis, partial-failure recovery, and visible evidence-linked results.

**Architecture:** All HTTP requests originate in the Tauri backend, which resolves secrets from Windows Credential Manager. Provider-independent services accept typed requests and validate typed JSON responses. Vision is page-parallel with bounded concurrency; text synthesis is hierarchical and consumes AnyDoc text plus persisted page analyses.

**Tech Stack:** Rust, Reqwest, Tokio, Serde, Schemars, Jsonschema, Tauri events, React/TypeScript, Vitest, WireMock.

## Global Constraints

- Vision and text models may be separate or the same profile.
- Never send an image to a profile with `supports_vision = false`.
- The generic vision transport is an OpenAI-style `image_url` containing a Base64 Data URI; no document image is uploaded to a third-party hosting service. A profile must pass the vision capability test before analysis.
- Every derived claim must keep a page or slide reference when evidence exists.
- A failed page must not fail the remaining document.
- 401/403 stop new requests; 429 and transient 5xx use bounded backoff.
- Raw secrets and Authorization headers must never enter logs or frontend payloads.

---

### Task 1: Implement an OpenAI-compatible backend client

**Files:**
- Create: `apps/document-assistant/src-tauri/src/models/mod.rs`
- Create: `apps/document-assistant/src-tauri/src/models/types.rs`
- Create: `apps/document-assistant/src-tauri/src/models/openai_compatible.rs`
- Create: `apps/document-assistant/src-tauri/src/models/retry.rs`
- Create: `apps/document-assistant/src-tauri/src/models/openai_compatible_test.rs`
- Modify: `apps/document-assistant/src-tauri/src/commands.rs`

**Interfaces:**
- Consumes: `ModelProfile` and `SecretStore` from Plan 2.
- Produces: `ModelClient::chat(profile, messages, response_schema) -> Result<ModelResponse>` and `test_model_profile` command.

- [ ] **Step 1: Write failing WireMock request tests**

```rust
#[tokio::test]
async fn sends_openai_compatible_chat_request_without_leaking_key() {
    let server = MockServer::start().await;
    mount_chat_success(&server, r#"{"ok":true}"#).await;
    let client = test_client(&server, "secret-key");
    let response = client.chat(sample_request()).await.unwrap();
    assert_eq!(response.content, r#"{"ok":true}"#);
    assert!(!format!("{response:?}").contains("secret-key"));
}
```

Add tests for endpoint joining, timeout, 401, 429 with `Retry-After`, and 500 retry exhaustion.

- [ ] **Step 2: Run tests and verify failure**

Run: `cargo test --manifest-path apps/document-assistant/src-tauri/Cargo.toml models::openai_compatible`

Expected: FAIL because client modules are missing.

- [ ] **Step 3: Implement typed request construction**

Use `POST {base_url}/chat/completions`, bearer authentication, text content, and OpenAI-style `image_url` content. Normalize base URLs that already end in `/v1`. Limit response body size. Redact secrets from all error types.

- [ ] **Step 4: Implement bounded retry policy**

Retry network errors, timeout, 429, 500, 502, 503, and 520 up to three attempts using server `Retry-After` when present, otherwise exponential delay with jitter. Do not retry 400, 401, 403, or 404.

- [ ] **Step 5: Verify contract tests and commit**

Run model tests, then:

```powershell
git add apps/document-assistant/src-tauri
git commit -m "feat: call OpenAI-compatible models safely"
```

### Task 2: Define and validate page analysis schemas

**Files:**
- Create: `apps/document-assistant/src-tauri/src/analysis/types.rs`
- Create: `apps/document-assistant/src-tauri/src/analysis/schema.rs`
- Create: `apps/document-assistant/src-tauri/src/analysis/schema_test.rs`
- Create: `apps/document-assistant/src/lib/analysis/types.ts`

**Interfaces:**
- Produces: matching Rust/TypeScript types `PageAnalysis`, `EvidenceRef`, `UncertainItem`, `DocumentSummary`, and `CitedAnswer`.

```rust
pub struct PageAnalysis {
    pub page_number: u32,
    pub title: Option<String>,
    pub summary: String,
    pub visual_elements: Vec<VisualElement>,
    pub logical_relations: Vec<LogicalRelation>,
    pub key_facts: Vec<String>,
    pub uncertain_items: Vec<UncertainItem>,
    pub confidence: f32,
}
```

- [ ] **Step 1: Write failing validation tests**

```rust
#[test]
fn rejects_confidence_outside_zero_to_one() {
    let json = page_analysis_json_with_confidence(1.5);
    assert!(parse_page_analysis(&json).is_err());
}

#[test]
fn rejects_a_page_number_mismatch() {
    assert!(validate_page_number(&sample_analysis(3), 4).is_err());
}
```

- [ ] **Step 2: Verify failure**

Run schema tests; expected missing types and parser.

- [ ] **Step 3: Implement schemas and parser**

Use Serde deny-unknown-fields where safe, validate confidence range, require non-empty summary, and validate all evidence page numbers against document page count. Generate JSON Schema for model requests. Mirror field names in TypeScript using camelCase at the Tauri boundary.

- [ ] **Step 4: Run Rust tests and TypeScript typecheck**

Expected: schemas reject malformed data and `npm run build` passes.

- [ ] **Step 5: Commit analysis contracts**

```powershell
git add apps/document-assistant
git commit -m "feat: define structured analysis schemas"
```

### Task 3: Implement per-page vision analysis with text-only degradation

**Files:**
- Create: `apps/document-assistant/src-tauri/src/analysis/prompts.rs`
- Create: `apps/document-assistant/src-tauri/src/analysis/vision.rs`
- Create: `apps/document-assistant/src-tauri/src/analysis/vision_test.rs`
- Modify: `apps/document-assistant/src-tauri/src/tasks/runner.rs`
- Modify: `apps/document-assistant/src-tauri/src/db/documents.rs`

**Interfaces:**
- Consumes: rendered page image, page text, nearby headings, `ModelClient`, `PageAnalysis` schema.
- Produces: `VisionAnalyzer::analyze_page(input) -> Result<PageAnalysis>` and persisted page analysis.

- [ ] **Step 1: Write failing prompt and degradation tests**

```rust
#[tokio::test]
async fn refuses_to_send_images_to_text_only_profiles() {
    let analyzer = analyzer_with_profile(profile(false));
    let error = analyzer.analyze_page(sample_page()).await.unwrap_err();
    assert!(matches!(error, AnalysisError::VisionUnsupported));
}

#[test]
fn prompt_requires_uncertainty_instead_of_guessing() {
    let prompt = build_page_prompt(&sample_input());
    assert!(prompt.contains("无法确定"));
    assert!(prompt.contains("uncertain_items"));
}
```

- [ ] **Step 2: Verify failures**

Run vision tests; expected missing analyzer.

- [ ] **Step 3: Implement image preparation and structured request**

Read PNG from the document cache, enforce configured pixel and byte limits, and send as the provider-supported image URL/Data URI representation. Include page text and only the nearest preceding/following heading context. Require the generated PageAnalysis JSON Schema.

- [ ] **Step 4: Add one repair pass for invalid JSON**

On schema failure, send the raw response plus validation errors to the same profile with a strict repair prompt and no page image. Persist the raw response when the repair also fails; mark only that page failed.

- [ ] **Step 5: Add bounded page concurrency and checkpoints**

Use a Tokio semaphore based on `max_concurrency`. Persist each successful result before emitting `task-progress`. On missing vision configuration, record document capability `visual_content_analyzed = false` and continue to synthesis with AnyDoc text.

- [ ] **Step 6: Run tests and commit**

Run vision, task, and model contract tests, then commit:

```powershell
git add apps/document-assistant/src-tauri
git commit -m "feat: analyze document pages with vision models"
```

### Task 4: Implement hierarchical document synthesis

**Files:**
- Create: `apps/document-assistant/src-tauri/src/analysis/synthesis.rs`
- Create: `apps/document-assistant/src-tauri/src/analysis/synthesis_test.rs`
- Modify: `apps/document-assistant/src-tauri/src/analysis/prompts.rs`
- Modify: `apps/document-assistant/src-tauri/src/tasks/runner.rs`

**Interfaces:**
- Consumes: AnyDoc Markdown, document structure, notes, tables, successful `PageAnalysis` rows, failed page list, text model profile.
- Produces: `DocumentSynthesizer::summarize(input) -> Result<DocumentSummary>`.

- [ ] **Step 1: Write failing short/long-document routing tests**

```rust
#[tokio::test]
async fn short_documents_use_one_synthesis_request() {
    let model = recording_model();
    synthesizer(model.clone(), budget(50_000)).summarize(short_document()).await.unwrap();
    assert_eq!(model.call_count(), 1);
}

#[tokio::test]
async fn long_documents_create_section_summaries_before_final_summary() {
    let model = recording_model();
    synthesizer(model.clone(), budget(2_000)).summarize(long_document()).await.unwrap();
    assert!(model.call_count() > 1);
}
```

- [ ] **Step 2: Verify failures**

Run synthesis tests; expected missing synthesizer.

- [ ] **Step 3: Implement deterministic context budgeting**

Estimate tokens conservatively from Unicode character counts. Short inputs use one request. Long inputs group contiguous pages by headings, summarize sections, then synthesize the section summaries. Every section summary carries page ranges and final citations must be a subset of those ranges.

- [ ] **Step 4: Include partial-analysis disclosure**

If vision is disabled or pages failed, `DocumentSummary` includes `analysisLimitations` listing the exact condition and page numbers. Do not describe the result as complete visual analysis.

- [ ] **Step 5: Run tests and commit**

```powershell
cargo test --manifest-path apps/document-assistant/src-tauri/Cargo.toml analysis::synthesis
git add apps/document-assistant/src-tauri
git commit -m "feat: synthesize document-wide insights"
```

### Task 5: Build settings, progress, and intelligent-insights UI

**Files:**
- Modify: `apps/document-assistant/src/features/settings/ModelSettings.tsx`
- Create: `apps/document-assistant/src/features/settings/modelTestApi.ts`
- Create: `apps/document-assistant/src/features/settings/ModelSettings.integration.test.tsx`
- Create: `apps/document-assistant/src/features/results/InsightsView.tsx`
- Create: `apps/document-assistant/src/features/results/InsightsView.test.tsx`
- Create: `apps/document-assistant/src/features/results/AnalysisLimitations.tsx`
- Create: `apps/document-assistant/src/features/results/StructuredJsonView.tsx`
- Create: `apps/document-assistant/src/features/results/StructuredJsonView.test.tsx`
- Modify: `apps/document-assistant/src/App.tsx`

**Interfaces:**
- Consumes: model profile commands, task progress events, `DocumentSummary`.
- Produces: dual-profile settings UX and evidence-linked insights view.

- [ ] **Step 1: Write failing UI tests**

```tsx
it('shows separate vision and text model profiles', () => {
  render(<ModelSettings />);
  expect(screen.getByRole('heading', { name: '视觉模型' })).toBeVisible();
  expect(screen.getByRole('heading', { name: '文本模型' })).toBeVisible();
});

it('shows a visual-analysis limitation', () => {
  render(<InsightsView summary={summaryWithoutVision} />);
  expect(screen.getByText(/视觉内容未分析/)).toBeVisible();
});

it('renders versioned structured analysis JSON', () => {
  render(<StructuredJsonView value={{ schemaVersion: 1, summary: sampleSummary }} />);
  expect(screen.getByText(/"schemaVersion": 1/)).toBeVisible();
});
```

- [ ] **Step 2: Verify failures**

Run the two component test files; expected missing UI.

- [ ] **Step 3: Implement connectivity and capability tests**

The text test asks for a fixed short JSON response. The vision test sends a bundled 1x1 or tiny labeled fixture and requires a fixed answer. Save a profile only after URL validation; capability-test failure does not erase existing settings. Before the first remote analysis for each document, show a confirmation listing whether extracted text, page images, or both will be sent; persist only the user's consent timestamp and selected profile IDs, never the key.

- [ ] **Step 4: Implement insights view**

Render document theme, logical outline, key facts, risks, action items, limitations, and citation buttons. Citation buttons emit `navigate-to-page` with the page number for Plan 4's source viewer. `StructuredJsonView` displays formatted persisted JSON, offers copy, and clearly labels raw model responses from failed schema repairs as diagnostic data rather than valid analysis.

- [ ] **Step 5: Run tests, desktop smoke test, and commit**

```powershell
npm test -- --run
npm run build
npm run tauri dev
git add apps/document-assistant
git commit -m "feat: present dual-model document insights"
```

## Plan 3 Completion Gate

Pass criteria: users can configure separate or shared profiles, secrets stay backend-only, vision analysis persists per page with bounded retries, text synthesis supports short and long documents, partial failures are disclosed, and insights link to evidence pages.
