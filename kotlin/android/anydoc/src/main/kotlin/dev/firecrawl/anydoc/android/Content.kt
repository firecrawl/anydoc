package dev.firecrawl.anydoc.android

import android.content.ContentResolver
import android.net.Uri
import dev.firecrawl.anydoc.ConvertException
import dev.firecrawl.anydoc.Document
import dev.firecrawl.anydoc.Format
import dev.firecrawl.anydoc.MAX_DOCUMENT_BYTES
import dev.firecrawl.anydoc.readDocumentBytes
import dev.firecrawl.anydoc.toDocument
import dev.firecrawl.anydoc.toMarkdownBytes
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Read a `content://` or `file://` URI into memory, capped at
 * [MAX_DOCUMENT_BYTES]. Android scoped storage does not give the Rust crate
 * a usable filesystem path.
 */
fun ContentResolver.readDocumentBytes(
    uri: Uri,
    maxBytes: Int = MAX_DOCUMENT_BYTES,
): ByteArray =
    openInputStream(uri)?.use { it.readDocumentBytes(maxBytes) }
        ?: throw ConvertException.Io("unable to open document")

fun ContentResolver.toMarkdown(uri: Uri, format: Format? = null): String =
    toMarkdownBytes(readDocumentBytes(uri), format)

fun ContentResolver.toDocument(uri: Uri, format: Format? = null): Document =
    toDocument(readDocumentBytes(uri), format)

suspend fun ContentResolver.toMarkdownAsync(uri: Uri, format: Format? = null): String {
    val bytes = withContext(Dispatchers.IO) { readDocumentBytes(uri) }
    return withContext(Dispatchers.Default) { toMarkdownBytes(bytes, format) }
}

suspend fun ContentResolver.toDocumentAsync(uri: Uri, format: Format? = null): Document {
    val bytes = withContext(Dispatchers.IO) { readDocumentBytes(uri) }
    return withContext(Dispatchers.Default) { toDocument(bytes, format) }
}
