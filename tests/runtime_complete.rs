use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use inshellah::parsers::manpage::{ManpageEntry, ManpageResult, ManpageSubcommand, OwnedSwitch};
use inshellah::store::write_result;

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
        cache_dir.join("fakecmd_clone.json").is_file(),
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
