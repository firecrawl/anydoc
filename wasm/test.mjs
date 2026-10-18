// Smoke test: the wasm bindings load in Node and every entry point
// round-trips a fixture. Build first: wasm-pack build wasm --release --target web
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { createServer } from 'node:http'
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
const ENCRYPTED = await readFile(fixture('malformed/encrypted--errors.odt'))
const MIXED = await readFile(fixture('pdf/handmade-mixed.pdf'))

test('toMarkdownBytes converts in memory', async () => {
  const markdown = await toMarkdownBytes(RICH, 'docx')
  assert.match(markdown, /\| Quarter \| Widgets \|/)
})

test('toMarkdownBytes detects the format when none is named', async () => {
  assert.match(await toMarkdownBytes(RICH), /\| Quarter \| Widgets \|/)
  // CSV carries no signature, so it has to be named.
  await assert.rejects(toMarkdownBytes(CSV), /unrecognized file content/)
  assert.match(await toMarkdownBytes(CSV, 'csv'), /\| --- \|/)
})

test('pdf converts to Markdown but has no document model', async () => {
  assert.ok((await toMarkdownBytes(PDF)).length > 0)
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
test('conversion errors carry a code', async () => {
  const coded = (code, message) => (error) => {
    assert.ok(error instanceof Error)
    assert.equal(error.code, code)
    assert.match(error.message, message)
    return true
  }

  await assert.rejects(toMarkdownBytes(new TextEncoder().encode('not a document'), 'docx'), coded('malformed', /malformed/))
  await assert.rejects(toMarkdownBytes(CSV), coded('unsupported', /unrecognized file content/))
  await assert.rejects(toMarkdownBytes(ENCRYPTED, 'odt'), coded('encrypted', /encrypted/))
  assert.throws(() => toDocument(ENCRYPTED, 'odt'), coded('encrypted', /encrypted/))
  await assert.rejects(toMarkdownBytes(MIXED), coded('needsOcr', /page 2 of 2 needs OCR/))
})

test('a pdf with scanned pages rejects naming them instead of dropping them', async () => {
  await assert.rejects(toMarkdownBytes(MIXED), (error) => {
    assert.deepEqual([error.pages, error.pageCount], [[2], 2])
    return true
  })
})

// A stand-in for api.firecrawl.dev that answers every request with `reply`
// and records each hit as [method, path, whether the multipart form carries
// the options and a PDF file, authorization].
async function withHostedStub(reply, run) {
  const hits = []
  const server = createServer((request, response) => {
    const chunks = []
    request.on('data', (chunk) => chunks.push(chunk))
    request.on('end', () => {
      const body = Buffer.concat(chunks)
      const form =
        /^multipart\/form-data; boundary=/.test(request.headers['content-type']) &&
        body.includes('name="options"') &&
        body.includes('name="file"') &&
        body.includes('%PDF-')
      hits.push([request.method, request.url, form, request.headers.authorization])
      response.writeHead(reply.status, { 'content-type': 'application/json' })
      response.end(JSON.stringify(reply.body))
    })
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  try {
    await run(hits, { ocr: 'hosted', apiUrl: `http://127.0.0.1:${server.address().port}` })
  } finally {
    server.close()
  }
}

const HOSTED = { status: 200, body: { success: true, data: { markdown: '# Read by the hosted parser\n' } } }

test("ocr: 'hosted' sends a pdf with scanned pages to Firecrawl Parse, and nothing else", async () => {
  await withHostedStub(HOSTED, async (hits, options) => {
    assert.equal(await toMarkdownBytes(MIXED, null, options), HOSTED.body.data.markdown)
    assert.deepEqual(hits, [['POST', '/v2/parse', true, undefined]])
    assert.match(await toMarkdownBytes(OUTLINE, null, options), /^# /m)
    assert.deepEqual(hits, [['POST', '/v2/parse', true, undefined]])
  })
})

test('an api key goes as a bearer token', async () => {
  await withHostedStub(HOSTED, async (hits, options) => {
    await toMarkdownBytes(MIXED, null, { ...options, apiKey: 'fc-test' })
    assert.deepEqual(hits, [['POST', '/v2/parse', true, 'Bearer fc-test']])
  })
})

test('an unreachable Firecrawl Parse rejects as hosted', async () => {
  // Port 1 on loopback refuses the connection.
  await assert.rejects(toMarkdownBytes(MIXED, null, { ocr: 'hosted', apiUrl: 'http://127.0.0.1:1' }), (error) => {
    assert.equal(error.code, 'hosted')
    assert.match(error.message, /^Firecrawl Parse: /)
    assert.ok(error.cause)
    return true
  })
})

test('the keyless limit says to pass an api key', async () => {
  const limited = { status: 429, body: { success: false, error: 'Rate limit exceeded' } }
  await withHostedStub(limited, async (_, options) => {
    await assert.rejects(toMarkdownBytes(MIXED, null, options), (error) => {
      assert.equal(error.code, 'hosted')
      assert.match(error.message, /pass apiKey/)
      return true
    })
  })
})
