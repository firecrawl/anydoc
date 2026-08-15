// Convert a document to Markdown:
// `node examples/convert.mjs <file> [-f csv] [-o out.md] [--assets dir]`
//
// Run `npm install && npm run build` in `node/` first, or import
// '@firecrawl/anydoc' instead of the local build.
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { basename, extname, join } from 'node:path';
import { argv, exit, stdout } from 'node:process';

import {
  formatFromBytes,
  formatFromPath,
  toDocument,
  toMarkdownBytes,
} from '../node/index.js';

const USAGE = 'usage: convert <file> [-f csv] [-o out.md] [--assets dir]';

const millis = (ms) => (ms < 10 ? `${ms.toFixed(2)}ms` : `${ms.toFixed(0)}ms`);

const args = argv.slice(2);
let input;
let output;
let format;
let assets;
for (let i = 0; i < args.length; i++) {
  switch (args[i]) {
    case '-o':
    case '--output':
      output = args[++i];
      break;
    case '-f':
    case '--format':
      format = args[++i];
      break;
    case '--assets':
      assets = args[++i];
      break;
    default:
      input = args[i];
  }
}
if (!input) {
  console.error(USAGE);
  exit(1);
}

try {
  const bytes = await readFile(input);
  // Without -f the format comes from the file content, with the extension as
  // the fallback.
  format ??= formatFromBytes(bytes) ?? formatFromPath(input);
  if (!format) {
    throw new Error(`unrecognized file content and extension: ${input}`);
  }

  const start = performance.now();
  const markdown = await toMarkdownBytes(bytes, format);
  const elapsed = performance.now() - start;
  console.error(`converted ${input} in ${millis(elapsed)}`);

  if (output) {
    await writeFile(output, markdown);
  } else {
    stdout.write(markdown);
  }

  // Images and embedded objects live on the document model, not in the
  // Markdown, so they need a second pass to write out.
  if (assets) {
    const document = await toDocument(bytes, format);
    await mkdir(assets, { recursive: true });
    const stem = basename(input, extname(input));
    for (const asset of document.assets) {
      const [kind, subtype] = asset.mediaType.split('/');
      const extension =
        kind === 'image' ? subtype.replace(/[^a-z0-9]/gi, '') : 'bin';
      await writeFile(
        join(assets, `${stem}-${asset.id}.${extension}`),
        asset.data,
      );
    }
    console.error(`wrote ${document.assets.length} assets to ${assets}`);
  }
} catch (error) {
  console.error(`error: ${error.message}`);
  exit(1);
}
