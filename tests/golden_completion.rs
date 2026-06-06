// SPDX-License-Identifier: EUPL-1.2
//! Golden characterization of `inshellah complete` output.
//!
//! This is a BEHAVIOR-PRESERVATION net for the resolver/completer refactor
//! (Phases 1-4). Unlike `runtime_complete.rs`, which asserts `.contains(...)`,
//! this pins the EXACT JSON the binary emits for a matrix of inputs against a
//! hermetic fixture cache. A refactor that is meant to preserve behavior must
//! leave every golden unchanged; an intentional behavior change is recorded by
//! re-blessing (`INSHELLAH_BLESS=1 cargo test --test golden_completion`).
//!
//! Goldens capture CURRENT behavior, bugs included — that is the point: the
//! refactor must not change behavior silently. Known-pinned oddities are noted
//! in `goldens-NOTES.md` alongside the snapshots.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use inshellah::parsers::manpage::{
    ManpageEntry, ManpageResult, ManpageSubcommand, OwnedParam, OwnedSwitch,
};
use inshellah::store::write_result;

mod common;

// note: ManpageResult.positionals is Vec<(String, Positional)>; the fixtures
// here use no positionals, so an empty Vec suffices.

fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()))
}

fn switch_long(name: &str) -> OwnedSwitch {
    OwnedSwitch::Long(name.to_string())
}

fn switch_both(c: char, long: &str) -> OwnedSwitch {
    OwnedSwitch::Both(c, long.to_string())
}

fn write_stub_executable(bin: &Path, name: &str) {
    let path = bin.join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write stub executable");
    let mut perms = fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod");
}

fn entry(switch: OwnedSwitch, param: Option<&str>, desc: &str) -> ManpageEntry {
    ManpageEntry {
        switch,
        param: param.map(|name| OwnedParam::Mandatory(name.to_string())),
        desc: desc.to_string(),
    }
}

fn sub(name: &str, desc: &str) -> ManpageSubcommand {
    ManpageSubcommand::new(name.to_string(), desc.to_string())
}

/// Build the hermetic fixture: a user cache, a system cache, and a fake `tool`
/// binary on PATH for the on-the-fly resolve case. Returns (user, system, bin).
fn build_fixture(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let user = root.join("user");
    let system = root.join("system");
    let bin = root.join("bin");
    fs::create_dir_all(&user).expect("user dir");
    fs::create_dir_all(&system).expect("system dir");
    fs::create_dir_all(&bin).expect("bin dir");

    // user cache: `tool` with subcommands + flags
    let tool = ManpageResult {
        entries: vec![
            entry(switch_both('v', "verbose"), None, "verbose output"),
            entry(switch_long("color"), Some("WHEN"), "colorize output"),
        ],
        subcommands: vec![
            sub("build", "Compile the package"),
            sub("check", "Analyze the package"),
            sub("clean", "Remove build artifacts"),
        ],
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: "A demo tool".to_string(),
    };
    write_result(&user, "tool", "manpage", &tool).expect("write tool");

    // user cache: `tool build` with its own flags, no subs
    let tool_build = ManpageResult {
        entries: vec![
            entry(switch_long("release"), None, "build in release mode"),
            entry(switch_both('j', "jobs"), Some("N"), "parallel jobs"),
        ],
        subcommands: Vec::new(),
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: "Compile the package".to_string(),
    };
    write_result(&user, "tool build", "manpage", &tool_build).expect("write tool build");

    // system cache: a command present only in the system dir
    let othertool = ManpageResult {
        entries: Vec::new(),
        subcommands: vec![
            sub("start", "Start it"),
            sub("stop", "Stop it"),
            sub("status", "Show status"),
        ],
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: "Other tool".to_string(),
    };
    write_result(&system, "othertool", "manpage", &othertool).expect("write othertool");

    // fake `tool` binary: resolves the uncached `tool extra` subcommand via --help
    let tool_bin = bin.join("tool");
    fs::write(
        &tool_bin,
        r#"#!/bin/sh
if [ "$1" = "extra" ] && { [ "$2" = "--help" ] || [ "$2" = "-h" ]; }; then
  cat <<'EOF'
Usage: tool extra [OPTIONS]

Options:
  --fast        go fast
  -n, --name <NAME>   set a name
EOF
  exit 0
fi
exit 2
"#,
    )
    .expect("write tool bin");
    let mut perms = fs::metadata(&tool_bin).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&tool_bin, perms).expect("chmod");
    write_stub_executable(&bin, "othertool");

    (user, system, bin)
}

fn run_complete(user: &Path, system: &Path, bin: &Path, spans: &[&str]) -> String {
    let dir = format!("{}:{}", user.display(), system.display());
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&dir)
        .arg("--timeout-ms")
        .arg("1000")
        .args(spans)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), old_path.to_string_lossy()),
        )
        // pin completer config so goldens are independent of ambient env
        .env_remove("INSHELLAH_FLAG_TRIGGERS")
        .env_remove("INSHELLAH_FLAG_ON_EMPTY")
        .env_remove("INSHELLAH_MAX_COMPLETIONS")
        .output()
        .expect("run inshellah complete");
    assert!(
        output.status.success(),
        "complete {:?} failed: {}",
        spans,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf8 stdout")
}

/// The case matrix. Each entry pins one (spans -> output) point of the
/// resolver/completer surface that the refactor must preserve.
const CASES: &[(&str, &[&str])] = &[
    // first-level subcommand completion (the depth-guard surface)
    ("subs_empty_token", &["tool", ""]),
    ("subs_prefix_b", &["tool", "b"]),
    ("subs_prefix_c", &["tool", "c"]),
    ("subs_no_match", &["tool", "zzz"]),
    // flag completion
    ("flags_dash", &["tool", "-"]),
    ("flags_long_prefix", &["tool", "--c"]),
    // depth-2: cached subcommand's own flags
    ("sub_build_flags", &["tool", "build", "-"]),
    ("sub_build_empty", &["tool", "build", ""]),
    // on-the-fly resolution of an uncached subcommand via --help
    ("resolve_extra_flags", &["tool", "extra", "-"]),
    // system-dir-only command (user/system precedence + read path)
    ("system_only_subs", &["othertool", "s"]),
    // elevation wrapper transparency
    ("sudo_passthrough", &["sudo", "tool", "b"]),
];

#[test]
fn golden_completion_matrix() {
    let root = unique_temp_dir("inshellah-golden-completion");
    let (user, system, bin) = build_fixture(&root);
    for (name, spans) in CASES {
        // fresh user cache per case so on-the-fly resolves are deterministic
        let case_user = root.join(format!("user-{name}"));
        fs::create_dir_all(&case_user).expect("case user dir");
        for entry in fs::read_dir(&user).expect("read user") {
            let entry = entry.expect("dir entry");
            fs::copy(entry.path(), case_user.join(entry.file_name())).expect("seed case user");
        }
        let actual = run_complete(&case_user, &system, &bin, spans);
        common::check_golden("completion", name, "json", &actual);
    }
    let _ = fs::remove_dir_all(&root);
}
