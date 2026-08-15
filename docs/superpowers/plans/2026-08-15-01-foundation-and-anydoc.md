# AnyDoc Assistant Foundation and Local Parsing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Merge the AnyDoc upstream project into this repository and deliver a runnable Tauri/React Windows shell that preserves the AnyDoc Web Demo experience and converts local documents through the repository's own WASM build.

**Architecture:** Keep upstream parser folders intact and add `apps/document-assistant` as a separate application boundary. React renders the enhanced AnyDoc UI; a Web Worker owns the local WASM parser so large files do not block the UI. Tauri provides the Windows shell but does not yet call remote models.

**Tech Stack:** AnyDoc Rust/WASM, wasm-pack, React, TypeScript, Vite, Vitest, Testing Library, Tauri 2, Rust.

## Global Constraints

- Target platform is Windows 10/11.
- Project root is `D:\项目保存地址\各应用转md文件应用`.
- Preserve the AnyDoc Rust parser and upstream test layout; new product code belongs under `apps/document-assistant`.
- Retain AnyDoc MIT copyright and license notices.
- Files must convert locally and no model or network request is part of this plan.
- Use test-first steps and commit after each independently testable task.

---

### Task 1: Merge AnyDoc upstream and establish the application boundary

**Files:**
- Modify: `.git/config` via Git remote commands
- Create: `apps/document-assistant/package.json`
- Create: `apps/document-assistant/tsconfig.json`
- Create: `apps/document-assistant/vite.config.ts`
- Create: `apps/document-assistant/index.html`
- Create: `apps/document-assistant/src/main.tsx`
- Create: `apps/document-assistant/src/App.tsx`
- Create: `apps/document-assistant/src/App.test.tsx`
- Create: `apps/document-assistant/src/test/setup.ts`
- Create: `apps/document-assistant/src-tauri/Cargo.toml`
- Create: `apps/document-assistant/src-tauri/tauri.conf.json`
- Create: `apps/document-assistant/src-tauri/src/main.rs`
- Create: `apps/document-assistant/src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: approved design specification at `docs/superpowers/specs/2026-08-15-anydoc-document-assistant-design.md`.
- Produces: `App` React component, Tauri application named `AnyDoc Assistant`, and local AnyDoc WASM package at `wasm/pkg`.

- [ ] **Step 1: Merge upstream without discarding the existing design commit**

Run:

```powershell
git remote add upstream https://github.com/firecrawl/anydoc.git
git fetch upstream main
git merge --allow-unrelated-histories --no-edit upstream/main
```

Expected: the repository contains upstream `src`, `wasm`, `node`, `python`, and `tests` folders plus the existing `docs/superpowers` files.

- [ ] **Step 2: Build AnyDoc's local WASM package**

Run:

```powershell
wasm-pack build wasm --release --target web --scope firecrawl
```

Expected: `wasm/pkg/package.json`, JavaScript bindings, TypeScript declarations, and `.wasm` binary exist.

- [ ] **Step 3: Create the package and test harness, then install dependencies**

Use this package script contract in `package.json`:

```json
{
  "scripts": {
    "build:anydoc": "wasm-pack build ../../wasm --release --target web --scope firecrawl",
    "dev": "vite",
    "build": "tsc -b && vite build",
    "test": "vitest",
    "tauri": "tauri"
  },
  "dependencies": {
    "@firecrawl/anydoc-wasm": "file:../../wasm/pkg",
    "@tauri-apps/api": "^2",
    "react": "^19",
    "react-dom": "^19"
  }
}
```

Create `vite.config.ts`, `tsconfig.json`, `index.html`, and `src/test/setup.ts` with Vitest `jsdom` and `@testing-library/jest-dom/vitest`, but do not create `App.tsx` yet. Then run `npm install` from `apps/document-assistant`.

- [ ] **Step 4: Write the failing application smoke test**

Create `src/App.test.tsx`:

```tsx
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from './App';

describe('App', () => {
  it('renders the AnyDoc Assistant shell', () => {
    render(<App />);
    expect(screen.getByRole('heading', { name: /Any document in/i })).toBeVisible();
    expect(screen.getByRole('button', { name: /模型设置/i })).toBeVisible();
  });
});
```

- [ ] **Step 5: Run the test and verify failure**

Run: `npm test -- --run src/App.test.tsx`

Expected: FAIL because `App`, the test environment, or package scripts do not exist.

- [ ] **Step 6: Create minimal React/Vite/Tauri implementation**

Implement `App.tsx` with an `<h1>Any document in.</h1>` and a `模型设置` button. Add `main.tsx`, `src-tauri/Cargo.toml`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`, and `src-tauri/tauri.conf.json`. Configure Tauri `beforeDevCommand` as `npm run dev` and `beforeBuildCommand` as `npm run build:anydoc && npm run build`.

