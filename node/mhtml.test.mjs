import test from 'node:test'
import assert from 'node:assert/strict'
import { formatFromBytes, formatFromExtension, toMarkdownBytes } from './index.js'

const input = Buffer.from(
  'Snapshot-Content-Location: https://example.test/page\r\n' +
  'MIME-Version: 1.0\r\n' +
  'Content-Type: multipart/related; type="text/html"; boundary="b"\r\n\r\n' +
  '--b\r\nContent-Type: text/html\r\n\r\n' +
  '<!doctype html><h1>Hello MHTML</h1>\r\n--b--\r\n'
)

test('MHTML is exposed through the Node binding', async () => {
  assert.equal(formatFromExtension('mhtml'), 'mhtml')
  assert.equal(formatFromExtension('mht'), 'mhtml')
  assert.equal(formatFromBytes(input), 'mhtml')
  assert.equal(await toMarkdownBytes(input), '# Hello MHTML\n')
})
