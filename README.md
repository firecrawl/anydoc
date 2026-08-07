# anydoc

[![Crates.io](https://img.shields.io/crates/v/anydoc.svg)](https://crates.io/crates/anydoc)
[![npm](https://img.shields.io/npm/v/@firecrawl/anydoc.svg)](https://www.npmjs.com/package/@firecrawl/anydoc)
[![PyPI](https://img.shields.io/pypi/v/firecrawl-anydoc.svg)](https://pypi.org/project/firecrawl-anydoc/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![skills.sh](https://skills.sh/b/firecrawl/anydoc)](https://skills.sh/firecrawl/anydoc)

Fast Rust library that converts documents (Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, CSV, and PDF) into clean GitHub-Flavored Markdown. Includes bindings for [Node.js](node/README.md), [Python](python/README.md), and the [browser](wasm/README.md) (WebAssembly).

Built by [Firecrawl](https://firecrawl.dev) to turn any office document into LLM-ready Markdown in single-digit milliseconds, with one consistent output no matter which format goes in. It powers [Firecrawl Parse](https://firecrawl.dev/parse), so if you'd rather not run it yourself, the hosted API gives you the same conversion with the OCR models managed for you — anydoc reads scanned pages too, with [local OCR](#local-ocr-optional) you enable and supply models for.

**[Try it in your browser](https://firecrawl.github.io/anydoc/)**: the demo page runs the library as WebAssembly, so files are converted locally and never leave your machine.

## Quick start

### Agent skill

anydoc ships as an [Agent Skill](https://agentskills.io), so your agent can read any document it runs into:

```bash
npx skills add firecrawl/anydoc
```

The [skill](skills/convert-documents-to-markdown/SKILL.md) teaches the agent to convert documents with the anydoc CLI. Works with [Claude Code](https://claude.ai/code), [Codex](https://openai.com/codex/), [Cursor](https://cursor.com), [OpenCode](https://opencode.ai), and any other [compatible agent](https://agentskills.io/clients).

### CLI

```bash
npx @firecrawl/anydoc report.docx               # Markdown to stdout
npx @firecrawl/anydoc slides.pptx -o slides.md  # or to a file
npx @firecrawl/anydoc - --format csv < data.csv # read stdin
```

`npx` downloads the prebuilt binary for your platform on first run. For a permanent `anydoc` command, install globally with `npm install -g @firecrawl/anydoc`. Run `anydoc --help` for all options.

### Node.js

```bash
npm install @firecrawl/anydoc
```

```js
import { toDocument, toMarkdown, toMarkdownBytes } from '@firecrawl/anydoc';

// From a file path:
const markdown = await toMarkdown('report.docx');

// From bytes, with the format detected from the content:
const fromBytes = await toMarkdownBytes(bytes);

// Or name it, which signature-less formats (CSV) need:
const fromCsv = await toMarkdownBytes(bytes, 'csv');

// Or stop at the document model, which also carries embedded assets:
const document = await toDocument(bytes);
```

> Full API reference: [node/README.md](node/README.md)

### Python

```bash
pip install firecrawl-anydoc
```

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

> Full API reference: [python/README.md](python/README.md)

### Browser (WebAssembly)

```bash
npm install @firecrawl/anydoc-wasm
```

```js
import init, { toMarkdownBytes, toDocument } from '@firecrawl/anydoc-wasm';

await init();

// From bytes, with the format detected from the content:
const markdown = toMarkdownBytes(bytes);

// Or name it, which signature-less formats (CSV) need:
const fromCsv = toMarkdownBytes(bytes, 'csv');

// Or stop at the document model, which also carries embedded assets:
const document = toDocument(bytes);
```

> Full API reference: [wasm/README.md](wasm/README.md)

### Rust

```bash
cargo add anydoc
```

```rust
// From a file path:
let markdown = anydoc::to_markdown("report.docx")?;

// From bytes, with the format detected from the content:
let markdown = anydoc::to_markdown_bytes(&bytes, None)?;

// Or name it, which signature-less formats (CSV) need:
let markdown = anydoc::to_markdown_bytes(&bytes, anydoc::Format::Csv)?;

// Or stop at the document model, which also carries embedded assets:
let document = anydoc::to_document(&bytes, None)?;
```

## Features

- **One output for every format.** Each format parses into a shared document model and renders through a single Markdown serializer, so escaping, tables, heading anchors, and footnotes behave identically whether the input was a `.doc` from 2003 or a `.pptx` from yesterday.
- **Full document structure.** Headings with anchors, bold/italic/strikethrough, inline code and code blocks, links and internal cross-references, bulleted/numbered/nested/task lists with the source's own numbering, tables with merged cells and header rows, block quotes, footnotes and endnotes, and speaker notes.
- **Embedded assets.** Images and embedded objects render as their alt text in the Markdown, and the raw bytes stay available on the document model, tagged with their media type. Images with an external URL become ordinary Markdown images.
- **Content-based format detection.** The format is read from the bytes themselves (PDF header, RTF open group, OLE stream names, ZIP package mimetype), so mislabeled files still convert correctly.
- **Fast.** Pure Rust, no ML models, no external services. Median conversion time is under 5ms per document.
- **Bindings that stay out of the way.** Node.js conversion runs on the libuv thread pool and never blocks the event loop; Python releases the GIL so other threads keep running. TypeScript types and Python stubs ship with the packages.
- **PDF support built in.** Text-based PDFs convert locally through [pdf-inspector](https://github.com/firecrawl/pdf-inspector), no OCR service required. Scanned and mixed PDFs, and image documents, can be read with [optional local OCR](#local-ocr-optional) when you supply the models.
- **Agent ready.** Ships as an [Agent Skill](#agent-skill): one `npx skills add firecrawl/anydoc` and any agent can read office documents.

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
| Images           | `.png`, `.jpg`, `.jpeg`, `.webp`, `.tif`, `.tiff`, `.bmp`  |

Text-based PDF pages convert locally with no OCR. Pages that carry no extractable text, and image documents, need [local OCR](#local-ocr-optional): without it they report as unsupported.

## Local OCR (optional)

anydoc extracts text; it does not read pixels. A scanned PDF page or an image document has no text to extract, so reading one needs OCR. That runs locally, behind the `ocr` cargo feature, and stays off unless you ask for it: the default build is pure Rust with no models and no network, exactly as before.

The models are yours to supply — the library never downloads anything. Parse them once into a reusable converter and every conversion made with it shares that engine.

```rust
// Rust: cargo add anydoc --features ocr
let converter = anydoc::Converter::builder()
    .with_ocr_models(std::fs::read("text-detection.rten")?, std::fs::read("text-recognition.rten")?)?
    .build();
let markdown = converter.to_markdown_bytes(&std::fs::read("scan.pdf")?, anydoc::Format::Pdf)?;
```

```js
// Node.js
const converter = await Converter.create({ detectionModel, recognitionModel })
const markdown = await converter.toMarkdownBytes(await readFile('scan.pdf'))
```

```js
// Browser: build the wasm package with --features ocr, then run it in a Worker
const converter = new Converter({ detectionModel, recognitionModel })
const markdown = converter.toMarkdownBytes(pdfBytes, 'pdf')
```

### Models

Two files from the [ocrs](https://github.com/robertknight/ocrs) project, in RTen format, published at `https://ocrs-models.s3-accelerate.amazonaws.com/`:

| File                     | SHA-256                                                            |
| ------------------------ | ------------------------------------------------------------------ |
| `text-detection.rten`    | `f15cfb56bd02c4bf478a20343986504a1f01e1665c2b3a0ad66340f054b1b5ca` |
| `text-recognition.rten`  | `e484866d4cce403175bd8d00b128feb08ab42e208de30e42cd9889d8f1735a6e` |

The models are licensed CC-BY-SA-4.0, separately from anydoc's MIT license, so check that it suits your use before shipping them. Verify the digests after downloading: anydoc parses whatever bytes you hand it.

The code the feature pulls in is permissively licensed throughout: `ocrs` and `rten` are MIT OR Apache-2.0, `image` is MIT OR Apache-2.0, and the PDF renderer behind it (`hayro`, `vello_cpu`) is Apache-2.0 OR MIT with `bytemuck` adding Zlib to that choice. Nothing in the graph is copyleft; the models are the only CC-BY-SA-4.0 component.

### What to expect

Local OCR is an early preview, and it is worth knowing its limits before relying on it:

- The current models read **Latin scripts only**.
- Output is **plain text**: no headings, tables, or lists are inferred from a recognized page, only the lines as read.
- Accuracy varies with scan quality. **Overlapping text garbles lines** — a watermark across body text can merge into one unreadable line.
- An image with no text can still produce **short spurious fragments**.
- **EXIF orientation is not applied**: a photo stored rotated is read as stored.
- A scanned document with no extractable text at all has **every page** rendered and recognized, not just some: there is no per-page text to tell the pages apart.
- Recognition is **CPU-bound** and costs roughly a tenth of a second per page — a one-page scan converts in about 155 ms after a ~20 ms engine startup, against ~5 ms for a text document. In a browser, run it in a Web Worker or the page freezes while it works.
- Building with `ocr` needs **Rust 1.92**; the crate without it keeps its 1.88 minimum.

For hosted OCR with no models to manage, [Firecrawl Parse](https://firecrawl.dev) runs the same conversions as a service.

## Benchmark

anydoc is measured against six other converters on 100 real-world documents spanning fourteen formats. Scores run from 0 to 100, higher is better; speed is the median time to convert one document.

| tool         | formats   | median ms | docs judged | score  | completeness | structure | formatting | cleanliness |
| ------------ | --------- | --------- | ----------- | ------ | ------------ | --------- | ---------- | ----------- |
| anydoc       | **14/14** | **4.4**   | 94          | **81** | **87**       | **79**    | **78**     | **81**      |
| libreoffice  | 12/14     | 1129.5    | 87          | 40     | 59           | 42        | 40         | 24          |
| unstructured | 8/14      | 572.9     | 58          | 63     | 76           | 59        | 51         | 63          |
| markitdown   | 6/14      | 134.8     | 33          | 65     | 78           | 66        | 60         | 52          |
| pandoc       | 5/14      | 102.1     | 34          | 56     | 74           | 57        | 56         | 38          |
| docling      | 4/14      | 513.6     | 21          | 57     | 60           | 60        | 57         | 51          |
| mammoth      | 1/14      | 52.5      | 8           | 70     | 84           | 71        | 75         | 51          |

Per format, like for like:

| format | anydoc | libreoffice | unstructured | markitdown | pandoc | docling | mammoth |
| ------ | ------ | ----------- | ------------ | ---------- | ------ | ------- | ------- |
| doc    | **87** | 57          | 67           | -          | -      | -       | -       |
| docm   | **84** | 48          | -            | -          | -      | -       | -       |
| docx   | **88** | 56          | 53           | 71         | 68     | 71      | 70      |
| epub   | **77** | -           | 72           | 72         | 52     | -       | -       |
| odp    | **86** | 23          | -            | -          | -      | -       | -       |
| ods    | **82** | 38          | -            | -          | -      | -       | -       |
| odt    | **80** | 51          | 68           | -          | 60     | -       | -       |
| ppt    | **80** | 26          | -            | -          | -      | -       | -       |
| pptx   | **74** | 24          | -            | 66         | -      | 52      | -       |
| rtf    | **88** | 53          | 46           | -          | 45     | -       | -       |
| xls    | **80** | 38          | 66           | 62         | -      | -       | -       |
| xlsm   | **76** | 32          | -            | -          | -      | -       | -       |
| xlsx   | **72** | 30          | 66           | 55         | -      | 47      | -       |

**How quality was scored:** an LLM judge (Claude Sonnet 5) compares two tools' outputs blind against ground truth: the document's first six pages, rendered to images by LibreOffice. Each output is scored on completeness, structure, formatting, and cleanliness. Every pair is judged twice with the outputs swapped to cancel position bias, for 482 verdicts in total. Each tool's `score` averages its per-format scores over the formats it supports, so a corpus heavy in one format can't skew it. It also means each row averages a different set of formats (mammoth's 69 is docx alone, while anydoc's 81 spans all fourteen), so the per-format table is the fair comparison.

Speed is one warm conversion per document on a Ryzen 9 9950X3D (Windows 11, 64 GB DDR5-6400). anydoc and the Python libraries are timed with process spawn excluded; the CLI tools include it, since that is how they are used. The harness lives in [`bench/`](bench/README.md); the corpus is not redistributable and is not in the repo.

**Best fit:** pipelines that receive a mixed bag of office documents and need one consistent, structured Markdown output. In this comparison, anydoc was the only tool to cover all fourteen formats, scored highest on every judged format, and converted documents an order of magnitude faster than the next-fastest tool.

## Format detection

The format is read from the file content, using the marker its specification designates: the PDF header, the RTF open group, OLE stream names, the ZIP package mimetype and content types. CSV has no such marker, so the extension or an explicit format names it instead.

```rust
Format::from_bytes(&bytes); // Some(Format::Docx), or None when nothing matches
Format::from_extension("pptm"); // Some(Format::Pptx)
Format::from_path(Path::new("report.odt")); // Some(Format::Odt)
```

The same three functions exist in Node (`formatFromBytes`, ...) and Python (`anydoc.format_from_bytes`, ...).

## Errors

A conversion returns `Err` only when no meaningful Markdown could come out of the file. `ConvertError` names what went wrong:

```rust
match anydoc::to_markdown(path) {
    Ok(markdown) => Some(markdown),
    // No document comes out of these, so record the file and take the next one.
    Err(error @ (ConvertError::Encrypted | ConvertError::Unsupported(_))) => {
        unconverted.push((path, error));
        None
    }
    Err(error) => return Err(error),
}
```

| Variant         | Meaning                                                             |
| --------------- | ------------------------------------------------------------------- |
| `Unsupported`   | Unknown format, or one that cannot be converted (an image-only PDF) |
| `Malformed`     | Structurally unusable: no meaningful content could be extracted     |
| `Encrypted`     | Encrypted or password-protected                                     |
| `ResourceLimit` | Crossed a fixed safety limit (decompression, nesting, node count)   |
| `MissingPart`   | A part required for any meaningful output is absent                 |
| `Ocr`           | OCR ran on every page carrying content and read no text             |
| `Io`            | The file could not be read, from `to_markdown` only                 |

Node and wasm publish the variant name on `error.code`; Python raises one `anydoc.ConvertError` subclass per named variant (a variant added later, like the OCR ones, raises the base class until it gets a name), or `OSError` when the file cannot be read.

## How it works

```
document bytes
  │
  ├─► format detection      → content markers, not the extension
  │
  ├─► format parser          → one per format (doc, docx, ppt, pptx, xls,
  │                            xlsx, odt/ods/odp, rtf, epub, csv)
  │         │
  │         └─► Document     → shared model: blocks, inlines, tables,
  │                            footnotes, assets
  │               │
  │               └─► GFM serializer → Markdown
  │
  └─► PDF → pdf-inspector    → Markdown directly
```

Because every format funnels through the same document model and serializer, output quirks get fixed once. A table-escaping fix for docx is automatically a table-escaping fix for rtf, odt, and everything else.

## Development

```bash
cargo test
cd node && npm install && npm run build && npm test
cd python && pip install maturin && maturin develop && python -m unittest discover -s tests
wasm-pack build wasm --release --target web --scope firecrawl && node --test wasm/test.mjs  # see wasm/README.md
```

A committed fixture corpus under `tests/fixtures/` is snapshot-tested, `tests/robustness.rs` mutation-tests every fixture, and `fuzz/` carries cargo-fuzz targets per format. The speed and quality benchmark lives in [`bench/`](bench/README.md).

Releases are tagged `v<version>`, which publishes the crate, the npm package, and the PyPI wheels from [`.github/workflows/release.yml`](.github/workflows/release.yml). The version lives in three places, bumped together for a release:

- [`Cargo.toml`](Cargo.toml): the crate
- [`node/package.json`](node/package.json): the npm package
- [`python/Cargo.toml`](python/Cargo.toml): the wheel (`python/pyproject.toml` reads it)

## License

[MIT](LICENSE)
