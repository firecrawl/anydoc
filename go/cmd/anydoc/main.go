// Command anydoc converts a document to Markdown:
//
//	anydoc <file> [options]
//	anydoc - [options] < document
package main

import (
	"errors"
	"fmt"
	"io"
	"os"
	"strings"
	"syscall"

	anydoc "github.com/firecrawl/anydoc/go"
)

const formats = "doc, docx, odt, pdf, ppt, pptx, rtf, epub, xlsx, ods, odp, csv"

const help = `anydoc: convert documents to GitHub-Flavored Markdown

Usage:
  anydoc <file> [options]
  anydoc - [options] < document

Converts one document per invocation and writes the Markdown to stdout.
Pass - as the input to read the document from stdin. Never prompts; all
diagnostics go to stderr.

Options:
  -o, --output <path>    Write the Markdown to <path> instead of stdout
  -f, --format <format>  Name the input format instead of detecting it:
                         ` + formats + `
                         (extension aliases like xls, docm, ppsx resolve
                         to these)
  -h, --help             Print this help and exit
  -V, --version          Print the version and exit

The format is detected from the file content; the file extension is the
fallback for signature-less formats (CSV). stdin has no extension, so CSV
input from stdin needs --format csv. Scanned or image-only PDFs need OCR,
which anydoc does not do, and error as unsupported.

Exit codes:
  0  success
  1  the document could not be read or converted
  2  usage error: unknown option, missing input, or invalid --format

Examples:
  anydoc report.docx
  anydoc slides.pptx -o slides.md
  anydoc - --format csv < data.csv
  curl -s https://example.com/paper.pdf | anydoc -
`

func fail(code int, format string, args ...any) {
	fmt.Fprintf(os.Stderr, "anydoc: "+format+"\n", args...)
	os.Exit(code)
}

func main() {
	args := os.Args[1:]
	var input string
	var output string
	var formatName string
	positionalOnly := false
	for i := 0; i < len(args); i++ {
		arg := args[i]
		if positionalOnly || arg == "-" || !strings.HasPrefix(arg, "-") {
			if input != "" {
				fail(2, "one document per invocation: unexpected second input '%s'", arg)
			}
			input = arg
			continue
		}
		if arg == "--" {
			positionalOnly = true
			continue
		}
		value := func() string {
			if i+1 >= len(args) {
				fail(2, "%s requires a value", arg)
			}
			i++
			return args[i]
		}
		switch arg {
		case "-h", "--help":
			fmt.Print(help)
			os.Exit(0)
		case "-V", "--version":
			fmt.Println(anydoc.Version)
			os.Exit(0)
		case "-o", "--output":
			output = value()
		case "-f", "--format":
			formatName = value()
		default:
			if strings.HasPrefix(arg, "--output=") {
				output = strings.TrimPrefix(arg, "--output=")
			} else if strings.HasPrefix(arg, "--format=") {
				formatName = strings.TrimPrefix(arg, "--format=")
			} else {
				fail(2, "unknown option '%s' (see anydoc --help)", arg)
			}
		}
	}
	if input == "" {
		fail(2, "missing input: pass a document path, or - for stdin (see anydoc --help)")
	}

	var format *anydoc.Format
	if formatName != "" {
		f, ok := anydoc.FormatFromExtension(formatName)
		if !ok {
			fail(2, "invalid format '%s'; expected one of: %s", formatName, formats)
		}
		format = &f
	}

	var markdown string
	var err error
	if input == "-" {
		if info, statErr := os.Stdin.Stat(); statErr != nil {
			fail(1, "%v", statErr)
		} else if info.Mode()&os.ModeCharDevice != 0 {
			fail(2, "stdin is a terminal; pipe or redirect a document into anydoc -")
		}
		data, readErr := io.ReadAll(os.Stdin)
		if readErr != nil {
			fail(1, "%v", readErr)
		}
		markdown, err = anydoc.ToMarkdownBytes(data, format)
	} else if format != nil {
		data, readErr := os.ReadFile(input)
		if readErr != nil {
			fail(1, "%v", readErr)
		}
		markdown, err = anydoc.ToMarkdownBytes(data, format)
	} else {
		markdown, err = anydoc.ToMarkdown(input)
	}
	if err != nil {
		fail(1, "%v", err)
	}

	if output != "" {
		if err := os.WriteFile(output, []byte(markdown), 0o644); err != nil {
			fail(1, "%v", err)
		}
		return
	}
	if _, err := fmt.Fprint(os.Stdout, markdown); err != nil && !errors.Is(err, syscall.EPIPE) {
		fail(1, "%v", err)
	}
}
