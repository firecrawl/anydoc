// The math frontends claim to emit LaTeX inside the subset KaTeX implements.
// Nothing measured that claim, so this renders every equation the fixture
// corpus produces and fails on the first one KaTeX will not accept.
import assert from 'node:assert/strict'
import { readFile, readdir } from 'node:fs/promises'
import { extname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { test } from 'node:test'

import katex from 'katex'

import { toDocument } from './index.js'

const FIXTURES = fileURLToPath(new URL('../tests/fixtures', import.meta.url))

// PDFs bypass the document model, CSV carries no styling, and the malformed
// and abuse corpora exist to fail rather than to convert.
const SKIP_DIRS = new Set(['pdf', 'csv', 'malformed', 'abuse'])

function collect(inlines, out) {
  for (const inline of inlines ?? []) {
    if (inline.kind === 'math') out.push(inline)
    collect(inline.content, out)
  }
}

function walk(blocks, out) {
  for (const block of blocks ?? []) {
    collect(block.content, out)
    walk(block.blocks, out)
    for (const item of block.items ?? []) walk(item.blocks, out)
    for (const row of block.rows ?? []) {
      for (const cell of row.cells ?? []) walk(cell.blocks, out)
    }
  }
}

async function equationsIn(path) {
  const document = await toDocument(await readFile(path))
  const found = []
  walk(document.blocks, found)
  for (const note of document.notes ?? []) walk(note.blocks, found)
  return found
}

async function fixturePaths() {
  const paths = []
  for (const dir of await readdir(FIXTURES, { withFileTypes: true })) {
    if (!dir.isDirectory() || SKIP_DIRS.has(dir.name)) continue
    for (const name of await readdir(join(FIXTURES, dir.name))) {
      if (extname(name)) paths.push(join(FIXTURES, dir.name, name))
    }
  }
  return paths.sort()
}

test('every equation the corpus produces renders in KaTeX', async () => {
  const counts = new Map()
  for (const path of await fixturePaths()) {
    let equations
    try {
      equations = await equationsIn(path)
    } catch {
      continue // Unconvertible fixtures are another test's subject.
    }
    for (const { latex, display } of equations) {
      assert.doesNotThrow(
        () => katex.renderToString(latex, { throwOnError: true, displayMode: display }),
        `${path}: ${latex}`,
      )
      counts.set(path, (counts.get(path) ?? 0) + 1)
    }
  }

  // A walk that quietly stops finding anything would otherwise pass while
  // measuring nothing, so each format that carries equations must contribute.
  const withMath = [...counts.keys()]
  for (const path of await fixturePaths()) {
    if (!path.includes('handmade-math')) continue
    assert.ok(counts.has(path), `no equation reached the document model from ${path}`)
  }
  assert.ok(withMath.length >= 4, `only ${withMath.length} fixtures carried equations`)
})
