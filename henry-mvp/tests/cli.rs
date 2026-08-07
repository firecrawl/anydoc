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

#[cfg(unix)]
#[test]
fn successful_cli_secures_private_root_and_outputs() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let private_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("out/private");
    let suffix = format!("permissions-{}", std::process::id());
    let output_path = private_root.join(format!("{suffix}.md"));
    let report_path = private_root.join(format!("{suffix}.json"));
    let result = Command::new(binary())
        .arg("--input")
        .arg(public_pdf())
        .arg("--output")
        .arg(&output_path)
        .arg("--report")
        .arg(&report_path)
        .args(["--label", "permissions-fixture"])
        .output()
        .unwrap();

    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(fs::metadata(&private_root).unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(fs::metadata(&output_path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(&report_path).unwrap().permissions().mode() & 0o777, 0o600);

    fs::remove_file(output_path).unwrap();
    fs::remove_file(report_path).unwrap();
}
