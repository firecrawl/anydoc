// Smoke test: the bindings load and every entry point round-trips a fixture.
//
//go:build cgo

package anydoc

import (
	"bytes"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
	"testing"
)

var (
	fixtureDir  = filepath.Join("..", "tests", "fixtures")
	outlineDocx = filepath.Join(fixtureDir, "docx", "handmade-outline.docx")
	richDocx    = filepath.Join(fixtureDir, "docx", "handmade-rich.docx")
	csvSheet    = filepath.Join(fixtureDir, "csv", "sheet.csv")
)

func mustRead(t *testing.T, path string) []byte {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("read %s: %v", path, err)
	}
	return b
}

func TestToMarkdownDetectsFormatFromFileContent(t *testing.T) {
	md, err := ToMarkdown(outlineDocx)
	if err != nil {
		t.Fatalf("ToMarkdown: %v", err)
	}
	if !regexp.MustCompile(`(?m)^# `).MatchString(md) {
		t.Fatalf("expected a top-level heading, got:\n%s", md)
	}
}

func TestToMarkdownBytesConvertsInMemory(t *testing.T) {
	docx := FormatDocx
	md, err := ToMarkdownBytes(mustRead(t, richDocx), &docx)
	if err != nil {
		t.Fatalf("ToMarkdownBytes: %v", err)
	}
	if !strings.Contains(md, "| Quarter | Widgets |") {
		t.Fatalf("expected the table header, got:\n%s", md)
	}
}

func TestToMarkdownBytesDetectsFormatWhenNoneNamed(t *testing.T) {
	md, err := ToMarkdownBytes(mustRead(t, richDocx), nil)
	if err != nil {
		t.Fatalf("ToMarkdownBytes: %v", err)
	}
	if !strings.Contains(md, "| Quarter | Widgets |") {
		t.Fatalf("expected the table header, got:\n%s", md)
	}

	// CSV carries no signature, so it has to be named.
	if _, err := ToMarkdownBytes(mustRead(t, csvSheet), nil); err == nil {
		t.Fatalf("expected an error for unnamed CSV")
	}
	csv := FormatCsv
	mdCsv, err := ToMarkdownBytes(mustRead(t, csvSheet), &csv)
	if err != nil {
		t.Fatalf("ToMarkdownBytes csv: %v", err)
	}
	if !strings.Contains(mdCsv, "| --- |") {
		t.Fatalf("expected a CSV table, got:\n%s", mdCsv)
	}
}

func TestToDocumentExposesDocumentModel(t *testing.T) {
	docx := FormatDocx
	doc, err := ToDocument(mustRead(t, outlineDocx), &docx)
	if err != nil {
		t.Fatalf("ToDocument: %v", err)
	}
	var heading *Block
	for i := range doc.Blocks {
		if doc.Blocks[i].Kind == "heading" {
			heading = &doc.Blocks[i]
			break
		}
	}
	if heading == nil {
		t.Fatalf("no heading block in document: %+v", doc.Blocks)
	}
	if heading.Level == nil || *heading.Level < 1 || *heading.Level > 6 {
		t.Fatalf("heading level out of range: %v", heading.Level)
	}
	if len(heading.Content) == 0 {
		t.Fatalf("heading has no content")
	}
	if heading.Content[0].Kind != "text" || heading.Content[0].Text == nil {
		t.Fatalf("first inline is not text: %+v", heading.Content[0])
	}
	if heading.Content[0].Style == nil {
		t.Fatalf("first inline has no style")
	}
}

func TestToDocumentCarriesEmbeddedAssetsAsBytes(t *testing.T) {
	docx := FormatDocx
	doc, err := ToDocument(mustRead(t, richDocx), &docx)
	if err != nil {
		t.Fatalf("ToDocument: %v", err)
	}
	imageIndex := -1
	for i := range doc.Assets {
		if doc.Assets[i].MediaType == "image/png" {
			imageIndex = i
			break
		}
	}
	if imageIndex < 0 {
		t.Fatalf("no image/png asset in document: %+v", doc.Assets)
	}
	image := doc.Assets[imageIndex]
	if len(image.Data) == 0 {
		t.Fatalf("image asset has empty bytes")
	}
	if image.ID != uint64(imageIndex) {
		t.Fatalf("asset id %d != index %d", image.ID, imageIndex)
	}
}

