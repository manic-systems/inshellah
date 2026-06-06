// SPDX-License-Identifier: EUPL-1.2
//! Composed end-to-end seam test: real nu tokenizer -> real `inshellah`
//! binary -> fixture cache -> JSON back.
//!
//! This is the ONLY test that realizes the full production path in one process.
//! Every other test cuts the seam: the Rust integration tests hand-write
//! pre-tokenized argv (bypassing nu), and the nu shim tests run against a fake
//! backend that ignores its arguments (bypassing the binary). The bug class
//! that lives *between* nu and the binary — a tokenizer that drops the command
//! head for stub-extern commands — is invisible to both. This test is the
//! guard for it.
//!
//! Skips cleanly (passes as a no-op) when `nu` is not on PATH, so it does not
//! break `cargo test` on machines without nushell; the flake's nushell check
//! runs it where nu is guaranteed.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use inshellah::parsers::manpage::{ManpageResult, ManpageSubcommand};
use inshellah::store::write_result;

fn nu_available() -> bool {
    Command::new("nu")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()))
}

fn write_stub_executable(bin_dir: &Path, name: &str) {
    fs::create_dir_all(bin_dir).expect("bin dir");
    let path = bin_dir.join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write stub executable");
    let mut perms = fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod");
}

fn subs(pairs: &[(&str, &str)]) -> ManpageResult {
    ManpageResult {
        entries: Vec::new(),
        subcommands: pairs
            .iter()
            .map(|(name, desc)| ManpageSubcommand::new(name.to_string(), desc.to_string()))
            .collect(),
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    }
}

#[test]
fn nu_tokenizer_to_real_binary_seam() {
    if !nu_available() {
        eprintln!("skipping seam test: `nu` not found on PATH");
        return;
    }

    let root = unique_temp_dir("inshellah-seam");
    // completer.nu calls `^inshellah complete ...$spans` with no --dir, so it
    // reads the default store at $XDG_CACHE_HOME/inshellah. Seed that.
    let cache = root.join("inshellah");
    fs::create_dir_all(&cache).expect("cache dir");

    // fixture commands are no-op executables
    let fixture_bin = root.join("bin");
    write_stub_executable(&fixture_bin, "demotool");
    write_stub_executable(&fixture_bin, "othertool");
    write_result(
        &cache,
        "demotool",
        "manpage",
        &subs(&[("alpha", "first"), ("beta", "second"), ("gamma", "third")]),
    )
    .expect("write demotool");
    write_result(
        &cache,
        "demotool alpha",
        "manpage",
        &subs(&[("run", "run it"), ("reset", "reset it")]),
    )
    .expect("write demotool alpha");
    write_result(
        &cache,
        "othertool",
        "manpage",
        &subs(&[("start", "start it"), ("stop", "stop it")]),
    )
    .expect("write othertool");

    let bin_dir = Path::new(env!("CARGO_BIN_EXE_inshellah"))
        .parent()
        .expect("binary dir")
        .to_path_buf();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let completer = manifest.join("nix/inshellah-completer.nu");
    let assertions = manifest.join("tests/seam-completer.nu");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let output = Command::new("nu")
        .arg("--no-config-file")
        .arg("-c")
        .arg(format!(
            "source {}; source {}",
            completer.display(),
            assertions.display()
        ))
        .env(
            "PATH",
            format!(
                "{}:{}:{}",
                fixture_bin.display(),
                bin_dir.display(),
                old_path.to_string_lossy()
            ),
        )
        .env("XDG_CACHE_HOME", &root)
        .env_remove("INSHELLAH_FLAG_TRIGGERS")
        .env_remove("INSHELLAH_FLAG_ON_EMPTY")
        .env_remove("INSHELLAH_MAX_COMPLETIONS")
        .output()
        .expect("run nu seam test");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("SEAM OK"),
        "seam test failed.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );

    let _ = fs::remove_dir_all(&root);
}
