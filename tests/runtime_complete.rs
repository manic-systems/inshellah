use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use inshellah::parsers::manpage::{
    ManpageEntry, ManpageResult, ManpageSubcommand, OwnedParam, OwnedSwitch,
};
use inshellah::store::{filename_of_command, write_native, write_result};

fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()))
}

fn write_stub_executable(bin_dir: &Path, name: &str) {
    write_executable(bin_dir, name, "#!/bin/sh\nexit 0\n");
}

fn write_executable(bin_dir: &Path, name: &str, script: &str) {
    fs::create_dir_all(bin_dir).expect("bin dir");
    let path = bin_dir.join(name);
    fs::write(&path, script).expect("write executable");
    let mut perms = fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod");
}

fn path_with_bin(bin_dir: &Path) -> String {
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    format!("{}:{}", bin_dir.display(), old_path.to_string_lossy())
}

fn write_fake_nu(bin_dir: &Path, commands_json: &str) {
    write_executable(
        bin_dir,
        "nu",
        &format!(
            r#"#!/bin/sh
printf '%s\n' '{}'
"#,
            commands_json
        ),
    );
}

fn completion_values(stdout: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(stdout);
    let trimmed = text.trim();
    if trimmed == "null" || trimmed.is_empty() {
        return Vec::new();
    }
    serde_json::from_str::<serde_json::Value>(trimmed)
        .expect("completion JSON")
        .as_array()
        .expect("completion array")
        .iter()
        .map(|v| {
            v.get("value")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

#[test]
fn complete_scrapes_missing_subcommand_when_parent_is_cached() {
    let root = unique_temp_dir("inshellah-runtime-complete");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let fakecmd = bin_dir.join("fakecmd");
    fs::write(
        &fakecmd,
        r#"#!/bin/sh
if [ "$1" = "clone" ]; then
  if [ "$2" = "--help" ] || [ "$2" = "-h" ]; then
    cat <<'EOF'
Usage: fakecmd clone [OPTIONS] <repository> [directory]

Options:
  --depth <n>          clone depth
  -v, --verbose        verbose
EOF
    exit 0
  fi
fi

if [ "$1" = "--help" ] || [ "$1" = "-h" ]; then
  cat <<'EOF'
Usage: fakecmd [OPTIONS] COMMAND

Commands:
  clone    Clone a repository

Options:
  -h, --help           show help
EOF
  exit 0
fi

exit 2
"#,
    )
    .expect("write fakecmd");
    let mut perms = fs::metadata(&fakecmd).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fakecmd, perms).expect("chmod");

    let parent = ManpageResult {
        entries: Vec::new(),
        subcommands: vec![ManpageSubcommand::new(
            "clone".to_string(),
            "Clone a repository".to_string(),
        )],
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "fakecmd", "help", &parent).expect("parent cache");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("--timeout-ms")
        .arg("1000")
        .arg("fakecmd")
        .arg("clone")
        .arg("--")
        .env(
            "PATH",
            format!("{}:{}", bin_dir.display(), old_path.to_string_lossy()),
        )
        .output()
        .expect("run inshellah complete");

    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(stdout.contains("--depth"), "stdout = {stdout}");
    assert!(
        cache_dir
            .join(format!("{}.json", filename_of_command("fakecmd clone")))
            .is_file(),
        "subcommand cache was not written"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_descends_through_subcommand_alias() {
    // a typed alias (`cl` for clone) must resolve to the canonical child so its
    // --help is scraped and its flags complete, like typing the full name.
    let root = unique_temp_dir("inshellah-alias-descent");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let fakecmd = bin_dir.join("fakealias");
    fs::write(
        &fakecmd,
        r#"#!/bin/sh
if [ "$1" = "clone" ]; then
  if [ "$2" = "--help" ] || [ "$2" = "-h" ]; then
    cat <<'EOF'
Usage: fakealias clone [OPTIONS] <repository>

Options:
  --depth <n>          clone depth
EOF
    exit 0
  fi
fi
exit 2
"#,
    )
    .expect("write fakealias");
    let mut perms = fs::metadata(&fakecmd).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fakecmd, perms).expect("chmod");

    let parent = ManpageResult {
        entries: Vec::new(),
        subcommands: vec![ManpageSubcommand {
            name: "clone".to_string(),
            desc: "Clone a repository".to_string(),
            aliases: vec!["cl".to_string()],
        }],
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "fakealias", "help", &parent).expect("parent cache");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let run = |sub: &str| {
        Command::new(env!("CARGO_BIN_EXE_inshellah"))
            .args(["complete", "--dir"])
            .arg(&cache_dir)
            .args(["--timeout-ms", "1000", "fakealias", sub, "--"])
            .env(
                "PATH",
                format!("{}:{}", bin_dir.display(), old_path.to_string_lossy()),
            )
            .output()
            .expect("run inshellah complete")
    };

    // the alias descends to clone's scraped flags.
    let via_alias = String::from_utf8(run("cl").stdout).expect("stdout");
    assert!(via_alias.contains("--depth"), "alias path: {via_alias}");

    // and `complete fakealias ''` advertises the alias in the tooltip.
    let listing = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .args(["complete", "--dir"])
        .arg(&cache_dir)
        .args(["--timeout-ms", "1000", "fakealias", ""])
        .env(
            "PATH",
            format!("{}:{}", bin_dir.display(), old_path.to_string_lossy()),
        )
        .output()
        .expect("run listing");
    let listing = String::from_utf8(listing.stdout).expect("stdout");
    assert!(listing.contains("(aka cl)"), "listing: {listing}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_does_not_cache_timed_out_partial_help() {
    let root = unique_temp_dir("inshellah-partial-help-timeout");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let slow = bin_dir.join("slowhelp");
    fs::write(
        &slow,
        r#"#!/bin/sh
if [ "$1" = "--help" ]; then
  printf 'Usage: slowhelp [OPTIONS]\nOptions:\n  --partial partial output\n'
  sleep 1
  exit 0
fi
exit 2
"#,
    )
    .expect("write slowhelp");
    let mut perms = fs::metadata(&slow).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&slow, perms).expect("chmod");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("--timeout-ms")
        .arg("40")
        .arg("slowhelp")
        .arg("--")
        .env(
            "PATH",
            format!("{}:{}", bin_dir.display(), old_path.to_string_lossy()),
        )
        .output()
        .expect("run inshellah complete");

    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert_eq!(stdout.trim(), "null", "stdout = {stdout}");
    assert!(
        !cache_dir.join("slowhelp.json").exists(),
        "partial timed-out help was cached"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_lookup_skips_global_flag_values_before_subcommands() {
    let root = unique_temp_dir("inshellah-global-flag-value");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    write_stub_executable(&bin_dir, "demo");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let parent = ManpageResult {
        entries: vec![ManpageEntry {
            switch: OwnedSwitch::Long("config".to_string()),
            param: Some(OwnedParam::Mandatory("FILE".to_string())),
            desc: "config file".to_string(),
        }],
        subcommands: vec![ManpageSubcommand::new(
            "sub".to_string(),
            "subcommand".to_string(),
        )],
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    let child = ManpageResult {
        entries: vec![ManpageEntry {
            switch: OwnedSwitch::Long("child".to_string()),
            param: None,
            desc: "child flag".to_string(),
        }],
        subcommands: Vec::new(),
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "demo", "help", &parent).expect("parent cache");
    write_result(&cache_dir, "demo sub", "help", &child).expect("child cache");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("demo")
        .arg("--config")
        .arg("cfg")
        .arg("sub")
        .arg("--")
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah complete");

    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(stdout.contains(r#""value":"--child""#), "stdout = {stdout}");
    assert!(
        !stdout.contains(r#""value":"--config""#),
        "resolved parent instead of child: {stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_dynamic_git_uses_explicit_executable_path() {
    let root = unique_temp_dir("inshellah-explicit-git-dynamic");
    let explicit_dir = root.join("explicit");
    let path_dir = root.join("path");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&explicit_dir).expect("explicit dir");
    fs::create_dir_all(&path_dir).expect("path dir");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let explicit_git = explicit_dir.join("git");
    fs::write(
        &explicit_git,
        r#"#!/bin/sh
if [ "$1" = "remote" ]; then
  printf 'explicit-origin\n'
  exit 0
fi
exit 2
"#,
    )
    .expect("write explicit git");
    let path_git = path_dir.join("git");
    fs::write(
        &path_git,
        r#"#!/bin/sh
if [ "$1" = "remote" ]; then
  printf 'path-origin\n'
  exit 0
fi
exit 2
"#,
    )
    .expect("write path git");
    for git in [&explicit_git, &path_git] {
        let mut perms = fs::metadata(git).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(git, perms).expect("chmod");
    }

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("--timeout-ms")
        .arg("100")
        .arg(explicit_git.to_string_lossy().as_ref())
        .arg("push")
        .arg("")
        .env("PATH", &path_dir)
        .output()
        .expect("run inshellah complete");

    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(
        stdout.contains(r#""value":"explicit-origin""#),
        "stdout = {stdout}"
    );
    assert!(
        !stdout.contains("path-origin"),
        "dynamic provider invoked PATH git: {stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_nix_value_position_includes_dynamic_candidates_when_static_matches() {
    let root = unique_temp_dir("inshellah-nix-static-dynamic");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    write_executable(
        &bin_dir,
        "nix",
        r#"#!/bin/sh
if [ -n "${NIX_GET_COMPLETIONS:-}" ]; then
  printf 'header\nnixpkgs#zig\tZig compiler\n'
  exit 0
fi
exit 0
"#,
    );

    let nix_root = ManpageResult {
        entries: Vec::new(),
        subcommands: vec![ManpageSubcommand::new(
            "develop".to_string(),
            "Run a development shell".to_string(),
        )],
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    let nix_develop = ManpageResult {
        entries: Vec::new(),
        subcommands: Vec::new(),
        positional_choices: vec![ManpageSubcommand::new(
            "nixpkgs#zinc".to_string(),
            "cached installable".to_string(),
        )],
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "nix", "help", &nix_root).expect("nix cache");
    write_result(&cache_dir, "nix develop", "help", &nix_develop).expect("nix develop cache");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .args(["complete", "--dir"])
        .arg(&cache_dir)
        .args(["nix", "develop", "nixpkgs#zi"])
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah complete");
    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        completion_values(&output.stdout),
        vec!["nixpkgs#zig".to_string(), "nixpkgs#zinc".to_string()],
        "stdout = {}",
        String::from_utf8_lossy(&output.stdout)
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_long_flag_false_positive_falls_through_to_dynamic() {
    let root = unique_temp_dir("inshellah-long-flag-dynamic");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    write_executable(
        &bin_dir,
        "nix",
        r#"#!/bin/sh
if [ -n "${NIX_GET_COMPLETIONS:-}" ]; then
  printf 'header\n--command\tRun a command\n'
  exit 0
fi
exit 0
"#,
    );

    let nix_develop = ManpageResult {
        entries: vec![ManpageEntry {
            switch: OwnedSwitch::Long("no-write-lock-file".to_string()),
            param: None,
            desc: "do not update the lock file".to_string(),
        }],
        subcommands: Vec::new(),
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "nix develop .", "help", &nix_develop).expect("nix develop cache");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .args(["complete", "--dir"])
        .arg(&cache_dir)
        .args(["nix", "develop", ".", "--c"])
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah complete");
    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(
        stdout.contains(r#""value":"--command"#),
        "stdout = {stdout}"
    );
    assert!(!stdout.contains("no-write-lock-file"), "stdout = {stdout}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_does_not_scan_path_at_command_position() {
    let root = unique_temp_dir("inshellah-command-position-complete");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let fake_git = bin_dir.join("git");
    fs::write(&fake_git, "#!/bin/sh\nexit 0\n").expect("write fake git");
    let mut perms = fs::metadata(&fake_git).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fake_git, perms).expect("chmod");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("gi")
        .env("PATH", &bin_dir)
        .output()
        .expect("run inshellah complete");

    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert_eq!(stdout.trim(), "null", "stdout = {stdout}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_ignores_cached_command_missing_from_path() {
    let root = unique_temp_dir("inshellah-missing-path-command");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let result = ManpageResult {
        entries: Vec::new(),
        subcommands: vec![ManpageSubcommand::new(
            "start".to_string(),
            "start it".to_string(),
        )],
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "vanished", "help", &result).expect("cache");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .args(["vanished", "st"])
        .env("PATH", "")
        .output()
        .expect("run inshellah complete");
    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert_eq!(stdout.trim(), "null", "stdout = {stdout}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_uses_boundary_aware_fuzzy_ranking() {
    let root = unique_temp_dir("inshellah-fuzzy-complete");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    write_stub_executable(&bin_dir, "demo");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let result = ManpageResult {
        entries: Vec::new(),
        subcommands: vec![
            ManpageSubcommand::new("load".to_string(), "load something".to_string()),
            ManpageSubcommand::new("clone".to_string(), "clone something".to_string()),
        ],
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "demo", "help", &result).expect("cache");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("demo")
        .arg("lo")
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah complete");

    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    let load_pos = stdout.find(r#""value":"load""#).unwrap_or(usize::MAX);
    let clone_pos = stdout.find(r#""value":"clone""#).unwrap_or(usize::MAX);
    assert!(
        load_pos < clone_pos,
        "expected boundary match to outrank substring match, stdout = {stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_returns_flags_only_after_hyphen() {
    let root = unique_temp_dir("inshellah-flag-prefix-complete");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    write_stub_executable(&bin_dir, "demo");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let result = ManpageResult {
        entries: vec![ManpageEntry {
            switch: OwnedSwitch::Long("verbose".to_string()),
            param: None,
            desc: "verbose output".to_string(),
        }],
        subcommands: Vec::new(),
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "demo", "help", &result).expect("cache");

    let argument_output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("demo")
        .arg("")
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah complete");
    assert!(
        argument_output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&argument_output.stderr)
    );
    let argument_stdout = String::from_utf8(argument_output.stdout).expect("stdout");
    assert_eq!(argument_stdout.trim(), "null", "stdout = {argument_stdout}");

    let flag_output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("demo")
        .arg("--")
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah complete");
    assert!(
        flag_output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&flag_output.stderr)
    );
    let flag_stdout = String::from_utf8(flag_output.stdout).expect("stdout");
    assert!(
        flag_stdout.contains(r#""value":"--verbose""#),
        "stdout = {flag_stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_does_not_leak_parent_subs_past_uncached_keyword() {
    // `systemctl --user status p` — `systemctl status` isn't cached as its
    // own file (the real systemctl manpage describes all verbs in one
    // place), so `find_result` falls back to the parent `systemctl`. the
    // completer must NOT then offer systemctl's top-level subs filtered by
    // "p" (poweroff, preset, ...) — the user has already typed `status`.
    // it must return null so the downstream dynamic completer (unit names)
    // can take over.
    let root = unique_temp_dir("inshellah-shallow-fallback");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    write_stub_executable(&bin_dir, "fakectl");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let parent = ManpageResult {
        entries: Vec::new(),
        subcommands: vec![
            ManpageSubcommand::new("status".to_string(), "show status".to_string()),
            ManpageSubcommand::new("poweroff".to_string(), "power off".to_string()),
            ManpageSubcommand::new("preset".to_string(), "set preset".to_string()),
        ],
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "fakectl", "manpage", &parent).expect("cache");

    // intermediate flag `--user` plus an uncached deep keyword `status`.
    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("fakectl")
        .arg("--user")
        .arg("status")
        .arg("p")
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah complete");
    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert_eq!(
        stdout.trim(),
        "null",
        "should not surface parent subs past an uncached keyword; stdout = {stdout}"
    );

    // sanity: at the right depth, parent subs are still offered.
    let top_partial = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("fakectl")
        .arg("p")
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah complete");
    let top_stdout = String::from_utf8(top_partial.stdout).expect("stdout");
    assert!(
        top_stdout.contains(r#""value":"poweroff""#) && top_stdout.contains(r#""value":"preset""#),
        "partial at the right depth should still match parent subs; stdout = {top_stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_drops_exact_subcommand_match() {
    // when the typed token exactly equals a cached subcommand, the binary
    // returns null so a downstream dynamic completer (systemctl unit names,
    // git remote names, etc.) can take over instead of echoing the
    // already-typed word back.
    let root = unique_temp_dir("inshellah-exact-subcommand-drop");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    write_stub_executable(&bin_dir, "demo");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let result = ManpageResult {
        entries: Vec::new(),
        subcommands: vec![
            ManpageSubcommand::new("status".to_string(), "show status".to_string()),
            ManpageSubcommand::new("start".to_string(), "start unit".to_string()),
        ],
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "demo", "manpage", &result).expect("cache");

    let exact = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("demo")
        .arg("status")
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah complete");
    assert!(
        exact.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&exact.stderr)
    );
    let exact_stdout = String::from_utf8(exact.stdout).expect("stdout");
    assert_eq!(
        exact_stdout.trim(),
        "null",
        "exact match should hand off; stdout = {exact_stdout}"
    );

    let partial = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("demo")
        .arg("sta")
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah complete");
    let partial_stdout = String::from_utf8(partial.stdout).expect("stdout");
    assert!(
        partial_stdout.contains(r#""value":"status""#)
            && partial_stdout.contains(r#""value":"start""#),
        "partial should still match both; stdout = {partial_stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_resolves_absolute_path_after_elevation_wrapper() {
    let root = unique_temp_dir("inshellah-absolute-elevation-complete");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let fakecmd = bin_dir.join("fakecmd");
    fs::write(
        &fakecmd,
        r#"#!/bin/sh
if [ "$1" = "--help" ] || [ "$1" = "-h" ]; then
  printf '%s\n' 'Usage: fakecmd [OPTIONS]' '' 'Options:' '  --verbose        verbose output'
  exit 0
fi
exit 2
"#,
    )
    .expect("write fakecmd");
    let mut perms = fs::metadata(&fakecmd).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fakecmd, perms).expect("chmod");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("--timeout-ms")
        .arg("1000")
        .arg("sudo")
        .arg(&fakecmd)
        .arg("--")
        .env("PATH", "")
        .output()
        .expect("run inshellah complete");

    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(
        stdout.contains(r#""value":"--verbose""#),
        "stdout = {stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_adb_dynamic_values_use_live_devices_and_packages() {
    let root = unique_temp_dir("inshellah-adb-dynamic-complete");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let adb = bin_dir.join("adb");
    fs::write(
        &adb,
        r#"#!/bin/sh
selector=""
case "$1" in
  -s|--serial|--one-device)
    selector="$2"
    shift 2
    ;;
  -t|--transport-id)
    selector="transport:$2"
    shift 2
    ;;
  --serial=*)
    selector="${1#--serial=}"
    shift
    ;;
  --one-device=*)
    selector="${1#--one-device=}"
    shift
    ;;
  --transport-id=*)
    selector="transport:${1#--transport-id=}"
    shift
    ;;
esac

if [ "$1" = "devices" ] && [ "$2" = "-l" ]; then
  printf '%s\n' 'List of devices attached'
  printf '%s\n' 'emulator-5554	device product:sdk_gphone_x86 model:Pixel_8 device:emu transport_id:1'
  printf '%s\n' 'R58M123456	device product:oriole model:Pixel_6 device:oriole transport_id:2'
  printf '%s\n' 'offline-1	offline transport_id:3'
  exit 0
fi

if [ "$1" = "shell" ] && [ "$2" = "pm" ] && [ "$3" = "list" ] && [ "$4" = "packages" ]; then
  case "$selector" in
    emulator-5554)
      printf '%s\n' 'package:com.example.emu'
      printf '%s\n' 'package:org.example.shared'
      ;;
    transport:2)
      printf '%s\n' 'package:com.example.transport'
      printf '%s\n' 'package:org.example.transport'
      ;;
    *)
      printf '%s\n' 'package:com.default.app'
      printf '%s\n' 'package:/data/app/org.default.path/base.apk=org.default.path'
      ;;
  esac
  exit 0
fi

exit 2
"#,
    )
    .expect("write adb");
    let mut perms = fs::metadata(&adb).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&adb, perms).expect("chmod");

    let run_complete = |args: &[&str]| -> String {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_inshellah"));
        cmd.arg("complete")
            .arg("--dir")
            .arg(&cache_dir)
            .arg("--timeout-ms")
            .arg("1000");
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd
            .env("PATH", &bin_dir)
            .output()
            .expect("run inshellah complete");
        assert!(
            output.status.success(),
            "stderr = {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("stdout")
    };

    let stdout = run_complete(&["adb", "-s", ""]);
    assert!(
        stdout.contains(r#""value":"emulator-5554""#),
        "stdout = {stdout}"
    );
    assert!(
        stdout.contains(r#""description":"device sdk gphone x86 Pixel 8""#),
        "stdout = {stdout}"
    );
    assert!(
        stdout.contains(r#""value":"R58M123456""#),
        "stdout = {stdout}"
    );
    assert!(
        stdout.contains(r#""value":"offline-1""#),
        "stdout = {stdout}"
    );

    let prefixed_stdout = run_complete(&["adb", "--serial=R5"]);
    assert!(
        prefixed_stdout.contains(r#""value":"--serial=R58M123456""#),
        "stdout = {prefixed_stdout}"
    );
    assert!(
        !prefixed_stdout.contains(r#""value":"--serial=emulator-5554""#),
        "stdout = {prefixed_stdout}"
    );

    let one_device_stdout = run_complete(&["adb", "--one-device", ""]);
    assert!(
        one_device_stdout.contains(r#""value":"emulator-5554""#),
        "stdout = {one_device_stdout}"
    );

    let transport_stdout = run_complete(&["adb", "-t", ""]);
    assert!(
        transport_stdout.contains(r#""value":"1""#),
        "stdout = {transport_stdout}"
    );
    assert!(
        transport_stdout.contains(r#""description":"emulator-5554 device sdk gphone x86 Pixel 8""#),
        "stdout = {transport_stdout}"
    );
    assert!(
        transport_stdout.contains(r#""value":"2""#),
        "stdout = {transport_stdout}"
    );

    let transport_prefixed_stdout = run_complete(&["adb", "--transport-id=2"]);
    assert!(
        transport_prefixed_stdout.contains(r#""value":"--transport-id=2""#),
        "stdout = {transport_prefixed_stdout}"
    );
    assert!(
        !transport_prefixed_stdout.contains(r#""value":"--transport-id=1""#),
        "stdout = {transport_prefixed_stdout}"
    );

    let uninstall_stdout = run_complete(&["adb", "uninstall", "org"]);
    assert!(
        uninstall_stdout.contains(r#""value":"org.default.path""#),
        "stdout = {uninstall_stdout}"
    );
    assert!(
        !uninstall_stdout.contains(r#""value":"com.default.app""#),
        "stdout = {uninstall_stdout}"
    );

    let clear_stdout = run_complete(&["adb", "-s", "emulator-5554", "shell", "pm", "clear", ""]);
    assert!(
        clear_stdout.contains(r#""value":"com.example.emu""#),
        "stdout = {clear_stdout}"
    );
    assert!(
        !clear_stdout.contains(r#""value":"com.example.transport""#),
        "stdout = {clear_stdout}"
    );

    let force_stop_stdout = run_complete(&[
        "adb",
        "-t",
        "2",
        "shell",
        "am",
        "force-stop",
        "--user",
        "0",
        "com.",
    ]);
    assert!(
        force_stop_stdout.contains(r#""value":"com.example.transport""#),
        "stdout = {force_stop_stdout}"
    );
    assert!(
        !force_stop_stdout.contains(r#""value":"com.example.emu""#),
        "stdout = {force_stop_stdout}"
    );

    let flag_value_stdout = run_complete(&["adb", "shell", "pm", "enable", "--user", ""]);
    assert_eq!(
        flag_value_stdout.trim(),
        "null",
        "stdout = {flag_value_stdout}"
    );

    let shell_flag_stdout = run_complete(&["adb", "shell", "-s", ""]);
    assert_eq!(
        shell_flag_stdout.trim(),
        "null",
        "stdout = {shell_flag_stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

/// write a single-command cache directory exposing the given long flags,
/// returning the cache dir. callers drive `inshellah complete demo ...`.
fn flag_demo_cache(name: &str, flags: &[&str]) -> PathBuf {
    let root = unique_temp_dir(name);
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    write_stub_executable(&bin_dir, "demo");
    fs::create_dir_all(&cache_dir).expect("cache dir");
    let result = ManpageResult {
        entries: flags
            .iter()
            .map(|f| ManpageEntry {
                switch: OwnedSwitch::Long((*f).to_string()),
                param: None,
                desc: format!("{f} flag"),
            })
            .collect(),
        subcommands: Vec::new(),
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "demo", "help", &result).expect("cache");
    cache_dir
}

fn flag_demo_path(cache_dir: &Path) -> String {
    path_with_bin(&cache_dir.parent().expect("cache parent").join("bin"))
}

#[test]
fn purge_clears_user_cache_but_not_system_dirs() {
    let root = unique_temp_dir("inshellah-purge");
    let user_dir = root.join("cache");
    let system_dir = root.join("system");
    fs::create_dir_all(&user_dir).expect("user dir");
    fs::create_dir_all(&system_dir).expect("system dir");

    let result = ManpageResult {
        entries: Vec::new(),
        subcommands: Vec::new(),
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&user_dir, "usercmd", "help", &result).expect("user cache");
    write_result(&system_dir, "syscmd", "manpage", &result).expect("system cache");
    // a non-cache file in the user dir must survive the purge.
    fs::write(user_dir.join("keep.txt"), "keep me").expect("sentinel");

    let dir_arg = format!("{}:{}", user_dir.display(), system_dir.display());
    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .args(["purge", "--dir", &dir_arg])
        .output()
        .expect("run inshellah purge");
    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // user cache entry gone, non-cache file kept, system dir untouched.
    assert!(
        !user_dir
            .join(format!("{}.json", filename_of_command("usercmd")))
            .exists(),
        "user entry not purged"
    );
    assert!(user_dir.join("keep.txt").exists(), "non-cache file removed");
    assert!(
        system_dir
            .join(format!("{}.json", filename_of_command("syscmd")))
            .exists(),
        "system dir must not be purged"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_dir_overlay_uses_user_before_system() {
    let root = unique_temp_dir("inshellah-dir-overlay");
    let bin_dir = root.join("bin");
    let user_dir = root.join("user");
    let system_dir = root.join("system");
    write_stub_executable(&bin_dir, "demo");
    fs::create_dir_all(&user_dir).expect("user dir");
    fs::create_dir_all(&system_dir).expect("system dir");

    let user = ManpageResult {
        entries: vec![ManpageEntry {
            switch: OwnedSwitch::Long("user".to_string()),
            param: None,
            desc: "user flag".to_string(),
        }],
        subcommands: Vec::new(),
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    let system = ManpageResult {
        entries: vec![ManpageEntry {
            switch: OwnedSwitch::Long("system".to_string()),
            param: None,
            desc: "system flag".to_string(),
        }],
        subcommands: Vec::new(),
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&user_dir, "demo", "help", &user).expect("user cache");
    write_result(&system_dir, "demo", "manpage", &system).expect("system cache");

    let dir_arg = format!("{}:{}", user_dir.display(), system_dir.display());
    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .args(["complete", "--dir", &dir_arg, "demo", "--"])
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah complete");
    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(stdout.contains(r#""value":"--user""#), "stdout = {stdout}");
    assert!(
        !stdout.contains(r#""value":"--system""#),
        "stdout = {stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_empty_native_file_does_not_shadow_json() {
    let root = unique_temp_dir("inshellah-empty-native");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    write_stub_executable(&bin_dir, "demo");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    write_native(
        &cache_dir,
        "demo",
        r#"export extern "other" [
  --native
]
"#,
    )
    .expect("native cache");
    let json = ManpageResult {
        entries: vec![ManpageEntry {
            switch: OwnedSwitch::Long("json".to_string()),
            param: None,
            desc: "json flag".to_string(),
        }],
        subcommands: Vec::new(),
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "demo", "help", &json).expect("json cache");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .args(["demo", "--"])
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah complete");
    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(stdout.contains(r#""value":"--json""#), "stdout = {stdout}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_discovers_underscored_subcommand_from_encoded_cache() {
    let root = unique_temp_dir("inshellah-underscore-subcommand");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    write_stub_executable(&bin_dir, "demo");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let parent = ManpageResult {
        entries: Vec::new(),
        subcommands: Vec::new(),
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    let child = ManpageResult {
        entries: Vec::new(),
        subcommands: Vec::new(),
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: "underscored child".to_string(),
    };
    write_result(&cache_dir, "demo", "manpage", &parent).expect("parent cache");
    write_result(&cache_dir, "demo foo_bar", "manpage", &child).expect("child cache");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .args(["demo", ""])
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah complete");
    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(stdout.contains(r#""value":"foo_bar""#), "stdout = {stdout}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_manpage_resolution_supplements_from_help() {
    let root = unique_temp_dir("inshellah-manpage-help-runtime");
    let bin_dir = root.join("bin");
    let man_dir = root.join("share/man/man1");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&man_dir).expect("man dir");
    write_fake_nu(&bin_dir, r#"["nu"]"#);
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let demo = bin_dir.join("demo");
    fs::write(
        &demo,
        r#"#!/bin/sh
if [ "$1" = "--help" ] || [ "$1" = "-h" ]; then
  cat <<'EOF'
Usage: demo [OPTIONS] <input>

Commands:
  help_sub    from help

Options:
  -v, --verbose              verbose output
EOF
  exit 0
fi
exit 2
"#,
    )
    .expect("write demo");
    let mut perms = fs::metadata(&demo).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&demo, perms).expect("chmod");

    fs::write(
        man_dir.join("demo.1"),
        r#".TH DEMO 1
.SH NAME
demo \- demo command
.SH OPTIONS
.TP
.B \-\-from\-man
man flag
.SH COMMANDS
.TP
.B man-sub
from man
"#,
    )
    .expect("write manpage");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .arg("--timeout-ms")
        .arg("1000")
        .args(["demo", "--"])
        .env(
            "PATH",
            format!("{}:{}", bin_dir.display(), old_path.to_string_lossy()),
        )
        .output()
        .expect("run inshellah complete");
    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(
        stdout.contains(r#""value":"--from-man""#),
        "stdout = {stdout}"
    );
    assert!(
        stdout.contains(r#""value":"--verbose""#),
        "stdout = {stdout}"
    );

    let cache = fs::read_to_string(cache_dir.join(format!("{}.json", filename_of_command("demo"))))
        .expect("cache json");
    let value: serde_json::Value = serde_json::from_str(&cache).expect("cache value");
    assert_eq!(
        value.get("source").and_then(|v| v.as_str()),
        Some("manpage+help")
    );
    assert!(
        value["subcommands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|sc| sc["name"] == "help_sub"),
        "cache = {cache}"
    );
    assert!(
        value["positionals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "input"),
        "cache = {cache}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn index_manpage_results_are_supplemented_from_help() {
    let root = unique_temp_dir("inshellah-manpage-help-index");
    let bin_dir = root.join("bin");
    let man_dir = root.join("share/man/man1");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&man_dir).expect("man dir");
    write_fake_nu(&bin_dir, r#"["nu"]"#);

    let demo = bin_dir.join("demo");
    fs::write(
        &demo,
        r#"#!/bin/sh
if [ "$1" = "--help" ] || [ "$1" = "-h" ]; then
  cat <<'EOF'
Usage: demo [OPTIONS] <input>

Commands:
  from_help    from help

Options:
  -v, --verbose              verbose output
EOF
  exit 0
fi
exit 2
"#,
    )
    .expect("write demo");
    let mut perms = fs::metadata(&demo).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&demo, perms).expect("chmod");

    fs::write(
        man_dir.join("demo.1"),
        r#".TH DEMO 1
.SH NAME
demo \- demo command
.SH OPTIONS
.TP
.B \-\-man\-only\-flag
man flag
"#,
    )
    .expect("write manpage");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("index")
        .arg(&root)
        .arg("--dir")
        .arg(&cache_dir)
        .arg("--timeout-ms")
        .arg("1000")
        .arg("--workers")
        .arg("1")
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah index");
    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cache = fs::read_to_string(cache_dir.join(format!("{}.json", filename_of_command("demo"))))
        .expect("cache json");
    let value: serde_json::Value = serde_json::from_str(&cache).expect("cache value");
    assert_eq!(
        value.get("source").and_then(|v| v.as_str()),
        Some("manpage+help")
    );
    let entries = value["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry["switch"]["name"] == "man-only-flag"),
        "cache = {cache}"
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry["switch"]["name"] == "verbose"),
        "cache = {cache}"
    );
    assert!(
        value["subcommands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|sc| sc["name"] == "from_help"),
        "cache = {cache}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_flag_on_empty_env_surfaces_flags_after_space() {
    let cache_dir = flag_demo_cache("inshellah-flag-on-empty", &["verbose"]);

    // baseline: empty token without the env knob yields no flags.
    let baseline = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .args(["complete", "--dir"])
        .arg(&cache_dir)
        .args(["demo", ""])
        .env("PATH", flag_demo_path(&cache_dir))
        .output()
        .expect("run inshellah complete");
    assert_eq!(
        String::from_utf8_lossy(&baseline.stdout).trim(),
        "null",
        "empty token should not surface flags by default"
    );

    // with INSHELLAH_FLAG_ON_EMPTY, the empty token surfaces flags.
    let opted_in = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .env("INSHELLAH_FLAG_ON_EMPTY", "1")
        .args(["complete", "--dir"])
        .arg(&cache_dir)
        .args(["demo", ""])
        .env("PATH", flag_demo_path(&cache_dir))
        .output()
        .expect("run inshellah complete");
    let stdout = String::from_utf8_lossy(&opted_in.stdout);
    assert!(
        stdout.contains(r#""value":"--verbose""#),
        "stdout = {stdout}"
    );

    let _ = fs::remove_dir_all(cache_dir.parent().unwrap());
}

#[test]
fn complete_custom_trigger_char_surfaces_flags() {
    let cache_dir = flag_demo_cache("inshellah-custom-trigger", &["verbose"]);

    // "+" is not a trigger by default — treated as an argument prefix.
    let baseline = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .args(["complete", "--dir"])
        .arg(&cache_dir)
        .args(["demo", "+v"])
        .env("PATH", flag_demo_path(&cache_dir))
        .output()
        .expect("run inshellah complete");
    assert_eq!(
        String::from_utf8_lossy(&baseline.stdout).trim(),
        "null",
        "'+' should not trigger flags by default"
    );

    // configured as a trigger, "+v" fuzzy-matches the bare flag name.
    let opted_in = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .env("INSHELLAH_FLAG_TRIGGERS", "-+")
        .args(["complete", "--dir"])
        .arg(&cache_dir)
        .args(["demo", "+v"])
        .env("PATH", flag_demo_path(&cache_dir))
        .output()
        .expect("run inshellah complete");
    let stdout = String::from_utf8_lossy(&opted_in.stdout);
    assert!(
        stdout.contains(r#""value":"--verbose""#),
        "stdout = {stdout}"
    );

    let _ = fs::remove_dir_all(cache_dir.parent().unwrap());
}

#[test]
fn complete_rescrapes_stale_user_cache_past_ttl() {
    // a user-cache entry older than INSHELLAH_CACHE_TTL_SECS is re-resolved on
    // the next touch; a fresh-or-disabled ttl serves the stale set unchanged.
    let root = unique_temp_dir("inshellah-stale-rescrape");
    let bin_dir = root.join("bin");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let fakecmd = bin_dir.join("fakerefresh");
    fs::write(
        &fakecmd,
        r#"#!/bin/sh
if [ "$1" = "--help" ] || [ "$1" = "-h" ]; then
  cat <<'EOF'
Usage: fakerefresh [OPTIONS] COMMAND

Commands:
  fresh    A freshly scraped command

Options:
  -h, --help           show help
EOF
  exit 0
fi
exit 2
"#,
    )
    .expect("write fakerefresh");
    let mut perms = fs::metadata(&fakecmd).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&fakecmd, perms).expect("chmod");

    // seed a cache whose subcommand differs from what --help now emits.
    let stale = ManpageResult {
        entries: Vec::new(),
        subcommands: vec![ManpageSubcommand::new(
            "staleonly".to_string(),
            "Only in the stale cache".to_string(),
        )],
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "fakerefresh", "help", &stale).expect("stale cache");
    let cache_file = cache_dir.join(format!("{}.json", filename_of_command("fakerefresh")));

    // backdate the entry well past a 1s ttl (deterministic, no sleep).
    let old = filetime::FileTime::from_unix_time(1_000_000_000, 0);
    filetime::set_file_mtime(&cache_file, old).expect("backdate cache mtime");

    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let path_env = format!("{}:{}", bin_dir.display(), old_path.to_string_lossy());
    let run = |ttl: &str| {
        Command::new(env!("CARGO_BIN_EXE_inshellah"))
            .args(["complete", "--dir"])
            .arg(&cache_dir)
            .args(["--timeout-ms", "1000", "fakerefresh", ""])
            .env("INSHELLAH_CACHE_TTL_SECS", ttl)
            .env("PATH", &path_env)
            .output()
            .expect("run inshellah complete")
    };

    // order matters: the ttl=1 run below rewrites the cache, so the stale-served
    // case must be asserted first.
    // ttl disabled: the stale set is served, the cache untouched.
    let disabled = run("0");
    assert!(
        disabled.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&disabled.stderr)
    );
    let disabled_stdout = String::from_utf8(disabled.stdout).expect("stdout");
    assert!(
        disabled_stdout.contains(r#""value":"staleonly""#),
        "disabled ttl should serve stale set; stdout = {disabled_stdout}"
    );
    assert!(
        fs::read_to_string(&cache_file)
            .expect("cache json")
            .contains("staleonly"),
        "disabled ttl must not rewrite the cache"
    );

    // ttl exceeded: the entry is re-resolved from --help and the cache rewritten.
    let refreshed = run("1");
    assert!(
        refreshed.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&refreshed.stderr)
    );
    let refreshed_stdout = String::from_utf8(refreshed.stdout).expect("stdout");
    assert!(
        refreshed_stdout.contains(r#""value":"fresh""#),
        "stale entry should be rescraped; stdout = {refreshed_stdout}"
    );
    assert!(
        !refreshed_stdout.contains(r#""value":"staleonly""#),
        "stale subcommand should be gone after rescrape; stdout = {refreshed_stdout}"
    );
    let rewritten = fs::read_to_string(&cache_file).expect("cache json");
    assert!(
        rewritten.contains("fresh") && !rewritten.contains("staleonly"),
        "cache should be rewritten with the fresh set; cache = {rewritten}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn index_fails_when_nushell_command_discovery_is_unavailable() {
    let root = unique_temp_dir("inshellah-index-no-nu");
    let prefix = root.join("prefix");
    let bin_dir = prefix.join("bin");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(
        &bin_dir,
        "demo",
        "#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then printf 'Usage: demo\\n'; fi\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("index")
        .arg(&prefix)
        .arg("--dir")
        .arg(&cache_dir)
        .env("PATH", "")
        .output()
        .expect("run inshellah index");
    assert!(
        !output.status.success(),
        "index should fail without nu; stdout = {}, stderr = {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Nushell native command discovery"),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn index_skips_commands_reported_by_nushell_discovery() {
    let root = unique_temp_dir("inshellah-index-nu-skip");
    let prefix = root.join("prefix");
    let bin_dir = prefix.join("bin");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_fake_nu(&bin_dir, r#"["ls","nu"]"#);
    write_executable(
        &bin_dir,
        "ls",
        "#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then printf 'Usage: ls\\nOptions:\\n  --demo demo\\n'; fi\n",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("index")
        .arg(&prefix)
        .arg("--dir")
        .arg(&cache_dir)
        .arg("--timeout-ms")
        .arg("1000")
        .env("PATH", path_with_bin(&bin_dir))
        .output()
        .expect("run inshellah index");
    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !cache_dir
            .join(format!("{}.json", filename_of_command("ls")))
            .exists(),
        "nushell native command should not be indexed"
    );
    assert!(
        cache_dir.join("nushell-native-commands").exists(),
        "discovered command set should be stored"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn complete_max_completions_caps_results() {
    let cache_dir = flag_demo_cache(
        "inshellah-max-completions",
        &["verbose", "version", "verify", "verbatim"],
    );

    let capped = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .env("INSHELLAH_MAX_COMPLETIONS", "2")
        .args(["complete", "--dir"])
        .arg(&cache_dir)
        .args(["demo", "--ver"])
        .env("PATH", flag_demo_path(&cache_dir))
        .output()
        .expect("run inshellah complete");
    let stdout = String::from_utf8_lossy(&capped.stdout);
    let count = stdout.matches(r#""value":"#).count();
    assert_eq!(count, 2, "expected 2 capped candidates, stdout = {stdout}");

    // without the cap, all four matching flags come back.
    let uncapped = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .args(["complete", "--dir"])
        .arg(&cache_dir)
        .args(["demo", "--ver"])
        .env("PATH", flag_demo_path(&cache_dir))
        .output()
        .expect("run inshellah complete");
    let stdout = String::from_utf8_lossy(&uncapped.stdout);
    let count = stdout.matches(r#""value":"#).count();
    assert_eq!(count, 4, "expected 4 candidates, stdout = {stdout}");

    let _ = fs::remove_dir_all(cache_dir.parent().unwrap());
}
