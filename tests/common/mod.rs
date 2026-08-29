//! Helpers shared by the integration-test binaries.

use std::path::{Path, PathBuf};

pub fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}
