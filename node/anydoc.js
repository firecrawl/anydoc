'use strict'

const { readFile } = require('node:fs/promises')
const { basename } = require('node:path')

const native = require('./index.js')
const { version } = require('./package.json')

const API_URL = 'https://api.firecrawl.dev'
const TIMEOUT_MS = 300_000

/**
 * Convert a document file to Markdown. `options.ocr` decides what happens to
 * a PDF whose pages need OCR: `'reject'` (the default) rejects with
 * `needsOcr`, `'hosted'` sends the document to Firecrawl Parse instead.
 */
async function toMarkdown(path, options) {
  try {
    return await native.toMarkdown(path)
  } catch (error) {
    if (!sendsToHosted(error, options)) throw error
    return parseHosted(await readFile(path), basename(path), options)
  }
}

/** `toMarkdown` for bytes; `options` as there. */
async function toMarkdownBytes(bytes, format, options) {
  try {
    return await native.toMarkdownBytes(bytes, format)
  } catch (error) {
    if (!sendsToHosted(error, options)) throw error
    return parseHosted(bytes, 'document.pdf', options)
  }
}

function sendsToHosted(error, options) {
  return error.code === 'needsOcr' && options?.ocr === 'hosted'
}

// The whole document goes, not only the pages that need OCR: Parse has no
// page selection.
async function parseHosted(bytes, filename, options) {
  const apiKey = options.apiKey ?? process.env.FIRECRAWL_API_KEY
  const apiUrl = options.apiUrl ?? process.env.FIRECRAWL_API_URL ?? API_URL
  const url = `${apiUrl.replace(/\/$/, '')}/v2/parse`
  const parse = { parsers: [{ type: 'pdf', mode: 'auto' }], origin: `anydoc@${version}` }
  const body = new FormData()
  body.append('options', JSON.stringify(parse))
  body.append('file', new Blob([bytes], { type: 'application/pdf' }), filename)
  const headers = apiKey ? { authorization: `Bearer ${apiKey}` } : {}
  let response
  let reply
  try {
    response = await fetch(url, { method: 'POST', headers, body, signal: AbortSignal.timeout(TIMEOUT_MS) })
    reply = (await response.json().catch(() => null)) ?? {}
  } catch (error) {
    throw hostedError(`Firecrawl Parse: ${error.message}`, error)
  }
  if (!response.ok || !reply.success) {
    throw hostedError(describe(response.status, reply.error ?? response.statusText, Boolean(apiKey)))
  }
  const markdown = reply.data?.markdown
  if (typeof markdown !== 'string' || !markdown) throw hostedError('Firecrawl Parse returned no Markdown')
  return markdown.endsWith('\n') ? markdown : `${markdown}\n`
}

function describe(status, detail, keyed) {
  switch (status) {
    case 401:
      return `Firecrawl Parse rejected the API key: ${detail}`
    case 402:
      return `Firecrawl Parse is out of credits: ${detail}`
    case 429:
      return keyed
        ? `Firecrawl Parse rate limit reached: ${detail}`
        : `Firecrawl Parse keyless limit reached, set FIRECRAWL_API_KEY: ${detail}`
    default:
      return `Firecrawl Parse: ${detail}`
  }
}

function hostedError(message, cause) {
  const error = cause === undefined ? new Error(message) : new Error(message, { cause })
  error.code = 'hosted'
  return error
}

// Spelled out so ESM `import { ... }` sees the names.
module.exports.BlockKind = native.BlockKind
module.exports.CellSlotKind = native.CellSlotKind
module.exports.Format = native.Format
module.exports.formatFromBytes = native.formatFromBytes
module.exports.formatFromExtension = native.formatFromExtension
module.exports.formatFromPath = native.formatFromPath
module.exports.ImageSourceKind = native.ImageSourceKind
module.exports.InlineKind = native.InlineKind
module.exports.LinkTargetKind = native.LinkTargetKind
module.exports.MarkerKind = native.MarkerKind
module.exports.NoteKind = native.NoteKind
module.exports.TableKind = native.TableKind
module.exports.toDocument = native.toDocument
module.exports.toMarkdown = toMarkdown
module.exports.toMarkdownBytes = toMarkdownBytes
