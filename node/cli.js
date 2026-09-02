#!/usr/bin/env node
'use strict'

const { readFile, writeFile } = require('node:fs/promises')

const FORMATS = 'doc, docx, odt, pdf, ppt, pptx, rtf, epub, xlsx, ods, odp, csv, msg'

const HELP = `anydoc: convert documents to GitHub-Flavored Markdown

Usage:
  anydoc <file> [options]
  anydoc - [options] < file

Converts one document per invocation and writes the Markdown to stdout.
Pass - as the input to read the document from stdin. Never prompts; all
diagnostics go to stderr.

Options:
  -o, --output <path>    Write the Markdown to <path> instead of stdout
  -f, --format <format>  Name the input format instead of detecting it:
                         ${FORMATS}
                         (extension aliases like xls, docm, ppsx resolve
                         to these)
  --ocr <mode>           What to do with a PDF whose pages need OCR:
                         reject (default) exits 3; hosted sends the
                         document to Firecrawl Parse
  --api-key <key>        Firecrawl API key for --ocr hosted, else
                         FIRECRAWL_API_KEY, else keyless
  --api-url <url>        Firecrawl API URL for --ocr hosted, else
                         FIRECRAWL_API_URL, else https://api.firecrawl.dev
  -h, --help             Print this help and exit
  -V, --version          Print the version and exit

The format is detected from the file content; the file extension is the
fallback for signature-less formats (CSV). stdin has no extension, so CSV
input from stdin needs --format csv. Scanned or image-only pages need OCR,
which anydoc does not do: the document exits 3, or goes to Firecrawl Parse
with --ocr hosted.

Exit codes:
  0  success
  1  the document could not be read or converted
  2  usage error: unknown option, missing input, or invalid --format
  3  pages of a PDF need OCR

Examples:
  anydoc report.docx
  anydoc slides.pptx -o slides.md
  anydoc - --format csv < data.csv
  curl -s https://example.com/paper.pdf | anydoc -
  anydoc scan.pdf --ocr hosted
`

const OCR_MODES = ['reject', 'hosted']

const USAGE_ERROR = 2
const CONVERSION_ERROR = 1
const NEEDS_OCR = 3

function fail(code, message) {
  process.stderr.write(`anydoc: ${message}\n`)
  process.exit(code)
}

function parseArgs(argv) {
  const args = { input: null, output: null, format: null, ocr: null, apiKey: null, apiUrl: null }
  let positionalOnly = false
  for (let i = 0; i < argv.length; i++) {
    let arg = argv[i]
    if (positionalOnly || arg === '-' || !arg.startsWith('-')) {
      if (args.input !== null) {
        fail(USAGE_ERROR, `one document per invocation: unexpected second input '${arg}'`)
      }
      args.input = arg
      continue
    }
    if (arg === '--') {
      positionalOnly = true
      continue
    }
    let inline = null
    const eq = arg.indexOf('=')
    if (arg.startsWith('--') && eq !== -1) {
      inline = arg.slice(eq + 1)
      arg = arg.slice(0, eq)
    }
    const value = () => {
      if (inline !== null) return inline
      if (i + 1 >= argv.length) fail(USAGE_ERROR, `${arg} requires a value`)
      return argv[++i]
    }
    switch (arg) {
      case '-h':
      case '--help':
        process.stdout.write(HELP)
        process.exit(0)
        break
      case '-V':
      case '--version':
        process.stdout.write(`${require('./package.json').version}\n`)
        process.exit(0)
        break
      case '-o':
      case '--output':
        args.output = value()
        break
      case '-f':
      case '--format':
        args.format = value()
        break
      case '--ocr':
        args.ocr = value()
        if (!OCR_MODES.includes(args.ocr)) {
          fail(USAGE_ERROR, `invalid --ocr '${args.ocr}'; expected one of: ${OCR_MODES.join(', ')}`)
        }
        break
      case '--api-key':
        args.apiKey = value()
        break
      case '--api-url':
        args.apiUrl = value()
        break
      default:
        fail(USAGE_ERROR, `unknown option '${arg}' (see anydoc --help)`)
    }
  }
  return args
}

async function readStdin() {
  if (process.stdin.isTTY) {
    fail(USAGE_ERROR, 'stdin is a terminal; pipe or redirect a document into anydoc -')
  }
  const chunks = []
  for await (const chunk of process.stdin) {
    chunks.push(chunk)
  }
  return Buffer.concat(chunks)
}

async function main() {
  const args = parseArgs(process.argv.slice(2))
  if (args.input === null) {
    fail(USAGE_ERROR, 'missing input: pass a document path, or - for stdin (see anydoc --help)')
  }

  // Loaded after argument handling so --help and --version work even where
  // no native binding is available.
  const { formatFromExtension, toMarkdown, toMarkdownBytes } = require('./anydoc.js')

  let format
  if (args.format !== null) {
    format = formatFromExtension(args.format)
    if (format === null) {
      fail(USAGE_ERROR, `invalid format '${args.format}'; expected one of: ${FORMATS}`)
    }
  }

  const options = { ocr: args.ocr ?? undefined, apiKey: args.apiKey ?? undefined, apiUrl: args.apiUrl ?? undefined }
  let markdown
  try {
    if (args.input === '-') {
      markdown = await toMarkdownBytes(await readStdin(), format, options)
    } else if (format !== undefined) {
      markdown = await toMarkdownBytes(await readFile(args.input), format, options)
    } else {
      markdown = await toMarkdown(args.input, options)
    }
  } catch (error) {
    fail(error.code === 'needsOcr' ? NEEDS_OCR : CONVERSION_ERROR, error.message)
  }

  if (args.output !== null) {
    try {
      await writeFile(args.output, markdown)
    } catch (error) {
      fail(CONVERSION_ERROR, error.message)
    }
  } else {
    // Downstream closing the pipe early (e.g. `anydoc big.xlsx | head`) is
    // not a conversion failure.
    process.stdout.on('error', (error) => {
      process.exit(error.code === 'EPIPE' ? 0 : CONVERSION_ERROR)
    })
    process.stdout.write(markdown)
  }
}

main()
