#!/usr/bin/env node
'use strict'

const { copyFile, mkdir, readdir, readFile, stat, writeFile } = require('node:fs/promises')
const { dirname, extname, join, relative, resolve, sep } = require('node:path')

const FORMATS = 'doc, docx, odt, pdf, ppt, pptx, rtf, epub, xlsx, ods, odp, csv'

// Plain-text extensions copied as-is in batch mode (names unchanged).
const PASSTHROUGH_EXTS = new Set(['.txt', '.json', '.md', '.py'])

const HELP = `anydoc: convert documents to GitHub-Flavored Markdown

Usage:
  anydoc <file> [options]
  anydoc --batch <dir> -o <dir> [options]
  anydoc - [options] < file

Converts one document per invocation and writes the Markdown to stdout,
or converts a directory tree in batch mode. Pass - as the input to read
the document from stdin. Never prompts; all diagnostics go to stderr.

Options:
  --batch                Convert every supported file under <dir>
                         recursively into -o, preserving relative paths.
                         Requires -o. Converted files keep the original
                         name and gain a .md suffix (report.pdf becomes
                         report.pdf.md). Plain-text files (.txt, .json,
                         .md, .py) are copied with names unchanged.
                         Unsupported files are skipped silently. Existing
                         outputs are overwritten. A conversion failure on
                         one file is logged to stderr and the run continues;
                         the process exits 1 if any conversion failed.
  -o, --output <path>    Write Markdown to <path> (single-file), or the
                         output directory (required with --batch)
  -f, --format <format>  Name the input format instead of detecting it:
                         ${FORMATS}
                         (extension aliases like xls, docm, ppsx resolve
                         to these). Not valid with --batch.
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
  1  the document could not be read or converted (in batch mode: at least
     one file failed to convert)
  2  usage error: unknown option, missing input, or invalid --format
  3  pages of a PDF need OCR

Examples:
  anydoc report.docx
  anydoc slides.pptx -o slides.md
  anydoc --batch ./docs -o ./out
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
  const args = { input: null, output: null, format: null, batch: false, ocr: null, apiKey: null, apiUrl: null }
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
      case '--batch':
        // Allow --batch <dir> (value form) or --batch with a separate positional.
        if (inline !== null) {
          if (args.input !== null) {
            fail(USAGE_ERROR, `one document per invocation: unexpected second input '${inline}'`)
          }
          args.input = inline
        } else if (i + 1 < argv.length && !argv[i + 1].startsWith('-')) {
          if (args.input !== null) {
            fail(USAGE_ERROR, `one document per invocation: unexpected second input '${argv[i + 1]}'`)
          }
          args.input = argv[++i]
        }
        args.batch = true
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

function isPassthrough(path) {
  return PASSTHROUGH_EXTS.has(extname(path).toLowerCase())
}

/** Walk a directory tree depth-first; yield absolute file paths. */
async function* walkFiles(root) {
  const entries = await readdir(root, { withFileTypes: true })
  for (const entry of entries) {
    const path = join(root, entry.name)
    if (entry.isDirectory()) {
      yield* walkFiles(path)
    } else if (entry.isFile()) {
      yield path
    }
  }
}

async function runBatch(args, { formatFromPath, toMarkdown }) {
  if (args.input === '-') {
    fail(USAGE_ERROR, '--batch does not read stdin; pass an input directory')
  }
  if (args.output === null) {
    fail(USAGE_ERROR, '--batch requires -o/--output naming an output directory')
  }
  if (args.format !== null) {
    fail(USAGE_ERROR, '--format is not valid with --batch (format is detected per file)')
  }

  let inputStat
  try {
    inputStat = await stat(args.input)
  } catch (error) {
    fail(CONVERSION_ERROR, error.message)
  }
  if (!inputStat.isDirectory()) {
    fail(USAGE_ERROR, `--batch expects a directory, got '${args.input}'`)
  }

  const inputRoot = resolve(args.input)
  const outputRoot = resolve(args.output)

  try {
    await mkdir(outputRoot, { recursive: true })
  } catch (error) {
    fail(CONVERSION_ERROR, error.message)
  }

  let failed = 0
  for await (const absPath of walkFiles(inputRoot)) {
    const rel = relative(inputRoot, absPath)
    // Guard against path escape on odd relative() results.
    if (rel.startsWith(`..${sep}`) || rel === '..') continue

    if (isPassthrough(absPath)) {
      const outPath = join(outputRoot, rel)
      try {
        await mkdir(dirname(outPath), { recursive: true })
        await copyFile(absPath, outPath)
      } catch (error) {
        process.stderr.write(`anydoc: ${rel}: ${error.message}\n`)
        failed++
      }
      continue
    }

    if (formatFromPath(absPath) === null) {
      // Unsupported (images, videos, archives, unknown extensions): skip.
      continue
    }

    const outPath = join(outputRoot, `${rel}.md`)
    const options = { ocr: args.ocr ?? undefined, apiKey: args.apiKey ?? undefined, apiUrl: args.apiUrl ?? undefined }
    try {
      const markdown = await toMarkdown(absPath, options)
      await mkdir(dirname(outPath), { recursive: true })
      await writeFile(outPath, markdown)
    } catch (error) {
      process.stderr.write(`anydoc: ${rel}: ${error.message}\n`)
      failed++
    }
  }

  if (failed > 0) {
    process.exit(CONVERSION_ERROR)
  }
}

async function runSingle(args, { formatFromExtension, toMarkdown, toMarkdownBytes }) {
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

async function main() {
  const args = parseArgs(process.argv.slice(2))
  if (args.input === null) {
    fail(
      USAGE_ERROR,
      args.batch
        ? 'missing input: pass a directory with --batch (see anydoc --help)'
        : 'missing input: pass a document path, or - for stdin (see anydoc --help)',
    )
  }

  // Loaded after argument handling so --help and --version work even where
  // no native binding is available.
  const bindings = require('./anydoc.js')

  if (args.batch) {
    await runBatch(args, bindings)
  } else {
    await runSingle(args, bindings)
  }
}

main()