func TestFormatDetectionReadsContentExtensionAndPath(t *testing.T) {
	f, ok := FormatFromBytes(mustRead(t, richDocx))
	if !ok || f != FormatDocx {
		t.Fatalf("FormatFromBytes(rich) = %q ok=%v, want docx", f, ok)
	}
	// CSV carries no signature: only the extension names it.
	if f, ok := FormatFromBytes(mustRead(t, csvSheet)); ok {
		t.Fatalf("FormatFromBytes(csv) = %q ok=%v, want none", f, ok)
	}
	if f, ok := FormatFromExtension(".pptm"); !ok || f != FormatPptx {
		t.Fatalf("FormatFromExtension(.pptm) = %q ok=%v, want pptx", f, ok)
	}
	if f, ok := FormatFromExtension("xls"); !ok || f != FormatXlsx {
		t.Fatalf("FormatFromExtension(xls) = %q ok=%v, want xlsx", f, ok)
	}
	if f, ok := FormatFromPath("/tmp/report.odt"); !ok || f != FormatOdt {
		t.Fatalf("FormatFromPath(report.odt) = %q ok=%v, want odt", f, ok)
	}
	if f, ok := FormatFromPath("/tmp/report.unknown"); ok {
		t.Fatalf("FormatFromPath(report.unknown) = %q ok=%v, want none", f, ok)
	}
}

func TestConversionErrorsRaiseWithCrateErrorMessage(t *testing.T) {
	docx := FormatDocx
	_, err := ToMarkdownBytes([]byte("not a document"), &docx)
	if err == nil {
		t.Fatalf("expected an error")
	}
	var ce *ConvertError
	if !errors.As(err, &ce) {
		t.Fatalf("expected a *ConvertError, got %T: %v", err, err)
	}
	if !regexp.MustCompile(`malformed|unsupported`).MatchString(ce.Kind) {
		t.Fatalf("error kind %q does not match malformed|unsupported", ce.Kind)
	}
}

func TestUnknownFormatErrors(t *testing.T) {
	bad := Format("nope")
	_, err := ToMarkdownBytes([]byte("x"), &bad)
	if err == nil {
		t.Fatalf("expected an error for unknown format")
	}
	var ce *ConvertError
	if !errors.As(err, &ce) || ce.Kind != "unknown_format" {
		t.Fatalf("expected unknown_format, got %v", err)
	}
}

func TestPDFToDocumentIsUnsupported(t *testing.T) {
	pdfPath := filepath.Join(fixtureDir, "pdf", "text.pdf")
	if _, err := os.Stat(pdfPath); err != nil {
		t.Skipf("no pdf fixture: %v", err)
	}
	pdf := FormatPdf
	_, err := ToDocument(mustRead(t, pdfPath), &pdf)
	if err == nil {
		t.Fatalf("expected an error for PDF ToDocument")
	}
	var ce *ConvertError
	if !errors.As(err, &ce) || ce.Kind != "pdf_no_model" {
		t.Fatalf("expected pdf_no_model, got %v", err)
	}
}

func TestPDFToMarkdownBytesWorks(t *testing.T) {
	pdfPath := filepath.Join(fixtureDir, "pdf", "text.pdf")
	if _, err := os.Stat(pdfPath); err != nil {
		t.Skipf("no pdf fixture: %v", err)
	}
	pdf := FormatPdf
	md, err := ToMarkdownBytes(mustRead(t, pdfPath), &pdf)
	if err != nil {
		t.Fatalf("ToMarkdownBytes pdf: %v", err)
	}
	if len(md) == 0 {
		t.Fatalf("expected non-empty markdown")
	}
}

func TestTableGridDecodesWithSlots(t *testing.T) {
	docx := FormatDocx
	doc, err := ToDocument(mustRead(t, richDocx), &docx)
	if err != nil {
		t.Fatalf("ToDocument: %v", err)
	}
	var table *Table
	for _, b := range doc.Blocks {
		if b.Kind == "table" && b.Table != nil {
			table = b.Table
			break
		}
	}
	if table == nil {
		t.Fatalf("no table in rich docx")
	}
	if table.Kind != "data" {
		t.Fatalf("table kind = %q, want data", table.Kind)
	}
	if table.HeaderRows == 0 {
		t.Fatalf("expected at least one header row")
	}
	if len(table.Grid) == 0 || len(table.Grid[0]) == 0 {
		t.Fatalf("table grid is empty")
	}
	// First row should be all origins.
	for j, slot := range table.Grid[0] {
		if slot.Kind != "origin" || slot.Cell == nil {
			t.Fatalf("grid[0][%d] = %+v, want an origin cell", j, slot)
		}
	}
}

