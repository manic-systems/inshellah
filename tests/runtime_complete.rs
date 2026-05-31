use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use inshellah::parsers::manpage::{ManpageEntry, ManpageResult, ManpageSubcommand, OwnedSwitch};
use inshellah::store::{filename_of_command, write_native, write_result};

fn unique_temp_dir(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()))
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
        subcommands: vec![ManpageSubcommand {
            name: "clone".to_string(),
            desc: "Clone a repository".to_string(),
        }],
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
fn complete_uses_boundary_aware_fuzzy_ranking() {
    let root = unique_temp_dir("inshellah-fuzzy-complete");
    let cache_dir = root.join("cache");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let result = ManpageResult {
        entries: Vec::new(),
        subcommands: vec![
            ManpageSubcommand {
                name: "load".to_string(),
                desc: "load something".to_string(),
            },
            ManpageSubcommand {
                name: "clone".to_string(),
                desc: "clone something".to_string(),
            },
        ],
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
    let cache_dir = root.join("cache");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let result = ManpageResult {
        entries: vec![ManpageEntry {
            switch: OwnedSwitch::Long("verbose".to_string()),
            param: None,
            desc: "verbose output".to_string(),
        }],
        subcommands: Vec::new(),
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
    let cache_dir = root.join("cache");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let parent = ManpageResult {
        entries: Vec::new(),
        subcommands: vec![
            ManpageSubcommand {
                name: "status".to_string(),
                desc: "show status".to_string(),
            },
            ManpageSubcommand {
                name: "poweroff".to_string(),
                desc: "power off".to_string(),
            },
            ManpageSubcommand {
                name: "preset".to_string(),
                desc: "set preset".to_string(),
            },
        ],
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
        .env("PATH", "")
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
        .env("PATH", "")
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
    let cache_dir = root.join("cache");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let result = ManpageResult {
        entries: Vec::new(),
        subcommands: vec![
            ManpageSubcommand {
                name: "status".to_string(),
                desc: "show status".to_string(),
            },
            ManpageSubcommand {
                name: "start".to_string(),
                desc: "start unit".to_string(),
            },
        ],
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
fn flag_demo_cache(name: &str, flags: &[&str]) -> std::path::PathBuf {
    let root = unique_temp_dir(name);
    let cache_dir = root.join("cache");
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
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "demo", "help", &result).expect("cache");
    cache_dir
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
    let user_dir = root.join("user");
    let system_dir = root.join("system");
    fs::create_dir_all(&user_dir).expect("user dir");
    fs::create_dir_all(&system_dir).expect("system dir");

    let user = ManpageResult {
        entries: vec![ManpageEntry {
            switch: OwnedSwitch::Long("user".to_string()),
            param: None,
            desc: "user flag".to_string(),
        }],
        subcommands: Vec::new(),
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
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&user_dir, "demo", "help", &user).expect("user cache");
    write_result(&system_dir, "demo", "manpage", &system).expect("system cache");

    let dir_arg = format!("{}:{}", user_dir.display(), system_dir.display());
    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .args(["complete", "--dir", &dir_arg, "demo", "--"])
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
    let cache_dir = root.join("cache");
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
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&cache_dir, "demo", "help", &json).expect("json cache");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("complete")
        .arg("--dir")
        .arg(&cache_dir)
        .args(["demo", "--"])
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
    let cache_dir = root.join("cache");
    fs::create_dir_all(&cache_dir).expect("cache dir");

    let parent = ManpageResult {
        entries: Vec::new(),
        subcommands: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    let child = ManpageResult {
        entries: Vec::new(),
        subcommands: Vec::new(),
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
        .output()
        .expect("run inshellah complete");
    let stdout = String::from_utf8_lossy(&uncapped.stdout);
    let count = stdout.matches(r#""value":"#).count();
    assert_eq!(count, 4, "expected 4 candidates, stdout = {stdout}");

    let _ = fs::remove_dir_all(cache_dir.parent().unwrap());
}
