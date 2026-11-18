# firecrawl-anydoc

[![PyPI](https://img.shields.io/pypi/v/firecrawl-anydoc.svg)](https://pypi.org/project/firecrawl-anydoc/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/firecrawl/anydoc/blob/main/LICENSE)

Convert Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, CSV, and PDF files into clean GitHub-Flavored Markdown. Python bindings for the [anydoc](https://github.com/firecrawl/anydoc) Rust crate, built by [Firecrawl](https://firecrawl.dev). Also available as a hosted API through [Firecrawl Parse](https://firecrawl.dev/parse), which adds our OCR models for the scanned pages anydoc can't read on its own.

Every format parses into one shared document model and renders through a single Markdown serializer, so headings, tables, lists, and footnotes come out the same no matter which format goes in. Conversion releases the GIL, so other threads keep running. Type stubs ship with the package.

```bash
pip install firecrawl-anydoc
```

The package installs as `firecrawl-anydoc` and imports as `anydoc`.

## Supported formats

| Format           | Extensions                                                 |
| ---------------- | ---------------------------------------------------------- |
| Word             | `.doc`, `.docx`, `.docm`                                   |
| PowerPoint       | `.ppt`, `.pps`, `.pot`, `.pptx`, `.pptm`, `.ppsx`, `.ppsm` |
| Excel            | `.xls`, `.xlsx`, `.xlsm`, `.xlsb`                          |
| OpenDocument     | `.odt`, `.ods`, `.odp`                                     |
| Rich Text Format | `.rtf`                                                     |
| EPUB             | `.epub`                                                    |
| CSV              | `.csv`                                                     |
| PDF              | `.pdf`                                                     |

## Usage

```python
import anydoc

# From a file path:
markdown = anydoc.to_markdown("report.docx")

# From bytes, with the format detected from the content:
markdown = anydoc.to_markdown_bytes(data)

# Or name it, which signature-less formats (CSV) need:
markdown = anydoc.to_markdown_bytes(data, "csv")

# Or stop at the document model, which also carries embedded assets:
document = anydoc.to_document(data)
```

## Scanned pages

anydoc converts locally and does not do OCR, so a PDF with scanned or image-only pages raises `NeedsOcrError`. Opt in with `ocr="hosted"` to send that document to [Firecrawl Parse](https://firecrawl.dev/parse). No signup needed. Set `api_key` or `FIRECRAWL_API_KEY` for higher limits.

```python
markdown = anydoc.to_markdown("scan.pdf", ocr="hosted")
```

`parse_mode` picks the Parse PDF mode: `"auto"` (the default) extracts text and falls back to OCR, `"fast"` extracts text only, `"ocr"` OCRs every page. Any other value raises `HostedError` before anything is sent.

```python
markdown = anydoc.to_markdown("scan.pdf", ocr="hosted", parse_mode="ocr")
```

## DocLang export

`to_doclang` and `to_doclang_bytes` export a PDF to DocLang, returned as the `.dclx` OPC ZIP archive: `document.xml` plus the extracted raster parts and the original PDF as the `source` part. Scanned pages export as `<picture>` elements with extracted rasters, so no OCR is needed. Non-PDF formats raise `UnsupportedError`.

```python
# From a file path, or from bytes with the format detected:
dclx = anydoc.to_doclang("report.pdf")
dclx = anydoc.to_doclang_bytes(pdf_bytes)
```

`lists` and `tables` steer structure detection (both on by default); `pages_with_tables` restricts the ruled-table detector to those 1-indexed pages.

```python
dclx = anydoc.to_doclang("report.pdf", lists=False, pages_with_tables=[1, 3])
```

## Errors

A conversion raises only when no complete Markdown could come out of the file. The exception type names what went wrong:

```python
try:
    return anydoc.to_markdown(path)
except (anydoc.EncryptedError, anydoc.UnsupportedError) as error:
    # No document comes out of these, so record the file and take the next one.
    unconverted.append((path, type(error).__name__))
    return None
```

| Exception            | Raised when                                                         |
| -------------------- | ------------------------------------------------------------------- |
| `UnsupportedError`   | Unknown format, or one that cannot be converted                     |
| `NeedsOcrError`      | Scanned or image-only pages of a PDF, listed in `pages`             |
| `MalformedError`     | Structurally unusable: no meaningful content could be extracted     |
| `EncryptedError`     | Encrypted or password-protected                                     |
| `ResourceLimitError` | Crossed a fixed safety limit (decompression, nesting, node count)   |
| `MissingPartError`   | A part required for any meaningful output is absent                 |
| `HostedError`        | `ocr="hosted"` could not get the document through Firecrawl Parse   |
| `OSError`            | The file could not be read, from `to_markdown` only                 |

Every conversion failure subclasses `anydoc.ConvertError`, so catching that handles all of them at once. `MalformedError.part` and `MissingPartError.part` name the package part at fault, `ResourceLimitError.limit` names the limit crossed, and `str(error)` carries the whole message. A `format` argument naming no supported format raises `ValueError`.

## Format detection

The format is read from the file content, using the marker its specification designates: the PDF header, the RTF open group, OLE stream names, the ZIP package mimetype and content types. CSV has no such marker, so detection returns `None` for it and the extension, or an explicit format, names it instead.

```python
anydoc.format_from_bytes(data)  # 'docx', or None when nothing matches
anydoc.format_from_extension(".pptm")  # 'pptx'
anydoc.format_from_path("report.odt")  # 'odt'
```

## Images and embedded objects

Markdown cannot embed bytes, so an embedded image renders as its alt text while the bytes stay on `document.assets`, tagged with a media type and the part they came from. Images that carry an external URL render as ordinary Markdown images.

Full behavior notes and benchmarks live in the [repository README](https://github.com/firecrawl/anydoc#readme).

## License

[MIT](https://github.com/firecrawl/anydoc/blob/main/LICENSE)