func TestConvertCLI(t *testing.T) {
	binary := filepath.Join(t.TempDir(), "anydoc")
	if runtime.GOOS == "windows" {
		binary += ".exe"
	}
	build := exec.Command("go", "build", "-o", binary, "./cmd/anydoc")
	if output, err := build.CombinedOutput(); err != nil {
		t.Fatalf("build anydoc command: %v\n%s", err, output)
	}

	run := func(args []string, stdin []byte) (stdout, stderr string, code int) {
		cmd := exec.Command(binary, args...)
		cmd.Stdin = bytes.NewReader(stdin)
		var out, err bytes.Buffer
		cmd.Stdout = &out
		cmd.Stderr = &err
		runErr := cmd.Run()
		if runErr == nil {
			return out.String(), err.String(), 0
		}
		exitErr, ok := runErr.(*exec.ExitError)
		if !ok {
			t.Fatalf("run anydoc %v: %v", args, runErr)
		}
		return out.String(), err.String(), exitErr.ExitCode()
	}

	t.Run("converts a file to stdout", func(t *testing.T) {
		stdout, stderr, code := run([]string{outlineDocx}, nil)
		if code != 0 || !regexp.MustCompile(`(?m)^# `).MatchString(stdout) || stderr != "" {
			t.Fatalf("code=%d stdout=%q stderr=%q", code, stdout, stderr)
		}
	})

	t.Run("writes output instead of stdout", func(t *testing.T) {
		output := filepath.Join(t.TempDir(), "outline.md")
		stdout, stderr, code := run([]string{outlineDocx, "-o", output}, nil)
		if code != 0 || stdout != "" || stderr != "" {
			t.Fatalf("code=%d stdout=%q stderr=%q", code, stdout, stderr)
		}
		markdown := string(mustRead(t, output))
		if !regexp.MustCompile(`(?m)^# `).MatchString(markdown) {
			t.Fatalf("output missing heading: %q", markdown)
		}
	})

	t.Run("reads stdin with an explicit format", func(t *testing.T) {
		stdout, stderr, code := run([]string{"-", "--format", "csv"}, mustRead(t, csvSheet))
		if code != 0 || !strings.Contains(stdout, "| --- |") || stderr != "" {
			t.Fatalf("code=%d stdout=%q stderr=%q", code, stdout, stderr)
		}
	})

	t.Run("returns conversion errors", func(t *testing.T) {
		_, stderr, code := run([]string{"no-such-file.docx"}, nil)
		if code != 1 || !strings.HasPrefix(stderr, "anydoc: ") {
			t.Fatalf("code=%d stderr=%q", code, stderr)
		}
	})

	t.Run("returns usage errors", func(t *testing.T) {
		for _, args := range [][]string{
			nil,
			{"--frmat", "csv", csvSheet},
			{"--format", "nope", csvSheet},
			{outlineDocx, richDocx},
		} {
			_, stderr, code := run(args, nil)
			if code != 2 || !strings.HasPrefix(stderr, "anydoc: ") {
				t.Fatalf("args=%v code=%d stderr=%q", args, code, stderr)
			}
		}
	})

	t.Run("prints help and version", func(t *testing.T) {
		stdout, stderr, code := run([]string{"--help"}, nil)
		if code != 0 || !strings.Contains(stdout, "Exit codes:") || stderr != "" {
			t.Fatalf("help: code=%d stdout=%q stderr=%q", code, stdout, stderr)
		}
		stdout, stderr, code = run([]string{"--version"}, nil)
		if code != 0 || !regexp.MustCompile(`^\d+\.\d+\.\d+\n$`).MatchString(stdout) || stderr != "" {
			t.Fatalf("version: code=%d stdout=%q stderr=%q", code, stdout, stderr)
		}
	})
}
