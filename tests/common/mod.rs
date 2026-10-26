// SPDX-License-Identifier: EUPL-1.2
//! Shared golden-file helper for the characterization tests. Each integration
//! test file is its own crate, so this is pulled in with `mod common;`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn golden_path(category: &str, name: &str, ext: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/golden/{category}/{name}.{ext}"))
}

/// Compare `actual` to the golden at `tests/golden/<category>/<name>.<ext>`,
/// or (re)write it when `INSHELLAH_BLESS=1` is set. Trailing newlines are
/// normalized so blessed files end in exactly one.
pub fn check_golden(category: &str, name: &str, ext: &str, actual: &str) {
    let path = golden_path(category, name, ext);
    let trimmed = actual.trim_end_matches('\n');
    if env::var_os("INSHELLAH_BLESS").is_some() {
        fs::create_dir_all(path.parent().expect("golden parent")).expect("golden dir");
        fs::write(&path, format!("{trimmed}\n")).expect("write golden");
        return;
    }
    let golden = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden {}; run `INSHELLAH_BLESS=1 cargo test` to create it",
            path.display()
        )
    });
    let expected = golden.trim_end_matches('\n');
    assert_eq!(
        trimmed, expected,
        "golden mismatch for {category}/{name}. \
         If intentional, re-bless with INSHELLAH_BLESS=1 and review the diff."
    );
}
