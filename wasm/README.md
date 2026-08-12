# @firecrawl/anydoc-wasm

WebAssembly bindings for [anydoc](../README.md), plus the source of the demo page at [firecrawl.github.io/anydoc](https://firecrawl.github.io/anydoc/).

The API mirrors the Rust library, minus the path-based `to_markdown`: wasm has no filesystem, so conversion always starts from bytes.

```bash
npm install @firecrawl/anydoc-wasm
```

```js
import init, {
  formatFromBytes,
  toMarkdownBytes,
  toDocument,
} from '@firecrawl/anydoc-wasm';

await init();

// With the format detected from the content:
const markdown = toMarkdownBytes(bytes);

// Or name it, which signature-less formats (CSV) need:
const fromCsv = toMarkdownBytes(bytes, 'csv');

// Or stop at the document model, which also carries embedded assets:
const document = toDocument(bytes);

// Format detection on its own:
formatFromBytes(bytes); // 'docx', or undefined when nothing matches
```

The package is built with `wasm-pack --target web`: it loads with a plain `<script type="module">` and with bundlers that handle the `new URL(..., import.meta.url)` asset pattern (Vite, webpack 5, Rollup). In Node, pass the module bytes to `initSync` instead of calling `init` (see [`test.mjs`](wasm/test.mjs)).

Calls are synchronous: wasm runs single-threaded on the calling thread, so convert on a worker if the main thread must stay responsive.

## Errors

A conversion throws only when no meaningful Markdown could come out of the bytes. The thrown value is an `Error` whose `code` names what went wrong:

```js
try {
  return toMarkdownBytes(bytes);
} catch (error) {
  // No document comes out of these, so record the file and take the next one.
  if (error.code === 'encrypted' || error.code === 'unsupported') {
    unconverted.push({ name, reason: error.code });
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
| `ocrInit`       | The OCR models could not be loaded, from `new Converter` only       |

`error.message` carries the detail, naming the package part at fault where the format identifies one. TypeScript gets the union as `ConvertErrorCode`. The crate's `io` code has no counterpart here: there is no filesystem to read from.

## Local OCR

Scanned PDFs and image documents carry no text to extract, so reading them needs OCR. It ships behind the `ocr` cargo feature, which is off by default because it multiplies the module size; build with `--features ocr` to get the `Converter` class.

The two RTen models are a separate download that you supply as `Uint8Array`s. They are parsed once, when the converter is constructed, and every conversion made with it reuses that engine:

```js
const [detectionModel, recognitionModel] = await Promise.all([
  fetch('/models/text-detection.rten').then((r) => r.arrayBuffer()),
  fetch('/models/text-recognition.rten').then((r) => r.arrayBuffer()),
])

const converter = new Converter({
  detectionModel: new Uint8Array(detectionModel),
  recognitionModel: new Uint8Array(recognitionModel),
})

const markdown = converter.toMarkdownBytes(new Uint8Array(pdfBytes), 'pdf')
```

A failed model load throws an `Error` with `'ocrInit'` on `code`.

Run it in a Web Worker. Recognition is CPU-bound and blocks whatever thread it runs on, so doing this on the UI thread freezes the page for as long as a document takes. Load the module and build the converter inside the worker, keep the converter alive between messages so the models are parsed once, and post only the bytes and the Markdown across.

[`examples/worker/`](examples/worker) is that setup in about 30 lines: [`worker.js`](examples/worker/worker.js) fetches the models and holds the converter, and [`index.html`](examples/worker/index.html) transfers the file bytes to it and prints the Markdown. No dependencies, no build step beyond the package itself.

## Building

```bash
wasm-pack build wasm --release --target web --scope firecrawl
ANYDOC_WASM_OCR_BUILD=0 node --test wasm/test.mjs
```

`ANYDOC_WASM_OCR_BUILD` tells the suite which build it is looking at, so the test that guards the feature gate has an expectation of its own to check. Build and test the OCR package the same way:

```bash
wasm-pack build wasm --release --target web --scope firecrawl -- --features ocr
ANYDOC_WASM_OCR_BUILD=1 node --test wasm/test.mjs
```

This produces the npm package in `wasm/pkg/`: the module, the JS glue, and TypeScript definitions. Publishing runs from [`../.github/workflows/release.yml`](../.github/workflows/release.yml) on release tags.

## Demo page

`www/` holds the static demo site, which imports the module from `www/pkg/`. Build into that directory, then serve `www/`:

```bash
wasm-pack build wasm --release --target web --no-pack --out-dir www/pkg
python -m http.server -d wasm/www
```

[`../.github/workflows/pages.yml`](../.github/workflows/pages.yml) builds and deploys the same layout to GitHub Pages on every push to main.
