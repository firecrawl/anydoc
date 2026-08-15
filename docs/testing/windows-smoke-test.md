# AnyDoc Assistant Windows Smoke Test

Date: 2026-08-15

## Automated on the build machine

- Frontend unit/integration suite: passed.
- Rust unit, recovery, security, model transport, retrieval, chat, and export suites: passed.
- AnyDoc WebAssembly release build: passed.
- Microsoft Word DOCX and PowerPoint PPTX conversion to PDF and non-empty PNG pages: passed on the installed Office applications.
- Tauri release executable launch for five seconds without early exit: passed.
- NSIS installer creation and packaged resource/credential scan: passed.
- Installer artifact: `AnyDoc Assistant_0.1.0_x64-setup.exe`.

## Runtime assumptions

- PDFium is bundled, so PDF page rendering does not need a development runtime.
- Node.js, Rust, Python, and Codex are not required on an installed machine.
- Word/PowerPoint page rendering uses installed Microsoft Office first, then an installed LibreOffice fallback. If neither is present, the app explicitly offers text-only analysis.
- Model requests require user-provided OpenAI-compatible endpoint, model name, and API Key. Keys are stored through Windows Credential Manager.

## Separate clean-PC acceptance checklist

The installer was built on the development machine, not inside a separate clean Windows VM. Before broad distribution, run this short acceptance check on one colleague PC:

1. Install without Node.js, Rust, or Python.
2. Launch and import one DOCX, PPTX, and PDF.
3. Confirm local Markdown and page images or the explicit text-only fallback.
4. Configure test vision/text profiles, approve remote sending, and verify insights.
5. Ask a cited question and open its source page.
6. Export enhanced Markdown and structured JSON.
7. Interrupt one task, restart, and choose `继续` in the recovery dialog.

This checklist is intentionally not marked as passed until it is run on a separate machine.
