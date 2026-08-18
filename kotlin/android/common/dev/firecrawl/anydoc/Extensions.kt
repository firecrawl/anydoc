@file:JvmName("AnydocExtensions")

package dev.firecrawl.anydoc

import java.io.ByteArrayOutputStream
import java.io.InputStream
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Hard cap on a single in-memory document read (URI helper and
 * [readDocumentBytes]). Matches anydoc's `MAX_ENTRY_BYTES` (128 MiB) so a
 * hostile `content://` stream cannot OOM the process before Rust sees it.
 */
const val MAX_DOCUMENT_BYTES: Int = 128 * 1024 * 1024

/**
 * Copy [this] into a [ByteArray], refusing anything larger than [maxBytes].
 * Throws [ConvertException.ResourceLimit] rather than growing without bound.
 */
fun InputStream.readDocumentBytes(maxBytes: Int = MAX_DOCUMENT_BYTES): ByteArray {
    require(maxBytes > 0) { "maxBytes must be positive" }
    val out = ByteArrayOutputStream()
    val buf = ByteArray(64 * 1024)
    var total = 0L
    val limit = maxBytes.toLong()
    while (true) {
        val n = read(buf)
        if (n < 0) break
        total += n.toLong()
        if (total > limit) {
            throw ConvertException.ResourceLimit(
                "max_input_bytes",
                "document exceeds $maxBytes bytes",
            )
        }
        out.write(buf, 0, n)
    }
    return out.toByteArray()
}

/**
 * UniFFI exports the Rust `ConvertError` as [ConvertException]. `code` is
 * the stable name callers branch on, matching Node's `error.code`.
 */
val ConvertException.code: String
    get() = when (this) {
        is ConvertException.Unsupported -> "unsupported"
        is ConvertException.Malformed -> "malformed"
        is ConvertException.Encrypted -> "encrypted"
        is ConvertException.ResourceLimit -> "resourceLimit"
        is ConvertException.MissingPart -> "missingPart"
        is ConvertException.Io -> "io"
    }

/** Full sentence matching the Rust `Display` for this failure. */
val ConvertException.reason: String
    get() = when (this) {
        is ConvertException.Unsupported -> "unsupported input: $detail"
        is ConvertException.Malformed ->
            if (part != null) "malformed document ($part): $detail" else "malformed document: $detail"
        is ConvertException.Encrypted -> "document is encrypted"
        is ConvertException.ResourceLimit -> "resource limit exceeded ($limit): $detail"
        is ConvertException.MissingPart -> "missing required part: $part"
        is ConvertException.Io -> "io error: $detail"
    }

/** [toMarkdown] on a background IO dispatcher. */
suspend fun toMarkdownAsync(path: String): String =
    withContext(Dispatchers.IO) { toMarkdown(path) }

/** [toMarkdownBytes] on a background CPU dispatcher. */
suspend fun toMarkdownBytesAsync(bytes: ByteArray, format: Format? = null): String =
    withContext(Dispatchers.Default) { toMarkdownBytes(bytes, format) }

/** Optional-format overload so callers are not forced to pass `null`. */
fun toMarkdownBytes(bytes: ByteArray): String = toMarkdownBytes(bytes, null)

/** [toDocument] on a background CPU dispatcher. */
suspend fun toDocumentAsync(bytes: ByteArray, format: Format? = null): Document =
    withContext(Dispatchers.Default) { toDocument(bytes, format) }

/** Optional-format overload so callers are not forced to pass `null`. */
fun toDocument(bytes: ByteArray): Document = toDocument(bytes, null)
