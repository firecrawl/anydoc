use std::path::Path;
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_anydoc-henry-mvp")
}

fn public_pdf() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/pdf/text.pdf")
}

#[test]
fn rejects_unknown_flags_on_stderr() {
    let result = Command::new(binary()).arg("--unknown").output().unwrap();

    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("cli_error: unknown flag"));
}

#[test]
fn rejects_labels_containing_a_slash() {
    let result = Command::new(binary())
        .args([
            "--input",
            public_pdf().to_str().unwrap(),
            "--output",
            "henry-mvp/out/private/test.md",
            "--report",
            "henry-mvp/out/private/test.json",
            "--label",
            "unsafe/label",
        ])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("contain no slash"));
}

#[test]
fn rejects_output_paths_outside_the_private_directory() {
    let temporary = tempfile::tempdir().unwrap();
    let result = Command::new(binary())
        .arg("--input")
        .arg(public_pdf())
        .arg("--output")
        .arg(temporary.path().join("escaped.md"))
        .arg("--report")
        .arg(temporary.path().join("escaped.json"))
        .args(["--label", "public-test-pdf"])
        .output()
        .unwrap();

    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
    assert!(String::from_utf8_lossy(&result.stderr).contains("henry-mvp/out/private"));
}
