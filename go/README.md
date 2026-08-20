# anydoc-go

[![Go Reference](https://pkg.go.dev/badge/github.com/firecrawl/anydoc/go.svg)](https://pkg.go.dev/github.com/firecrawl/anydoc/go)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

Go bindings for [anydoc](../README.md): convert documents (Word, PowerPoint, Excel, OpenDocument, RTF, EPUB, CSV, and PDF) to GitHub-Flavored Markdown, with full access to the parsed document model and embedded assets.

## Install

```bash
go get github.com/firecrawl/anydoc/go@latest
```

cgo must be enabled (`CGO_ENABLED=1`, the default when a C compiler is present). The module ships Rust static libraries for every supported platform, so no Rust toolchain or download step is required.

Install the CLI with:

```bash
go install github.com/firecrawl/anydoc/go/cmd/anydoc@latest
```

### musl (Alpine)

On musl-based distros (Alpine, Azul), build with `-tags musl` so cgo selects the bundled musl archive:

```bash
go build -tags musl ./...
```

### Building from source

If you have a Rust toolchain and want to replace an archive, build its target
and copy the output to the matching directory. For example, on Apple Silicon:

```bash
cargo build -p anydoc-go --release
cp target/release/libanydoc_go.a go/lib/darwin_arm64/
```

Release archives live under `lib/`: `darwin_<arch>`, `windows_amd64`, and
`linux_<arch>_{gnu,musl}`. They are committed to the module and release CI
rebuilds and compares every archive before publishing.

### Preparing a release

Maintainers run **Prepare Go Archives** from the Actions tab after the binding
code is merged. Its `go-libraries` artifact has the final `go/lib/...` layout.
Commit that payload in a release-prep PR, then create a `go/vX.Y.Z` tag. The
**Release Go Module** workflow rebuilds and compares every bundled archive.

Regenerate the C header after ABI changes:

```bash
sh scripts/gen-headers.sh
```

## Quick start

```go
import anydoc "github.com/firecrawl/anydoc/go"

// From a file path:
markdown, err := anydoc.ToMarkdown("report.docx")

// From bytes, with the format detected from the content:
markdown, err = anydoc.ToMarkdownBytes(bytes, nil)

// Or name it, which signature-less formats (CSV) need:
csv := anydoc.FormatCsv
markdown, err = anydoc.ToMarkdownBytes(csvBytes, &csv)

// Or stop at the document model, which also carries embedded assets:
document, err := anydoc.ToDocument(bytes, nil)
for _, asset := range document.Assets {
    _ = asset.Data // raw image/object bytes
}
```

## Command line

After `go install`, run:

```bash
anydoc report.docx
anydoc - --format csv < data.csv
```

The command follows the Node CLI: it accepts one file path or `-` for stdin,
uses `--format` for signature-less input, writes Markdown to stdout, and sends
only errors to stderr.

## API

### Format detection

```go
anydoc.FormatFromBytes(data)        // (Format, bool)
anydoc.FormatFromExtension(".pptm") // (Format, bool)
anydoc.FormatFromPath("report.odt") // (Format, bool)
```

### Conversion

| Function | Description |
| --- | --- |
| `ToMarkdown(path string) (string, error)` | Convert a file to Markdown; format detected from content, extension as fallback. |
| `ToMarkdownBytes(data []byte, format *Format) (string, error)` | Convert in-memory bytes; `nil` format auto-detects. |
| `ToDocument(data []byte, format *Format) (*Document, error)` | Parse into the document model with embedded assets. Unsupported for PDF. |

### Document model

Concrete structs with a `Kind` tag, identical in shape to the [Node](../node/README.md) and [Python](../python/README.md) bindings:

- `Document` — `Blocks`, `Notes`, `Assets`
- `Block` — `heading`, `paragraph`, `list`, `table`, `block_quote`, `code_block`, `rule`
- `Inline` — `text`, `link`, `image`, `anchor`, `note_ref`, `line_break`
- `Style` — `Bold`, `Italic`, `Strike`, `Code`
- `LinkTarget` — `external`, `relative`, `anchor`
- `ImageSource` — `external`, `asset`, `unavailable`
- `List` / `ListItem` — `marker` (`bullet`, `decimal`, `lower_alpha`, ...), `start`, `items`
- `Table` / `CellSlot` / `Cell` — canonical grid with `origin` and `covered` slots
- `Note` — `footnote`, `endnote`
- `Asset` — `id`, `media_type`, `origin_part`, `data`

Every struct is plain Go, easy to JSON-encode, with no interface dispatch overhead. Variant enums surface as the same lowercase `Kind` strings the other bindings use.

### Errors

Conversion errors are typed `*anydoc.ConvertError` with `Kind` and `Detail` fields. Conversion kinds match the other bindings: `unsupported`, `malformed`, `encrypted`, `resource_limit`, `missing_part`, `io`, `pdf_no_model`. Go validates explicit formats before crossing the ABI and reports `unknown_format` for invalid `Format` values.

```go
var ce *anydoc.ConvertError
if errors.As(err, &ce) {
    fmt.Println(ce.Kind, ce.Detail)
}
```

## Supported platforms

Prebuilt `libanydoc_go.a` assets are published for:

| GOOS | GOARCH |
| --- | --- |
| darwin | arm64, amd64 |
| linux | amd64, arm64 (glibc and musl) |
| windows | amd64 |

Windows cgo requires a C toolchain (MSVC or mingw). The Rust static lib is MSVC by default on Windows; cgo on Windows defaults to MSVC too, so they agree.

## Development

```bash
cargo build -p anydoc-go --release
cp target/release/libanydoc_go.a go/lib/$(go env GOOS)_$(go env GOARCH)/
go test ./...
```

The C ABI surface lives in [`src/lib.rs`](src/lib.rs) with `#[repr(C)]` DTOs in [`src/model.rs`](src/model.rs). The committed header [`include/anydoc.h`](include/anydoc.h) is regenerated by `cargo build` via `cbindgen`; CI verifies it is up to date.

## License

[MIT](../LICENSE)
