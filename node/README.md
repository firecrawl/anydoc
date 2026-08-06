# @firecrawl/anydoc

[![npm](https://img.shields.io/npm/v/@firecrawl/anydoc.svg)](https://www.npmjs.com/package/@firecrawl/anydoc)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/firecrawl/anydoc/blob/main/LICENSE)

Convert Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, CSV, and PDF files into clean GitHub-Flavored Markdown. Node.js bindings for the [anydoc](https://github.com/firecrawl/anydoc) Rust crate, built by [Firecrawl](https://firecrawl.dev). Scanned pages and image documents are read with [local OCR](#ocr-converter) when you supply the models. Also available as a hosted API through [Firecrawl Parse](https://firecrawl.dev/parse), which manages those models for you.

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
```

## OCR converter

A scanned PDF page or an image document carries no text to extract, so reading one needs OCR. Supply the two [ocrs](https://github.com/robertknight/ocrs) models as bytes: they are parsed once, and every conversion made with that converter reuses the engine. Nothing is downloaded for you, and model parsing and conversion both run off the event loop.

```js
import { readFile } from 'node:fs/promises';
import { Converter } from '@firecrawl/anydoc';

const converter = await Converter.create({
  detectionModel: await readFile('text-detection.rten'),
  recognitionModel: await readFile('text-recognition.rten'),
});

const markdown = await converter.toMarkdownBytes(await readFile('scan.pdf'));
```

Models that cannot be loaded reject with `code` `'ocrInit'`. The module-level functions never use OCR: through them a scanned PDF stays `'unsupported'`. See the [root README](../README.md#local-ocr-optional) for model digests, licensing, and what OCR can and cannot do.

OCR is compiled into the published binary, so it carries the recognition and rendering code whether or not you build a `Converter`: about 4.75 MB on top of a 5.6 MB native library, roughly doubling it.

## Errors

A conversion rejects only when no meaningful Markdown could come out of the file. The rejection is an `Error` whose `code` names what went wrong:

```js
try {
  return await toMarkdown(path);
} catch (error) {
  // No document comes out of these, so record the file and take the next one.
  if (error.code === 'encrypted' || error.code === 'unsupported') {
    unconverted.push({ path, reason: error.code });
    return null;
  }
  throw error;
}
```

| `code`          | Meaning                                                             |
| --------------- | ------------------------------------------------------------------- |
| `unsupported`   | Unknown format, or one that cannot be converted (an image-only PDF) |
| `malformed`     | Structurally unusable: no meaningful content could be extracted     |
| `encrypted`     | Encrypted or password-protected                                     |
| `resourceLimit` | Crossed a fixed safety limit (decompression, nesting, node count)   |
| `missingPart`   | A part required for any meaningful output is absent                 |
| `ocr`           | OCR ran on every page carrying content and read no text             |
| `ocrInit`       | The OCR models could not be loaded, from `Converter.create` only    |
| `io`            | The file could not be read, from `toMarkdown` only                  |

`error.message` carries the detail, naming the package part at fault where the format identifies one. TypeScript gets the union as `ConvertErrorCode`.

## Format detection

The format is read from the file content, using the marker its specification designates: the PDF header, the RTF open group, OLE stream names, the ZIP package mimetype and content types. CSV has no such marker, so detection returns `null` for it and the extension, or an explicit format, names it instead.

```js
formatFromBytes(bytes); // 'docx', or null when nothing matches
formatFromExtension('.pptm'); // 'pptx'
formatFromPath('report.odt'); // 'odt'
```

## Images and embedded objects

Markdown cannot embed bytes, so an embedded image renders as its alt text while the bytes stay on `document.assets`, tagged with a media type and the part they came from. Images that carry an external URL render as ordinary Markdown images.

Full behavior notes and benchmarks live in the [repository README](https://github.com/firecrawl/anydoc#readme).

## License

[MIT](https://github.com/firecrawl/anydoc/blob/main/LICENSE)