- [ ] **Step 7: Verify shell tests**

Run:

```powershell
Set-Location apps\document-assistant
npm install
npm test -- --run
npm run build
cargo test --manifest-path src-tauri\Cargo.toml
```

Expected: all tests pass and Vite produces `dist`.

- [ ] **Step 8: Commit the foundation**

```powershell
git add apps/document-assistant
git commit -m "feat: add AnyDoc Assistant application shell"
```

### Task 2: Recreate the AnyDoc Demo visual language as modular components

**Files:**
- Create: `apps/document-assistant/src/styles/tokens.css`
- Create: `apps/document-assistant/src/styles/app.css`
- Create: `apps/document-assistant/src/components/AppHeader.tsx`
- Create: `apps/document-assistant/src/components/AppHeader.test.tsx`
- Create: `apps/document-assistant/src/features/import/DocumentDropZone.tsx`
- Create: `apps/document-assistant/src/features/import/DocumentDropZone.test.tsx`
- Create: `apps/document-assistant/src/features/results/ResultTabs.tsx`
- Create: `apps/document-assistant/src/features/results/ResultTabs.test.tsx`
- Modify: `apps/document-assistant/src/App.tsx`
- Copy with attribution: `wasm/www/assets/*` to `apps/document-assistant/public/assets/anydoc/*`

**Interfaces:**
- Consumes: `App` from Task 1.
- Produces: `DocumentDropZone({onFile(file): void})`, `ResultTabs({active,onChange})`, and accessible navigation buttons.

- [ ] **Step 1: Write failing interaction tests**

```tsx
it('passes the selected document to onFile', async () => {
  const onFile = vi.fn();
  render(<DocumentDropZone onFile={onFile} />);
  const file = new File(['demo'], 'demo.docx');
  await userEvent.upload(screen.getByLabelText(/选择文档/i), file);
  expect(onFile).toHaveBeenCalledWith(file);
});

it('changes result views', async () => {
  const onChange = vi.fn();
  render(<ResultTabs active="markdown" onChange={onChange} />);
  await userEvent.click(screen.getByRole('tab', { name: '智能解读' }));
  expect(onChange).toHaveBeenCalledWith('insights');
});
```

- [ ] **Step 2: Verify the tests fail**

Run: `npm test -- --run src/features/import/DocumentDropZone.test.tsx src/features/results/ResultTabs.test.tsx`

Expected: FAIL because the components are missing.

- [ ] **Step 3: Implement the modular Demo UI**

Define the tab type exactly:

```ts
export type ResultView = 'insights' | 'markdown' | 'source' | 'chat' | 'json';
```

`DocumentDropZone` must expose a hidden file input with `aria-label="选择文档"`, accept one file, support keyboard activation, and handle `drop`. Reuse the official variables `--heat: #fa5d19`, `--bg: #f9f9f9`, and Demo font fallbacks. Add an attribution link in About rather than removing upstream attribution.

- [ ] **Step 4: Run UI tests and accessibility smoke checks**

Run:

```powershell
npm test -- --run
npm run build
```

Expected: PASS; no TypeScript errors; every tab is reachable by role and name.

- [ ] **Step 5: Commit the modular UI**

```powershell
git add apps/document-assistant
git commit -m "feat: extend AnyDoc demo interface"
```

### Task 3: Add the AnyDoc Web Worker conversion boundary

**Files:**
- Create: `apps/document-assistant/src/lib/anydoc/types.ts`
- Create: `apps/document-assistant/src/lib/anydoc/anydoc.worker.ts`
- Create: `apps/document-assistant/src/lib/anydoc/client.ts`
- Create: `apps/document-assistant/src/lib/anydoc/client.test.ts`
- Create: `apps/document-assistant/src/features/import/useDocumentConversion.ts`
- Create: `apps/document-assistant/src/features/import/useDocumentConversion.test.tsx`
- Modify: `apps/document-assistant/src/App.tsx`

**Interfaces:**
- Consumes: `formatFromBytes`, `formatFromPath`, `toMarkdownBytes`, and `toDocument` from local `@firecrawl/anydoc-wasm`.
- Produces: `convertDocument(file: File): Promise<ConversionResult>` and `ConversionResult` below.

```ts
export interface ConversionResult {
  fileName: string;
  format: string;
  markdown: string;
  characterCount: number;
  elapsedMs: number;
  document: unknown;
}
```

- [ ] **Step 1: Write a failing worker-client test**

```ts
it('resolves a conversion response by request id', async () => {
  const worker = new FakeWorker();
  const client = createAnyDocClient(worker as unknown as Worker);
  const promise = client.convert('notes.rtf', new Uint8Array([1, 2]));
  worker.emit({ id: worker.lastMessage.id, ok: true, result: sampleResult });
  await expect(promise).resolves.toEqual(sampleResult);
});
```

