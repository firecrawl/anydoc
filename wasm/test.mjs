// Smoke test: the wasm bindings load in Node and every entry point
// round-trips a fixture. Build first: wasm-pack build wasm --release --target web
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { test } from 'node:test'

import * as anydoc from './pkg/anydoc_wasm.js'

const {
  initSync,
  formatFromBytes,
  formatFromExtension,
  formatFromPath,
  toDocument,
  toMarkdownBytes,
} = anydoc

const fixture = (name) => fileURLToPath(new URL(`../tests/fixtures/${name}`, import.meta.url))

initSync({ module: await readFile(fileURLToPath(new URL('./pkg/anydoc_wasm_bg.wasm', import.meta.url))) })

const OUTLINE = await readFile(fixture('docx/handmade-outline.docx'))
const RICH = await readFile(fixture('docx/handmade-rich.docx'))
const CSV = await readFile(fixture('csv/sheet.csv'))
const PDF = await readFile(fixture('pdf/text.pdf'))
const ENCRYPTED = await readFile(fixture('malformed/encrypted--errors.odt'))

test('toMarkdownBytes converts in memory', () => {
  const markdown = toMarkdownBytes(RICH, 'docx')
  assert.match(markdown, /\| Quarter \| Widgets \|/)
})

test('toMarkdownBytes detects the format when none is named', () => {
  assert.match(toMarkdownBytes(RICH), /\| Quarter \| Widgets \|/)
  // CSV carries no signature, so it has to be named.
  assert.throws(() => toMarkdownBytes(CSV), /unrecognized file content/)
  assert.match(toMarkdownBytes(CSV, 'csv'), /\| --- \|/)
})

test('pdf converts to Markdown but has no document model', () => {
  assert.ok(toMarkdownBytes(PDF).length > 0)
  assert.throws(() => toDocument(PDF), /pdf/i)
})

test('toDocument exposes the document model', () => {
  const document = toDocument(OUTLINE, 'docx')
  const heading = document.blocks.find((block) => block.kind === 'heading')
  assert.ok(heading.level >= 1 && heading.level <= 6)
  assert.equal(typeof heading.content[0].text, 'string')
  assert.equal(heading.content[0].kind, 'text')
  assert.equal(typeof heading.content[0].style.bold, 'boolean')
})

test('toDocument carries embedded assets as Uint8Arrays', () => {
  const document = toDocument(RICH, 'docx')
  const image = document.assets.find((asset) => asset.mediaType === 'image/png')
  assert.ok(image.data instanceof Uint8Array)
  assert.ok(image.data.length > 0)
  assert.equal(image.id, document.assets.indexOf(image))
})

test('format detection reads content, extension, and path', () => {
  assert.equal(formatFromBytes(RICH), 'docx')
  // CSV carries no signature: only the extension names it.
  assert.equal(formatFromBytes(CSV), undefined)
  assert.equal(formatFromExtension('.pptm'), 'pptx')
  assert.equal(formatFromExtension('xls'), 'xlsx')
  assert.equal(formatFromPath('/tmp/report.odt'), 'odt')
  assert.equal(formatFromPath('/tmp/report.unknown'), undefined)
})

// `code` is what callers branch on, so every kind of failure is pinned here.
test('conversion errors throw a coded Error', () => {
  const throws = (call, code, message) =>
    assert.throws(call, (error) => {
      assert.ok(error instanceof Error)
      assert.equal(error.code, code)
      assert.match(error.message, message)
      return true
    })

  throws(() => toMarkdownBytes(new TextEncoder().encode('not a document'), 'docx'), 'malformed', /malformed/)
  throws(() => toMarkdownBytes(CSV), 'unsupported', /unrecognized file content/)
  throws(() => toMarkdownBytes(ENCRYPTED, 'odt'), 'encrypted', /encrypted/)
  throws(() => toDocument(ENCRYPTED, 'odt'), 'encrypted', /encrypted/)
})

// The Converter exists only in a build with the `ocr` feature, so the same
// suite covers both. Which build this is has to come from the caller:
// ANYDOC_WASM_OCR_BUILD=1 for an OCR build, 0 for a plain one. A test that
// read it off the module could not tell a broken feature gate from a build
// that never had the feature.
const { Converter } = anydoc
const OCR_BUILD = process.env.ANYDOC_WASM_OCR_BUILD

test('the OCR converter is present exactly when the build says it is', {
  skip: OCR_BUILD === undefined && 'set ANYDOC_WASM_OCR_BUILD=1 or 0 to assert the feature gate',
}, () => {
  assert.ok(['0', '1'].includes(OCR_BUILD), `ANYDOC_WASM_OCR_BUILD has to be 1 or 0, got ${OCR_BUILD}`)

  if (OCR_BUILD === '1') {
    assert.equal(typeof Converter, 'function', 'an OCR build has to export Converter')
  } else {
    assert.equal(Converter, undefined, 'a build without --features ocr may not export Converter')
  }
})

test('the converter rejects model bytes it cannot use', { skip: !Converter && 'built without --features ocr' }, () => {
  const throws = (models, message) =>
    assert.throws(() => new Converter(models), (error) => {
      assert.ok(error instanceof Error)
      assert.equal(error.code, 'ocrInit')
      assert.match(error.message, message)
      return true
    })

  throws({ detectionModel: new Uint8Array([0, 1, 2, 3]), recognitionModel: new Uint8Array([4, 5]) }, /detection model/)
  throws({ recognitionModel: new Uint8Array([4, 5]) }, /detectionModel is required/)
  throws({ detectionModel: 'not bytes', recognitionModel: new Uint8Array([4, 5]) }, /detectionModel has to be a Uint8Array/)
})
