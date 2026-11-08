// SPDX-License-Identifier: EUPL-1.2
//! Writes fake executables without ETXTBSY races. Pulled in with
//! `#[path = "common/exec.rs"] mod exec;` by the tests that plant binaries.

use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::Path,
    process::Command,
};

/// The file is written by a short-lived `sh` rather than this process. Test
/// threads fork concurrently, and a child forked while this thread held the
/// write fd keeps it open until its exec, so a later exec of the file fails
/// with ETXTBSY. A separate writer process is never forked by those threads.
pub fn write_executable_file(path: &Path, body: &str) {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).expect("executable dir");
    }
    let status = Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"printf '%s' "$2" > "$1""#)
        .arg("sh")
        .arg(path)
        .arg(body)
        .status()
        .expect("run writer");
    assert!(status.success(), "write {}", path.display());
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod executable");
}
