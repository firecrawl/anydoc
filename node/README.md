# @firecrawl/anydoc

[![npm](https://img.shields.io/npm/v/@firecrawl/anydoc.svg)](https://www.npmjs.com/package/@firecrawl/anydoc)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/firecrawl/anydoc/blob/main/LICENSE)

Convert Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, CSV, and PDF files into clean GitHub-Flavored Markdown. Node.js bindings for the [anydoc](https://github.com/firecrawl/anydoc) Rust crate, built by [Firecrawl](https://firecrawl.dev). Also available as a hosted API through [Firecrawl Parse](https://firecrawl.dev/parse), which adds our OCR models for the scanned pages anydoc can't read on its own.

Every format parses into one shared document model and renders through a single Markdown serializer, so headings, tables, lists, and footnotes come out the same no matter which format goes in. Conversion runs on the libuv thread pool and never blocks the event loop. TypeScript types ship with the package.

```bash
npm install @firecrawl/anydoc
```

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

## CLI

The package ships an `anydoc` command, so `npx` converts a document with no install:

```bash
npx @firecrawl/anydoc report.docx               # Markdown to stdout
npx @firecrawl/anydoc slides.pptx -o slides.md  # or to a file
npx @firecrawl/anydoc - --format csv < data.csv # read stdin
```

Markdown goes to stdout, errors to stderr, and `anydoc --help` covers the rest.

## Usage

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

// After uploading those assets, render their URLs at the original positions:
const assetUrls = Object.fromEntries(await Promise.all(
  document.assets.map(async (asset) => [
    asset.id,
    await upload(asset.data, asset.mediaType),
  ])
));
const withImages = await toMarkdownBytes(bytes, null, { assetUrls });
```

## Format detection

The format is read from the file content, using the marker its specification designates: the PDF header, the RTF open group, OLE stream names, the ZIP package mimetype and content types. CSV has no such marker, so detection returns `null` for it and the extension, or an explicit format, names it instead.

```js
formatFromBytes(bytes); // 'docx', or null when nothing matches
formatFromExtension('.pptm'); // 'pptx'
formatFromPath('report.odt'); // 'odt'
```

## Images and embedded objects

Markdown cannot embed bytes, so an embedded image renders as its alt text by default while the bytes stay on `document.assets`, tagged with a media type and the part they came from. After storing those bytes and obtaining URLs, pass `{ assetUrls: { [asset.id]: url } }` as the third argument to `toMarkdownBytes`; resolved images become ordinary Markdown images at their original document positions. Missing mappings keep the default alt-text behavior.

Full behavior notes and benchmarks live in the [repository README](https://github.com/firecrawl/anydoc#readme).

## License

[MIT](https://github.com/firecrawl/anydoc/blob/main/LICENSE)
