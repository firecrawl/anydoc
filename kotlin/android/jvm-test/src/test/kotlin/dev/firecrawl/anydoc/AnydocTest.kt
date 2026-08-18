package dev.firecrawl.anydoc

import java.io.ByteArrayInputStream
import java.nio.file.Files
import java.nio.file.Path
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream
import kotlin.io.path.readBytes
import kotlin.test.Test
import kotlin.test.assertContains
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

class AnydocTest {
    private val fixtures: Path = fixtureRoot()

    @Test
    fun toMarkdownDetectsTheFormatFromTheFileContent() {
        val markdown = toMarkdown(fixtures.resolve("docx/handmade-outline.docx").toString())
        assertTrue(markdown.lineSequence().any { it.startsWith("# ") }, markdown)
    }

    @Test
    fun toMarkdownBytesConvertsInMemory() {
        val markdown = toMarkdownBytes(
            fixtures.resolve("docx/handmade-rich.docx").readBytes(),
            Format.DOCX,
        )
        assertContains(markdown, "| Quarter | Widgets |")
    }

    @Test
    fun toMarkdownBytesDetectsTheFormatWhenNoneIsNamed() {
        val rich = fixtures.resolve("docx/handmade-rich.docx").readBytes()
        assertContains(toMarkdownBytes(rich), "| Quarter | Widgets |")

        val csv = fixtures.resolve("csv/sheet.csv").readBytes()
        val unsupported = assertFailsWith<ConvertException.Unsupported> { toMarkdownBytes(csv) }
        assertContains(unsupported.detail, "unrecognized file content")
        assertEquals("unsupported", unsupported.code)
        assertContains(toMarkdownBytes(csv, Format.CSV), "| --- |")
    }

    @Test
    fun toDocumentExposesTheDocumentModel() {
        val document = toDocument(
            fixtures.resolve("docx/handmade-outline.docx").readBytes(),
            Format.DOCX,
        )
        val heading = document.blocks.filterIsInstance<Block.Heading>().first()
        assertTrue(heading.level.toUInt() in 1u..6u)
        val text = assertIs<Inline.Text>(heading.content.first())
        assertTrue(text.text.isNotEmpty())
        assertIs<Boolean>(text.style.bold)
    }

    @Test
    fun toDocumentCarriesEmbeddedAssetsAsBytes() {
        val document = toDocument(
            fixtures.resolve("docx/handmade-rich.docx").readBytes(),
            Format.DOCX,
        )
        val image = document.assets.first { it.mediaType == "image/png" }
        assertTrue(image.data.isNotEmpty())
        assertEquals(document.assets.indexOfFirst { it.id == image.id }.toUInt(), image.id)
    }

    @Test
    fun formatDetectionReadsContentExtensionAndPath() {
        assertEquals(
            Format.DOCX,
            formatFromBytes(fixtures.resolve("docx/handmade-rich.docx").readBytes()),
        )
        assertNull(formatFromBytes(fixtures.resolve("csv/sheet.csv").readBytes()))
        assertEquals(Format.PPTX, formatFromExtension(".pptm"))
        assertEquals(Format.XLSX, formatFromExtension("xls"))
        assertEquals(Format.ODT, formatFromPath("/tmp/report.odt"))
        assertNull(formatFromPath("/tmp/report.unknown"))
    }

    @Test
    fun conversionErrorsNameTheFailure() {
        val malformed = assertFailsWith<ConvertException.Malformed> {
            toMarkdownBytes("not a document".toByteArray(), Format.DOCX)
        }
        assertNull(malformed.part)
        assertEquals("malformed", malformed.code)

        assertFailsWith<ConvertException.Unsupported> {
            toMarkdownBytes(fixtures.resolve("csv/sheet.csv").readBytes())
        }

        assertFailsWith<ConvertException.Encrypted> {
            toMarkdownBytes(
                fixtures.resolve("malformed/encrypted--errors.odt").readBytes(),
                Format.ODT,
            )
        }
        assertFailsWith<ConvertException.Encrypted> {
            toDocument(
                fixtures.resolve("malformed/encrypted--errors.odt").readBytes(),
                Format.ODT,
            )
        }

        val limit = assertFailsWith<ConvertException.ResourceLimit> {
            toMarkdownBytes(
                fixtures.resolve("abuse/zipbomb--errors.docx").readBytes(),
                Format.DOCX,
            )
        }
        assertEquals("max_entry_bytes", limit.limit)
        assertEquals("resourceLimit", limit.code)

        val io = assertFailsWith<ConvertException.Io> { toMarkdown("no-such-file.docx") }
        assertEquals("io", io.code)
        assertContains(io.reason, "io error")
    }

    @Test
    fun missingPartNamesThePart() {
        val packageBytes = zipOf("[Content_Types].xml" to "<Types/>".toByteArray())
        val missing = assertFailsWith<ConvertException.MissingPart> {
            toMarkdownBytes(packageBytes, Format.DOCX)
        }
        assertEquals("word/document.xml", missing.part)
        assertEquals("missingPart", missing.code)
    }

    @Test
    fun streamReadStopsAtTheDocumentByteCap() {
        val over = ByteArray(16) { 1 }
        val limit = assertFailsWith<ConvertException.ResourceLimit> {
            ByteArrayInputStream(over).readDocumentBytes(maxBytes = 8)
        }
        assertEquals("max_input_bytes", limit.limit)
        assertEquals("resourceLimit", limit.code)

        val under = byteArrayOf(1, 2, 3, 4)
        assertEquals(
            under.toList(),
            ByteArrayInputStream(under).readDocumentBytes(maxBytes = 8).toList(),
        )
    }

    private fun zipOf(vararg files: Pair<String, ByteArray>): ByteArray {
        val out = java.io.ByteArrayOutputStream()
        ZipOutputStream(out).use { zip ->
            for ((name, bytes) in files) {
                zip.putNextEntry(ZipEntry(name))
                zip.write(bytes)
                zip.closeEntry()
            }
        }
        return out.toByteArray()
    }

    companion object {
        private fun fixtureRoot(): Path {
            var dir = Path.of("").toAbsolutePath()
            repeat(8) {
                val candidate = dir.resolve("tests").resolve("fixtures")
                if (Files.exists(candidate)) {
                    return candidate
                }
                dir = dir.parent ?: error("could not find tests/fixtures from ${Path.of("").toAbsolutePath()}")
            }
            error("could not find tests/fixtures")
        }
    }
}
