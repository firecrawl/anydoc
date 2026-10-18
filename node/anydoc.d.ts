export * from './index.js'
import type { Conversion, Format } from './index.js'

/** What happens to a PDF whose pages need OCR. */
export interface ConvertOptions {
  /**
   * `reject` (the default) rejects with `needsOcr` naming the pages.
   * `hosted` sends the whole document to Firecrawl Parse instead, keyless
   * unless a key is given. Documents anydoc converts itself never leave the
   * machine. `skip` converts the pages that carry text locally and resolves
   * to a `Conversion` naming the pages it left out; a document where no page
   * yielded text still rejects with `needsOcr`.
   */
  ocr?: 'reject' | 'hosted' | 'skip'
  /** Firecrawl API key for `hosted`, else `FIRECRAWL_API_KEY`, else keyless. */
  apiKey?: string
  /** Firecrawl API URL for `hosted`, else `FIRECRAWL_API_URL`, else `https://api.firecrawl.dev`. */
  apiUrl?: string
}

/** `ConvertOptions` that resolve to a `Conversion` instead of a string. */
export interface SkipOcrOptions extends ConvertOptions {
  ocr: 'skip'
}

/**
 * Convert a document file to Markdown. The format is detected from the file
 * content; the extension is the fallback for signature-less formats (CSV)
 * and unrecognizable containers.
 *
 * Rejects with an `Error` carrying a `ConvertErrorCode` on `code`; a file
 * that cannot be read is `'io'`.
 */
export declare function toMarkdown(path: string, options: SkipOcrOptions): Promise<Conversion>
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
  format: Format | null | undefined,
  options: SkipOcrOptions,
): Promise<Conversion>
export declare function toMarkdownBytes(
  bytes: Uint8Array,
  format?: Format | null,
  options?: ConvertOptions,
): Promise<string>
