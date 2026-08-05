// Smoke test: the bindings load and every entry point round-trips a fixture.
import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { test } from 'node:test'
import { promisify } from 'node:util'

import {
  formatFromBytes,
  formatFromExtension,
  formatFromPath,
  toDocument,
  toMarkdown,
  toMarkdownBytes,
} from './index.js'

const fixture = (name) => fileURLToPath(new URL(`../tests/fixtures/${name}`, import.meta.url))

const OUTLINE = fixture('docx/handmade-outline.docx')
const RICH = fixture('docx/handmade-rich.docx')
const CSV = fixture('csv/sheet.csv')

test('toMarkdown detects the format from the file content', async () => {
  const markdown = await toMarkdown(OUTLINE)
  assert.match(markdown, /^# /m)
})

test('toMarkdownBytes converts in memory', async () => {
  const markdown = await toMarkdownBytes(await readFile(RICH), 'docx')
  assert.match(markdown, /\| Quarter \| Widgets \|/)
})

test('toMarkdownBytes detects the format when none is named', async () => {
  const markdown = await toMarkdownBytes(await readFile(RICH))
  assert.match(markdown, /\| Quarter \| Widgets \|/)
  // CSV carries no signature, so it has to be named.
  await assert.rejects(toMarkdownBytes(await readFile(CSV)), /unrecognized file content/)
  assert.match(await toMarkdownBytes(await readFile(CSV), 'csv'), /\| --- \|/)
})

test('toDocument exposes the document model', async () => {
  const document = await toDocument(await readFile(OUTLINE), 'docx')
  const heading = document.blocks.find((block) => block.kind === 'heading')
  assert.ok(heading.level >= 1 && heading.level <= 6)
  assert.equal(typeof heading.content[0].text, 'string')
  assert.equal(heading.content[0].kind, 'text')
  assert.equal(typeof heading.content[0].style.bold, 'boolean')
  assert.ok(Array.isArray(document.outline))
  assert.equal(document.outline[0].level, heading.level)
  assert.equal(typeof document.outline[0].text, 'string')
  assert.equal(typeof document.outline[0].level, 'number')
})

test('toDocument carries embedded assets as buffers', async () => {
  const document = await toDocument(await readFile(RICH), 'docx')
  const image = document.assets.find((asset) => asset.mediaType === 'image/png')
  assert.ok(Buffer.isBuffer(image.data))
  assert.ok(image.data.length > 0)
  assert.equal(image.id, document.assets.indexOf(image))
})

test('format detection reads content, extension, and path', async () => {
  assert.equal(formatFromBytes(await readFile(RICH)), 'docx')
  // CSV carries no signature: only the extension names it.
  assert.equal(formatFromBytes(await readFile(CSV)), null)
  assert.equal(formatFromExtension('.pptm'), 'pptx')
  assert.equal(formatFromExtension('xls'), 'xlsx')
  assert.equal(formatFromPath('/tmp/report.odt'), 'odt')
  assert.equal(formatFromPath('/tmp/report.unknown'), null)
})

test('conversion errors reject with the crate error message', async () => {
  await assert.rejects(toMarkdownBytes(Buffer.from('not a document'), 'docx'), /malformed|unsupported/)
})

const CLI = fileURLToPath(new URL('./cli.js', import.meta.url))

// execFile rejects on a non-zero exit; the error carries code/stdout/stderr.
const runCli = (args, options) =>
  promisify(execFile)(process.execPath, [CLI, ...args], options).catch((error) => error)

test('cli converts a file to stdout', async () => {
  const { code, stdout, stderr } = await runCli([OUTLINE])
  assert.equal(code, undefined)
  assert.match(stdout, /^# /m)
  assert.equal(stderr, '')
})

test('cli writes to --output instead of stdout', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'anydoc-cli-'))
  try {
    const out = join(dir, 'outline.md')
    const { code, stdout } = await runCli([OUTLINE, '-o', out])
    assert.equal(code, undefined)
    assert.equal(stdout, '')
    assert.match(await readFile(out, 'utf8'), /^# /m)
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
})

test('cli reads stdin with an explicit format', async () => {
  const child = promisify(execFile)(process.execPath, [CLI, '-', '--format', 'csv'])
  child.child.stdin.end(await readFile(CSV))
  const { stdout } = await child
  assert.match(stdout, /\| --- \|/)
})

test('cli exits 1 when the document cannot be converted', async () => {
  const { code, stderr } = await runCli(['no-such-file.docx'])
  assert.equal(code, 1)
  assert.match(stderr, /^anydoc: /)
})

test('cli exits 2 on usage errors', async () => {
  for (const args of [[], ['--frmat', 'csv', CSV], ['--format', 'nope', CSV], [OUTLINE, RICH]]) {
    const { code, stderr } = await runCli(args)
    assert.equal(code, 2)
    assert.match(stderr, /^anydoc: /)
  }
})

test('cli help and version go to stdout and exit 0', async () => {
  const help = await runCli(['--help'])
  assert.equal(help.code, undefined)
  assert.match(help.stdout, /Exit codes:/)
  const version = await runCli(['--version'])
  assert.match(version.stdout.trim(), /^\d+\.\d+\.\d+/)
})
