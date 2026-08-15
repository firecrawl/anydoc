# AnyDoc Assistant — Third-Party Notices

This application combines local parsing, rendering, desktop, database, and UI
components. The installer includes the notices in `resources/licenses`.

## AnyDoc

- Project: https://github.com/firecrawl/anydoc
- License: MIT
- Use: local document-to-Markdown parsing, compiled to WebAssembly.

## PDFium

- Project: https://pdfium.googlesource.com/pdfium/
- Binary distributor: https://github.com/bblanchon/pdfium-binaries
- License: BSD-style; PDFium also contains third-party components with their
  own notices in the upstream distribution.
- Bundled build: chromium/7881, NuGet package 151.0.7881.

## LibreOffice

- Project: https://www.libreoffice.org/
- Primary license: Mozilla Public License 2.0.
- LibreOffice is not bundled. If already installed, it may be used as a local
  fallback renderer. Microsoft Office may also be used when installed.

## Rust, Tauri, React, SQLite, and supporting libraries

The application includes compiled open-source dependencies distributed under
their respective permissive or copyleft licenses. Package identities and exact
versions are recorded in `Cargo.lock` and `package-lock.json` in the source
distribution. No API keys, model credentials, or user documents are bundled.
