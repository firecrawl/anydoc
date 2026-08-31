// Smoke test: the wasm bindings load in Node and every entry point
// round-trips a fixture. Build first: wasm-pack build wasm --release --target web
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import { test } from 'node:test'

import {
  initSync,
  formatFromBytes,
  formatFromExtension,
  formatFromPath,
  toDocument,
  toMarkdownBytes,
  toMarkdownBytesWith,
} from './pkg/anydoc_wasm.js'

const fixture = (name) => fileURLToPath(new URL(`../tests/fixtures/${name}`, import.meta.url))

initSync({ module: await readFile(fileURLToPath(new URL('./pkg/anydoc_wasm_bg.wasm', import.meta.url))) })

const OUTLINE = await readFile(fixture('docx/handmade-outline.docx'))
const RICH = await readFile(fixture('docx/handmade-rich.docx'))
const CSV = await readFile(fixture('csv/sheet.csv'))
const PDF = await readFile(fixture('pdf/text.pdf'))
const ENCRYPTED = await readFile(fixture('malformed/encrypted--errors.odt'))
const MIXED = await readFile(fixture('pdf/handmade-mixed.pdf'))
const PARTLY_SCANNED = await readFile(fixture('pdf/handmade-partly-scanned.pdf'))
const SCANNED = await readFile(fixture('pdf/handmade-scanned.pdf'))

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
  throws(() => toMarkdownBytes(MIXED), 'needsOcr', /page 2 of 2 needs OCR/)
})

test("ocr: 'skip' converts the pages that carry text and names the rest", () => {
  const { markdown, pagesNeedingOcr, pageCount } = toMarkdownBytesWith(PARTLY_SCANNED, 'pdf', 'skip')
  assert.deepEqual([pagesNeedingOcr, pageCount], [[2, 5], 5])
  assert.match(markdown, /Readable page three/)
  // The pages left out are reported, never written into the Markdown.
  assert.doesNotMatch(markdown, /OCR|not converted/i)
  assert.throws(() => toMarkdownBytesWith(SCANNED, 'pdf', 'skip'), (error) => {
    assert.equal(error.code, 'needsOcr')
    return true
  })
  // Without it, the report form is the default: rejecting is still the default.
  assert.throws(() => toMarkdownBytesWith(PARTLY_SCANNED, 'pdf'), (error) => {
    assert.equal(error.code, 'needsOcr')
    return true
  })
})

test('a pdf with scanned pages throws naming them instead of dropping them', () => {
  assert.throws(() => toMarkdownBytes(MIXED), (error) => {
    assert.deepEqual([error.pages, error.pageCount], [[2], 2])
    return true
  })
})
