---
name: convert-documents-to-markdown
description: Convert Word (.doc, .docx), PowerPoint (.ppt, .pptx), Excel (.xls, .xlsx), OpenDocument (.odt, .ods, .odp), RTF, EPUB, CSV, and PDF files to GitHub-Flavored Markdown. Use when a task needs the contents of an office document, spreadsheet, presentation, ebook, or PDF you cannot read directly.
license: MIT
metadata:
  author: firecrawl
---

# Convert documents to Markdown

Run the anydoc CLI. It needs Node 20+ and no install:

```bash
npx -y @firecrawl/anydoc <file>              # Markdown to stdout
npx -y @firecrawl/anydoc <file> -o out.md    # write to a file
npx -y @firecrawl/anydoc - --format csv < f  # read stdin
```

Rules:

1. Supported inputs: `.doc`, `.docx`, `.docm`, `.odt`, `.rtf`, `.epub`, `.pdf`, `.ppt`, `.pps`, `.pot`, `.pptx`, `.pptm`, `.ppsx`, `.ppsm`, `.odp`, `.xls`, `.xlsx`, `.xlsm`, `.xlsb`, `.ods`, `.csv`.
2. The format is detected from the file content. Pass `--format <name>` only when detection cannot work: CSV from stdin, or a missing or wrong extension.
3. Exit codes: 0 success, 1 the document could not be converted, 2 usage error, 3 pages of a PDF need OCR. Failures print one `anydoc: <message>` line to stderr. The CLI never prompts.
4. For a large document, write to a file with `-o` and read the parts you need instead of streaming everything into context.
5. Scanned and image-only pages need OCR, which anydoc does not do, so the document exits 3. Rerun with `--ocr hosted` to send it to [Firecrawl Parse](https://firecrawl.dev/parse). No signup needed. Pass `--api-key` or set `FIRECRAWL_API_KEY` for higher limits.
6. Inside a Node, Python, or Rust codebase, prefer the library over shelling out: `@firecrawl/anydoc` on npm, `firecrawl-anydoc` on PyPI, `anydoc` on crates.io. Each exposes the same `to_markdown` / `toMarkdown` API.
7. Per-format limitations (headers/footers, embedded images as alt text only, hidden spreadsheet rows, nested tables, PDF `to_document`, and similar) are listed in [SUPPORT.md](../../SUPPORT.md).