- [ ] **Step 2: Run the test and verify failure**

Run: `npm test -- --run src/lib/anydoc/client.test.ts`

Expected: FAIL because `createAnyDocClient` is missing.

- [ ] **Step 3: Implement request-correlated worker conversion**

Messages must use discriminated payloads:

```ts
type ConvertRequest = { id: string; type: 'convert'; name: string; bytes: Uint8Array };
type ConvertResponse =
  | { id: string; ok: true; result: ConversionResult }
  | { id: string; ok: false; error: { code?: string; message: string } };
```

Initialize WASM once inside the worker. Detect format from bytes, then path. Measure conversion time with `performance.now()`. Call both `toMarkdownBytes` and `toDocument`. Transfer the input `ArrayBuffer` into the worker rather than copying it.

- [ ] **Step 4: Connect conversion state to the enhanced Demo**

`useDocumentConversion` states must be exactly `idle | reading | converting | completed | failed`. Show file name, format, character count, elapsed milliseconds, copy, and Markdown download controls after completion. Surface AnyDoc error `code` and `message` without replacing them with generic text.

- [ ] **Step 5: Run tests and an actual sample conversion**

Run:

```powershell
npm test -- --run
npm run dev
```

Manual check: drop `tests/fixtures/rtf/simple.rtf` or an available upstream RTF fixture; expected Markdown appears while the window remains responsive.

- [ ] **Step 6: Commit local conversion**

```powershell
git add apps/document-assistant
git commit -m "feat: convert documents with AnyDoc worker"
```

### Task 4: Establish the desktop command and event contract

**Files:**
- Create: `apps/document-assistant/src/lib/desktop/types.ts`
- Create: `apps/document-assistant/src/lib/desktop/api.ts`
- Create: `apps/document-assistant/src/lib/desktop/api.test.ts`
- Create: `apps/document-assistant/src-tauri/src/commands.rs`
- Create: `apps/document-assistant/src-tauri/src/events.rs`
- Modify: `apps/document-assistant/src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: Tauri `invoke` and event APIs.
- Produces: `getAppInfo(): Promise<AppInfo>` and event payload `TaskProgressEvent` used by later plans.

```ts
export interface AppInfo { version: string; appDataDir: string; }
export interface TaskProgressEvent {
  documentId: string;
  stage: 'queued' | 'parsing' | 'rendering' | 'vision_analysis' | 'text_synthesis' | 'completed' | 'failed';
  completed: number;
  total: number;
  message: string;
}
```

- [ ] **Step 1: Write failing TypeScript and Rust contract tests**

```ts
it('maps the app_info command response', async () => {
  invokeMock.mockResolvedValue({ version: '0.1.0', appDataDir: 'C:\\Data' });
  await expect(getAppInfo()).resolves.toEqual({ version: '0.1.0', appDataDir: 'C:\\Data' });
});
```

```rust
#[test]
fn app_info_contains_a_version() {
    let info = app_info_for_test("C:\\Data".into());
    assert!(!info.version.is_empty());
}
```

- [ ] **Step 2: Verify both tests fail**

Run:

```powershell
npm test -- --run src/lib/desktop/api.test.ts
cargo test --manifest-path src-tauri\Cargo.toml
```

Expected: FAIL because the command modules are missing.

- [ ] **Step 3: Implement and register the command**

Create a serializable Rust `AppInfo { version, app_data_dir }` with `#[serde(rename_all = "camelCase")]`, register `get_app_info` in `tauri::generate_handler!`, and centralize event name `task-progress` in `events.rs`. Apply the same camelCase serialization rule to `TaskProgressEvent`, while serializing stage enum values as the exact snake_case strings in its TypeScript union. The TypeScript wrapper must be the only frontend module importing Tauri `invoke`.

- [ ] **Step 4: Verify contracts and desktop startup**

Run:

```powershell
npm test -- --run
cargo test --manifest-path src-tauri\Cargo.toml
npm run tauri dev
```

Expected: tests pass; the desktop window opens and local AnyDoc conversion works.

- [ ] **Step 5: Commit the desktop contract**

```powershell
git add apps/document-assistant
git commit -m "feat: define desktop command contracts"
```

## Plan 1 Completion Gate

Run from `apps/document-assistant`:

```powershell
npm test -- --run
npm run build
cargo test --manifest-path src-tauri\Cargo.toml
npm run tauri dev
```

Pass criteria: the Tauri window opens, matches the AnyDoc-enhanced shell, imports a supported file, converts through the locally built AnyDoc WASM package, displays Markdown, and performs no external model call.
