# ODF `text:p` heading-style audit

Date: 2026-08-05

## Question

Do real producers serialize a heading as `text:p` with a Heading-family
`text:style-name`, instead of using `text:h`?

## Corpus and method

The checked-in binary corpus contains one producer-generated ODT:

| Producer | Files | `text:h` elements | `text:p` with a Heading-family style |
| --- | ---: | ---: | ---: |
| LibreOffice export (`tests/fixtures/odt/text.odt`) | 1 | 7 | 0 |
| Google Docs export | 0 | — | — |
| OnlyOffice export | 0 | — | — |
| Microsoft Word ODF export | 0 | — | — |

The ODT was inspected by extracting `content.xml` and counting element
names and paragraph `text:style-name` values. The three `handmade-*.odt`
fixtures were kept out of the producer count: they exercise parser behavior,
not producer output. The source fixture confirms the same LibreOffice path:
`tests/gen_fixtures.py` creates `text.odt` with the `odt` export filter.

The local qmd corpus contains research Markdown but no additional ODT corpus.
There is no installed Google Docs, OnlyOffice, or Word-generated sample in
this checkout, so those rows are unmeasured rather than zero findings.

## Decision

Do not add a changeset from this evidence. The only real-producer sample uses
`text:h` for all seven headings and contains no `text:p` Heading-family
heading. The other requested producers have no samples here, so promoting
paragraphs based on a style-name convention would be an unverified heuristic
with a weaker signal than ODF's explicit `text:h` and `text:outline-level`.

Revisit only after adding a corpus with at least one attributable export from
each producer. A future implementation would need to resolve paragraph-style
inheritance and `style:default-outline-level`, then prove that ordinary
paragraphs are not misclassified before changing `formats/odf/text.rs`.
