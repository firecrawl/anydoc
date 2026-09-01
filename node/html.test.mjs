import test from 'node:test'
import assert from 'node:assert/strict'
import { formatFromBytes, formatFromExtension, toMarkdownBytes } from './index.js'

test('standalone HTML is exposed through the Node binding', async () => {
  const input = Buffer.from('<!doctype html><h1>Hello</h1><p><b>world</b></p>')
  assert.equal(formatFromExtension('html'), 'html')
  assert.equal(formatFromBytes(input), 'html')
  assert.equal(await toMarkdownBytes(input), '# Hello\n\n**world**\n')
})
