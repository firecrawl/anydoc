# Format support and limitations

anydoc aims for clean, LLM-ready Markdown rather than a pixel-faithful render. Most formats parse into a shared document model and then render through one GFM serializer; PDF is the exception (see below). This page lists what is kept, reduced, or dropped in the current code.

For the extension matrix, see [Supported formats](README.md#supported-formats) in the README.

## Cross-cutting

These apply wherever the shared model and Markdown renderer are used (everything except PDF Markdown extraction).

| Area | Behavior |
| --- | --- |
| Embedded images / OLE objects | Markdown shows **alt text only**. Raw bytes stay on `Document.assets` (with a media type) when the parser retained them. External image URLs become ordinary Markdown images. Tracking: [#63](https://github.com/firecrawl/anydoc/issues/63); open work also explores `asset:N` hrefs and PDF image files. |
| Nested tables | GFM cannot nest tables. An inner table inside a cell is **flattened** into that cell (` / ` between cells, `<br>` between rows). Structure is lost; text is kept. Tracking: [#14](https://github.com/firecrawl/anydoc/issues/14). |
| Page / column breaks | Not represented in Markdown. A break still separates the runs around it so words do not glue together. |
| Encryption | Password-protected / encrypted packages error as `Encrypted`. |
| Recovery | Corrupt optional parts are often skipped with a log; conversion continues when useful content remains. Log messages are not a stable API. |
| Literal `1.` (and similar) at line start | Real lists from source numbering (`w:numPr` and equivalents) become Markdown ordered lists such as `1. item`. When a plain paragraph's text itself starts with `1.` (or lookalikes like `-`, `+`, or `#`) and the source has no list numbering, the marker is escaped (`1\. item`) so Markdown parsers do not invent a list that was not in the document. |

## Word (`.doc`, `.docx`, `.docm`)

**Kept:** body text; headings and styles that map to structure; bold/italic/strikethrough; lists with source numbering; tables (including merges and header-row flags); hyperlinks; footnotes and endnotes; docx text-box content (`w:txbxContent`); chart / SmartArt **text** when those parts resolve; embedded pictures and OLE objects as assets (Markdown: alt text).

**Not in the document model / not read:**

| Feature | Notes |
| --- | --- |
| Page headers and footers | docx reads `word/document.xml` (plus notes parts), not header/footer parts. Binary `.doc` builds only the main story (`ccpText`); header/footer story ranges are not assembled. |
| Comments / annotations | Not extracted. |
| Tracked changes | Not modeled; visible result text may appear depending on what the file stores in the main story. |
| Form controls as forms | Complex field **results** and structured-document-tag **content** are walked as ordinary text. There is no form-field / checkbox / dropdown model. |
| Layout chrome | Page numbers, watermark drawings, and similar page furniture are out of scope for Markdown. |
| Equations | Office Math (OMML) is not converted on current `main`. |

Binary `.doc` also skips comment and macro story ranges; footnotes and endnotes are included when present.

## PowerPoint (`.ppt`, `.pptx`, and related)

**Kept:** slide text in document order; title placeholders as headings where recognized; tables; hyperlinks (pptx); speaker notes as a block quote after the slide (fixed policy); pictures as assets (alt text in Markdown); chart / diagram **text** when parts resolve. pptx applies the slide -> layout -> master text cascade for placeholder styling.

**Dropped or reduced:**

| Feature | Notes |
| --- | --- |
| Header / footer / date / slide-number chrome | pptx skips `hdr`, `ftr`, `dt`, and `sldNum` placeholders (and the slide-image placeholder on notes). ODP skips presentation classes `header`, `footer`, `date-time`, and `page-number`. |
| Notes master / template chrome | Excluded; only notes that belong to a slide are kept. |
| Animations, transitions, timing | Not represented. |
| Slide geometry / z-order | Reading order follows the shape tree / record stream, not a visual layout engine. |
| Legacy `.ppt` hyperlinks | Hyperlink records may be present but targets are not resolved into link inlines. |
| Slide boundaries | Slides are concatenated into one document. Untitled slides can read as a continuation of the previous slide unless a title heading separates them. Tracking: [#31](https://github.com/firecrawl/anydoc/issues/31). |

## Excel / spreadsheets (`.xls`, `.xlsx`, `.xlsm`, `.xlsb`)

Parsed through calamine into one Markdown table per sheet (sheet name as a heading when the workbook has more than one sheet).

| Feature | Notes |
| --- | --- |
| Cell values | Cached / calculated values as calamine exposes them. Formula expressions themselves are not emitted. |
| Number formats | Display formats (percent, currency, thousands separators, and similar) are not applied; numeric cells render as raw values (for example `0.075` instead of `7.5%`). Tracking: [#27](https://github.com/firecrawl/anydoc/issues/27). |
| Hidden rows and columns | Treated like visible cells; no visibility flag on the model. Tracking: [#9](https://github.com/firecrawl/anydoc/issues/9). |
| Hidden / very-hidden sheets | Included like visible sheets on current `main`. |
| Charts, images, comments, pivots, VBA | Not extracted. |
| Merged cells | Supported when the container exposes merge regions; regions are intersected with the used range. |
| Empty trailing area | Only the used range is materialized. |

## OpenDocument (`.odt`, `.ods`, `.odp`)

Shares the ODF frontend (`content.xml`, optional `styles.xml`).

**Kept:** text structure; lists; tables; footnotes/endnotes (odt); images and text boxes in `draw:frame`; odp speaker notes as a block quote; ods cell display text, with typed `office:value*` fallbacks when a cell has no text.

**Dropped or reduced:**

| Feature | Notes |
| --- | --- |
| Master-page headers / footers | Not read from styles; odp page chrome classes (`header`, `footer`, `date-time`, `page-number`) are skipped. |
| Annotations / tracked changes | Skipped in the text walker. |
| Soft page breaks | Ignored. |
| Charts / drawing objects without text or image | Not turned into structure beyond resolvable images and text boxes. |
| Hidden sheet / row visibility (ods) | No visibility filter equivalent to Excel's sheet state; content in `content.xml` is emitted. |
| Formulas (ods) | Formula strings are not emitted; cell text or typed value attributes are. |

## Rich Text Format (`.rtf`)

**Kept:** body text and styling that maps to the model; lists from list tables; tables; fields (result text; hyperlink fields become links); pictures in supported `\pict` formats as assets; footnotes/endnotes destinations that are not suppressed.

**Excluded by fixed policy / destination rules:** page headers and footers (`\header`, `\footer`, and first/left/right variants); annotations; embedded `\object` / `\objdata`; font/color/style/info tables as document content; several binary and metadata destinations. Unsupported picture formats are skipped with a log.

## EPUB (`.epub`)

Chapters follow the spine; each chapter body is walked with the shared HTML converter and chapter CSS (`display: none`, basic emphasis properties).

| Feature | Notes |
| --- | --- |
| Script / style / head / template / noscript | Not emitted as content. |
| Images | Package images become assets (alt text in Markdown); remote `src` URLs become Markdown images when retained as external. |
| SVG, audio, video, canvas, iframe | No dedicated extractors; they do not become first-class media blocks. |
| Navigation documents (NAV / NCX) | Not used as a separate outline source on current `main` (spine XHTML is the content). |
| DRM-encrypted publications | Fail as encrypted / unreadable packages. |

Unusable spine chapters are skipped with a log when others remain readable.

## CSV (`.csv`)

Delimiter-separated values into a single table. Encoding from BOM, then UTF-8, then Windows-1252. Delimiter sniffed among common candidates. Header row inferred from column shape when the format does not declare one.

No types beyond text, no formulas, and no separate "sheet" concept. Signature-less: name the format (or use a `.csv` path) when detection cannot work.

## PDF (`.pdf`)

PDF does **not** use the shared document model.

| Area | Behavior |
| --- | --- |
| `to_markdown` / `to_markdown_bytes` | Text extraction through [pdf-inspector](https://github.com/firecrawl/pdf-inspector), which emits Markdown directly. |
| `to_document` | Unsupported. Error directs callers to the Markdown APIs. Tracking: [#62](https://github.com/firecrawl/anydoc/issues/62). |
| Scanned / image-only PDFs | No OCR in this crate; conversion errors as `Unsupported` when there is no extractable text. Hosted [Firecrawl Parse](https://firecrawl.dev/parse) covers OCR for those cases. |
| Mixed PDFs | Pages flagged as needing OCR are omitted (warned in logs); remaining extractable text is still returned when non-empty. |
| Embedded images / assets | Not available on a document model (there is no PDF `Document`). Open work explores returning image files alongside Markdown; that is not the default API on current `main`. |
| Per-page API | Not exposed; output is one Markdown blob. See [#62](https://github.com/firecrawl/anydoc/issues/62). |

## Related issues

| Issue | Topic |
| --- | --- |
| [#2](https://github.com/firecrawl/anydoc/issues/2) | This support listing |
| [#41](https://github.com/firecrawl/anydoc/issues/41) | Literal `1.` escaping in plain paragraphs (expected) |
| [#9](https://github.com/firecrawl/anydoc/issues/9) | Hidden spreadsheet rows / columns |
| [#14](https://github.com/firecrawl/anydoc/issues/14) | Nested table flattening |
| [#27](https://github.com/firecrawl/anydoc/issues/27) | Spreadsheet number formats |
| [#31](https://github.com/firecrawl/anydoc/issues/31) | Presentation slide boundaries |
| [#62](https://github.com/firecrawl/anydoc/issues/62) | PDF `to_document` / per-page extraction |
| [#63](https://github.com/firecrawl/anydoc/issues/63) | Embedded images in Markdown output |

When behavior changes, update this file alongside the parser or renderer change.
