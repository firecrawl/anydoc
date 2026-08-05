// Real-model OCR test: the browser converter reading a scanned PDF.
//
// Needs an OCR build and three paths from the environment, and skips itself
// when any is missing:
//   wasm-pack build wasm --release --target web -- --features ocr
//   ANYDOC_OCR_DETECTION_MODEL=... ANYDOC_OCR_RECOGNITION_MODEL=... \
//   ANYDOC_PDF_SAMPLE_FILES=... node --test wasm/test-ocr.mjs
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { test } from 'node:test'

import * as anydoc from './pkg/anydoc_wasm.js'

const DETECTION_MODEL = process.env.ANYDOC_OCR_DETECTION_MODEL
const RECOGNITION_MODEL = process.env.ANYDOC_OCR_RECOGNITION_MODEL
const SAMPLE_FILES = process.env.ANYDOC_PDF_SAMPLE_FILES

// The pinned scanned page: its only extractable text is a tool watermark, so
// everything asserted below has to come from recognition.
const SCAN = '018-base64-image/base64image.pdf'
const SCAN_SHA256 = 'aaad90df16fce40ec768629d2135479b98f65b39bb27c7f80fb106393187d619'

const missing = [
  ['ANYDOC_OCR_DETECTION_MODEL', DETECTION_MODEL],
  ['ANYDOC_OCR_RECOGNITION_MODEL', RECOGNITION_MODEL],
  ['ANYDOC_PDF_SAMPLE_FILES', SAMPLE_FILES],
]
  .filter(([, value]) => !value)
  .map(([name]) => name)

const skip = missing.length
  ? `set ${missing.join(', ')} to run`
  : anydoc.Converter
    ? false
    : 'built without --features ocr'

if (!skip) {
  anydoc.initSync({
    module: await readFile(fileURLToPath(new URL('./pkg/anydoc_wasm_bg.wasm', import.meta.url))),
  })
}

const normalize = (markdown) => markdown.toLowerCase().split(/\s+/).filter(Boolean).join(' ')

const build = async () =>
  new anydoc.Converter({
    detectionModel: new Uint8Array(await readFile(DETECTION_MODEL)),
    recognitionModel: new Uint8Array(await readFile(RECOGNITION_MODEL)),
  })

const scan = async () => {
  const bytes = new Uint8Array(await readFile(join(SAMPLE_FILES, SCAN)))
  const digest = createHash('sha256').update(bytes).digest('hex')
  assert.equal(digest, SCAN_SHA256, 'the pinned corpus file changed')
  return bytes
}

test('the converter recognizes a scanned page in wasm', { skip }, async () => {
  const markdown = (await build()).toMarkdownBytes(await scan(), 'pdf')

  console.log(`018 output (verbatim):\n${markdown}`)
  assert.match(normalize(markdown), /fix the issue and close it/)
})

test('one converter reads several documents', { skip }, async () => {
  const converter = await build()
  const bytes = await scan()

  assert.equal(converter.toMarkdownBytes(bytes, 'pdf'), converter.toMarkdownBytes(bytes, 'pdf'))
})
