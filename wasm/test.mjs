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
} from './pkg/anydoc_wasm.js'

const fixture = (name) => fileURLToPath(new URL(`../tests/fixtures/${name}`, import.meta.url))

initSync({ module: await readFile(fileURLToPath(new URL('./pkg/anydoc_wasm_bg.wasm', import.meta.url))) })

const OUTLINE = await readFile(fixture('docx/handmade-outline.docx'))
const RICH = await readFile(fixture('docx/handmade-rich.docx'))
const CSV = await readFile(fixture('csv/sheet.csv'))
const PDF = await readFile(fixture('pdf/text.pdf'))

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

test('conversion errors throw with the crate error message', () => {
  assert.throws(() => toMarkdownBytes(new TextEncoder().encode('not a document'), 'docx'), /malformed|unsupported/)
})

const caught = (run) => {
  try {
    run()
  } catch (error) {
    return error
  }
  throw new Error('expected a throw')
}

test('conversion errors name the cause in kind', () => {
  // `kind` lets a caller branch on the cause. Matching the message text is the
  // alternative, and that breaks whenever the wording changes.
  const unrecognized = caught(() => toMarkdownBytes(new TextEncoder().encode('not a document')))
  assert.equal(unrecognized.name, 'ConvertError')
  assert.equal(unrecognized.kind, 'unsupported')
  assert.equal(typeof unrecognized.message, 'string')

  const named = caught(() => toMarkdownBytes(new TextEncoder().encode('not a document'), 'docx'))
  assert.ok(['malformed', 'unsupported'].includes(named.kind))

  // Every variant maps to one of these names.
  const kinds = ['unsupported', 'malformed', 'encrypted', 'resourceLimit', 'missingPart', 'io']
  assert.ok(kinds.includes(unrecognized.kind))
  assert.ok(kinds.includes(named.kind))
})
