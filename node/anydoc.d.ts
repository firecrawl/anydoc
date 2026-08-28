export * from './index.js'
import type { Format } from './index.js'

/** What happens to a PDF whose pages need OCR. */
export interface ConvertOptions {
  /**
   * `reject` (the default) rejects with `needsOcr` naming the pages.
   * `hosted` sends the whole document to Firecrawl Parse instead, keyless
   * unless a key is given. Documents anydoc converts itself never leave the
   * machine.
   */
  ocr?: 'reject' | 'hosted'
  /** Firecrawl API key for `hosted`, else `FIRECRAWL_API_KEY`, else keyless. */
  apiKey?: string
  /** Firecrawl API URL for `hosted`, else `FIRECRAWL_API_URL`, else `https://api.firecrawl.dev`. */
  apiUrl?: string
}

/**
 * Convert a document file to Markdown. The format is detected from the file
 * content; the extension is the fallback for signature-less formats (CSV)
 * and unrecognizable containers.
 *
 * Rejects with an `Error` carrying a `ConvertErrorCode` on `code`; a file
 * that cannot be read is `'io'`.
 */
export declare function toMarkdown(path: string, options?: ConvertOptions): Promise<string>

/**
 * Convert an in-memory document to Markdown. Without a format, it is
 * detected from the content, which signature-less formats (CSV) have to name
 * explicitly.
 *
 * Rejects with an `Error` carrying a `ConvertErrorCode` on `code`.
 */
export declare function toMarkdownBytes(
  bytes: Uint8Array,
  format?: Format | null,
  options?: ConvertOptions,
): Promise<string>
