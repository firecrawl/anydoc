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
  toMarkdownBytesWith,
  toDocument,
} from '@firecrawl/anydoc-wasm';

await init();

// With the format detected from the content:
const markdown = toMarkdownBytes(bytes);

// Or name it, which signature-less formats (CSV) need:
const fromCsv = toMarkdownBytes(bytes, 'csv');

// Or stop at the document model, which also carries embedded assets:
const document = toDocument(bytes);

// A PDF with scanned pages throws needsOcr; convert the rest instead:
const { markdown, pagesNeedingOcr } = toMarkdownBytesWith(bytes, 'pdf', 'skip');

// Format detection on its own:
formatFromBytes(bytes); // 'docx', or undefined when nothing matches
```

The package is built with `wasm-pack --target web`: it loads with a plain `<script type="module">` and with bundlers that handle the `new URL(..., import.meta.url)` asset pattern (Vite, webpack 5, Rollup). In Node, pass the module bytes to `initSync` instead of calling `init` (see [`test.mjs`](wasm/test.mjs)).

Calls are synchronous: wasm runs single-threaded on the calling thread, so convert on a worker if the main thread must stay responsive.

## Errors

A conversion throws only when no complete Markdown could come out of the bytes. The thrown value is an `Error` whose `code` names what went wrong:

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
| `unsupported`   | Unknown format, or one that cannot be converted                     |
| `needsOcr`      | Pages of a PDF are scanned or image-only; `pages` names them        |
| `malformed`     | Structurally unusable: no meaningful content could be extracted     |
| `encrypted`     | Encrypted or password-protected                                     |
| `resourceLimit` | Crossed a fixed safety limit (decompression, nesting, node count)   |
| `missingPart`   | A part required for any meaningful output is absent                 |

`error.message` carries the detail, naming the package part at fault where the format identifies one. TypeScript gets the union as `ConvertErrorCode`. The crate's `io` code has no counterpart here: there is no filesystem to read from.

## Building

```bash
wasm-pack build wasm --release --target web --scope firecrawl
node --test wasm/test.mjs
```

This produces the npm package in `wasm/pkg/`: the module, the JS glue, and TypeScript definitions. Publishing runs from [`../.github/workflows/release.yml`](../.github/workflows/release.yml) on release tags.

## Demo page

`www/` holds the static demo site, which imports the module from `www/pkg/`. Build into that directory, then serve `www/`:

```bash
wasm-pack build wasm --release --target web --no-pack --out-dir www/pkg
python -m http.server -d wasm/www
```

[`../.github/workflows/pages.yml`](../.github/workflows/pages.yml) builds and deploys the same layout to GitHub Pages on every push to main.
