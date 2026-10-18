//! The `anydoc` CLI: convert one document to GitHub-Flavored Markdown.
//!
//! The command itself lives in [`anydoc::cli`] so this `cargo install anydoc`
//! binary and the `firecrawl-anydoc` Python package's console script share one
//! implementation and stay interchangeable: same options, same help, same
//! exit codes as the npm CLI (`node/cli.js`).

use std::process::exit;

fn main() {
    exit(anydoc::cli::run(std::env::args_os().skip(1)));
}
