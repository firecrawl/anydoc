#!/usr/bin/env node
'use strict'

const { readFile, writeFile } = require('node:fs/promises')

const FORMATS = 'doc, docx, odt, pdf, ppt, pptx, rtf, epub, xlsx, ods, odp, csv'

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
  -p, --password <pw>    Decrypt a password-protected OOXML file first.
                         Falls back to the ANYDOC_PASSWORD environment
                         variable when omitted (argv leaks into shell
                         history and ps).
  -h, --help             Print this help and exit
  -V, --version          Print the version and exit

The format is detected from the file content; the file extension is the
fallback for signature-less formats (CSV). stdin has no extension, so CSV
input from stdin needs --format csv. Scanned or image-only PDFs need OCR,
which anydoc does not do, and error as unsupported.

Exit codes:
  0  success
  1  the document could not be read or converted
  2  usage error: unknown option, missing input, or invalid --format

Examples:
  anydoc report.docx
  anydoc slides.pptx -o slides.md
  anydoc - --format csv < data.csv
  curl -s https://example.com/paper.pdf | anydoc -
`

const USAGE_ERROR = 2
const CONVERSION_ERROR = 1

function fail(code, message) {
  process.stderr.write(`anydoc: ${message}\n`)
  process.exit(code)
}

function parseArgs(argv) {
  const args = { input: null, output: null, format: null, password: process.env.ANYDOC_PASSWORD || null }
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
      case '-p':
      case '--password':
        args.password = value()
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
  const {
  formatFromBytes,
  formatFromExtension,
  formatFromPath,
  toMarkdown,
  toMarkdownBytes,
} = require('./index.js')

  let format
  if (args.format !== null) {
    format = formatFromExtension(args.format)
    if (format === null) {
      fail(USAGE_ERROR, `invalid format '${args.format}'; expected one of: ${FORMATS}`)
    }
  }

  let markdown
  try {
    // A password only reaches the byte-level entry points; a path without
    // --format would otherwise drop it on the floor (#130 review).
    if (args.input === '-' || format !== undefined || args.password !== null) {
      const bytes = await (args.input === '-' ? readStdin() : readFile(args.input))
      // CSV has no content signature, so the path extension stays the last
      // fallback exactly as the no-password flow treats it (#130 review).
      const resolved = format ?? formatFromBytes(bytes) ?? formatFromPath(args.input)
      markdown = await toMarkdownBytes(bytes, resolved, args.password)
    } else {
      markdown = await toMarkdown(args.input)
    }
  } catch (error) {
    fail(CONVERSION_ERROR, error.message)
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
