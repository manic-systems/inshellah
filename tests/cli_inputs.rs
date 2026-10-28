// SPDX-License-Identifier: EUPL-1.2
//! command line inputs that name files and directories must be validated
//! rather than silently becoming empty or the working directory.
#![cfg(test)]

use std::{
    env,
    fs,
    path::{
        Path,
        PathBuf,
    },
    process::{
        self,
        Command,
        Output,
    },
    sync::atomic::{
        AtomicU64,
        Ordering,
    },
};

fn unique_temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = env::temp_dir().join(format!(
        "{name}-{}-{}",
        process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    drop(fs::remove_dir_all(&dir));
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn inshellah(cwd: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_inshellah"));
    let home = cwd.join("home");
    command
        .current_dir(cwd)
        .env("HOME", &home)
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env_remove("INSHELLAH_DIR");
    command
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn empty_inshellah_dir_entries_are_ignored() {
    let root = unique_temp_dir("inshellah-empty-dir-entries");
    let cache = root.join("cache-dir");
    fs::create_dir_all(&cache).expect("cache dir");
    let cached = cache.join("v1%demo.json");
    let stray = root.join("stray.json");
    for value in [
        format!(":{}", cache.display()),
        format!("{}:", cache.display()),
        format!("::{}", cache.display()),
    ] {
        fs::write(&cached, "{}").expect("write cached");
        fs::write(&stray, "{}").expect("write stray");
        let output = inshellah(&root)
            .env("INSHELLAH_DIR", &value)
            .arg("purge")
            .output()
            .expect("run purge");
        assert!(output.status.success(), "value = {value:?}");
        assert!(!cached.exists(), "value = {value:?}");
        assert!(stray.exists(), "value = {value:?}");
    }
}

#[test]
fn empty_dir_option_entries_are_ignored() {
    let root = unique_temp_dir("inshellah-empty-dir-option");
    let cache = root.join("cache-dir");
    fs::create_dir_all(&cache).expect("cache dir");
    let cached = cache.join("v1%demo.json");
    let stray = root.join("stray.json");
    for value in [
        format!(":{}", cache.display()),
        format!("{}:", cache.display()),
    ] {
        fs::write(&cached, "{}").expect("write cached");
        fs::write(&stray, "{}").expect("write stray");
        let output = inshellah(&root)
            .args(["purge", "--dir", &value])
            .output()
            .expect("run purge");
        assert!(output.status.success(), "value = {value:?}");
        assert!(!cached.exists(), "value = {value:?}");
        assert!(stray.exists(), "value = {value:?}");
    }
}

#[test]
fn unreadable_command_lists_fail_instead_of_becoming_empty() {
    let root = unique_temp_dir("inshellah-unreadable-lists");
    let prefix = root.join("prefix");
    fs::create_dir_all(prefix.join("bin")).expect("prefix bin");
    let missing = root.join("missing-list");
    for option in ["--ignore", "--help-only"] {
        let output = inshellah(&root)
            .arg("index")
            .arg(&prefix)
            .arg("--dir")
            .arg(root.join("cache"))
            .arg(option)
            .arg(&missing)
            .output()
            .expect("run index");
        assert!(!output.status.success(), "{option} accepted a missing file");
        assert!(
            stderr_of(&output).contains(&missing.display().to_string()),
            "{option} stderr = {}",
            stderr_of(&output)
        );
    }
}

#[test]
fn zero_workers_are_rejected() {
    let root = unique_temp_dir("inshellah-zero-workers");
    let prefix = root.join("prefix");
    fs::create_dir_all(prefix.join("bin")).expect("prefix bin");
    let output = inshellah(&root)
        .arg("index")
        .arg(&prefix)
        .args(["--workers", "0"])
        .output()
        .expect("run index");
    assert!(!output.status.success());
}

#[test]
fn manpage_dir_rejects_missing_and_non_directory_inputs() {
    let root = unique_temp_dir("inshellah-manpage-dir");
    let file = root.join("file");
    fs::write(&file, "").expect("write file");
    for (input, message) in [
        (root.join("missing"), "cannot read"),
        (file, "is not a directory"),
    ] {
        let output = inshellah(&root)
            .arg("manpage-dir")
            .arg(&input)
            .output()
            .expect("run manpage-dir");
        assert!(!output.status.success(), "input = {}", input.display());
        assert!(
            stderr_of(&output).contains(message),
            "stderr = {}",
            stderr_of(&output)
        );
    }
}

#[test]
fn manpage_rejects_missing_and_non_file_inputs() {
    let root = unique_temp_dir("inshellah-manpage");
    for (input, message) in [
        (root.join("missing"), "cannot read"),
        (root.clone(), "is not a file"),
    ] {
        let output = inshellah(&root)
            .arg("manpage")
            .arg(&input)
            .output()
            .expect("run manpage");
        assert!(!output.status.success(), "input = {}", input.display());
        assert!(
            stderr_of(&output).contains(message),
            "stderr = {}",
            stderr_of(&output)
        );
    }
}
