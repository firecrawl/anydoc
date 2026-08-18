# anydoc for Kotlin / Android

Convert Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, CSV, and PDF files
into clean GitHub-Flavored Markdown. Kotlin bindings for the [anydoc](https://github.com/firecrawl/anydoc)
Rust crate. The parsers stay in Rust; this package is the Android/JVM surface,
the same role [Node](../node/README.md) and [Python](../python/README.md) play
for those languages.

The GitHub Action **Kotlin Android** (`kotlin.yml`) builds the native
libraries, packs them into an AAR, and publishes a Maven repo. That is the
artifact an Android app depends on.

## Add it to an Android app

After the workflow has published a release (tag `v0.1.9` or a manual run
with **publish**), pick one:

### GitHub Packages (recommended once published)

```kotlin
// settings.gradle.kts
dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
        maven {
            url = uri("https://maven.pkg.github.com/OWNER/REPO")
            credentials {
                username = providers.gradleProperty("gpr.user").orElse(System.getenv("GITHUB_ACTOR"))
                password = providers.gradleProperty("gpr.key").orElse(System.getenv("GITHUB_TOKEN"))
            }
        }
    }
}

// app/build.gradle.kts
dependencies {
    implementation("dev.firecrawl:anydoc:0.1.9")
}
```

GitHub Packages needs a token to read, even for public repos. A fine-grained
PAT with `read:packages` is enough. Put it in `~/.gradle/gradle.properties`
as `gpr.user` / `gpr.key`.

### AAR from the GitHub Release or workflow artifact

Download `anydoc-0.1.9.aar` from the release (or the **kotlin-aar** workflow
artifact) and drop it in `app/libs/`:

```kotlin
dependencies {
    implementation(files("libs/anydoc-0.1.9.aar"))
    implementation("net.java.dev.jna:jna:5.19.1@aar")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")
}
```

The Maven publication already pulls JNA and coroutines transitively. The raw
AAR does not, so add them yourself.

The AAR ships `libanydoc_kotlin.so` for `arm64-v8a`, `armeabi-v7a`, and
`x86_64`, aligned for 16 KB pages.

## Usage

```kotlin
import dev.firecrawl.anydoc.Format
import dev.firecrawl.anydoc.toDocument
import dev.firecrawl.anydoc.toMarkdown
import dev.firecrawl.anydoc.toMarkdownBytes
import dev.firecrawl.anydoc.toMarkdownBytesAsync

// From a file path (app-private files only):
val markdown = toMarkdown("/data/data/app/files/report.docx")

// From bytes, with the format detected from the content:
val fromBytes = toMarkdownBytes(bytes)

// Or name it, which signature-less formats (CSV) need:
val fromCsv = toMarkdownBytes(bytes, Format.CSV)

// Or stop at the document model, which also carries embedded assets:
val document = toDocument(bytes)

// Off the main thread (this is CPU work):
val async = toMarkdownBytesAsync(bytes)
```

Android `content://` URIs are not filesystem paths. Use the Android helpers.
They read at most `MAX_DOCUMENT_BYTES` (128 MiB) so a hostile stream cannot
OOM the process before the Rust limits run.

```kotlin
import dev.firecrawl.anydoc.android.toMarkdown
import dev.firecrawl.anydoc.android.toMarkdownAsync

val markdown = contentResolver.toMarkdown(uri)
val async = contentResolver.toMarkdownAsync(uri)
```

## Errors

A conversion throws only when no meaningful Markdown could come out of the
file. Each failure is a subclass of `ConvertException` (UniFFI's name for
the Rust `ConvertError`). `error.code` is the stable name to branch on;
`error.reason` is the full sentence.

```kotlin
try {
    toMarkdown(path)
} catch (error: ConvertException) {
    if (error.code == "encrypted" || error.code == "unsupported") {
        unconverted += path to error.code
        return null
    }
    throw error
}
```

| `code`          | Class                              | Meaning                                                             |
| --------------- | ---------------------------------- | ------------------------------------------------------------------- |
| `unsupported`   | `ConvertException.Unsupported`     | Unknown format, or one that cannot be converted (an image-only PDF) |
| `malformed`     | `ConvertException.Malformed`       | Structurally unusable: no meaningful content could be extracted     |
| `encrypted`     | `ConvertException.Encrypted`       | Encrypted or password-protected                                     |
| `resourceLimit` | `ConvertException.ResourceLimit`   | Crossed a fixed safety limit (decompression, nesting, node count)   |
| `missingPart`   | `ConvertException.MissingPart`     | A part required for any meaningful output is absent                 |
| `io`            | `ConvertException.Io`              | The file could not be read, from `toMarkdown` only                  |

`Malformed.part` and `MissingPart.part` name the package part at fault.
`ResourceLimit.limit` names the limit crossed.

## Format detection

```kotlin
formatFromBytes(bytes)          // Format.DOCX, or null when nothing matches
formatFromExtension(".pptm")    // Format.PPTX
formatFromPath("report.odt")    // Format.ODT
```

CSV has no content signature. Detection returns `null` and the caller has to
name `Format.CSV`.

`toDocument` is unsupported for PDF: PDF conversion emits Markdown directly.
Use `toMarkdownBytes`.

## Build it yourself

From the repository root:

```bash
# Host library + Kotlin sources + JVM tests (no Android SDK needed)
cargo test -p anydoc-kotlin
sh kotlin/scripts/generate-bindings.sh
cd kotlin/android/jvm-test
gradle test -Panydoc.nativeDir="$PWD/../../../target/debug"

# Android .so files (needs NDK r28+ and cargo-ndk)
cargo install cargo-ndk --locked --version 4.1.2
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
sh kotlin/scripts/build-android.sh
cd kotlin/android
gradle :anydoc:assembleRelease :anydoc:publishReleasePublicationToLocalBuildRepository
```

The AAR lands in `kotlin/android/anydoc/build/outputs/aar/`. The local Maven
repo lands in `kotlin/android/build/maven/`.

## License

[MIT](../LICENSE)
