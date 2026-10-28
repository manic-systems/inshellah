// SPDX-License-Identifier: EUPL-1.2
//! command line inputs that name files and directories must be validated
//! rather than silently becoming empty or the working directory.
#![cfg(test)]

use std::{
    env,
    fs,
    iter,
    os::unix::fs::PermissionsExt as _,
    path::{
        Path,
        PathBuf,
    },
    process::{
        Command,
        Output,
    },
};

use inshellah::store::filename_of_command;

#[path = "common/temp_dir.rs"] mod temp_dir;

use temp_dir::TempDir;

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
fn empty_directory_entries_are_rejected_except_by_the_completer() {
    let guard = TempDir::new("inshellah-empty-dir-entries");
    let root = guard.path();
    let cache = root.join("cache-dir");
    fs::create_dir_all(&cache).expect("cache dir");
    let cached = cache.join("v1%demo.json");
    let cache_arg = cache.display().to_string();
    let prefix = root.join("prefix");
    fs::create_dir_all(prefix.join("bin")).expect("prefix bin");

    let env_values = [
        format!(":{cache_arg}"),
        format!("{cache_arg}:"),
        format!("{cache_arg}::{cache_arg}"),
        ":".to_owned(),
        String::new(),
    ];
    let mut cases: Vec<(Vec<String>, Option<String>, &str)> = Vec::new();
    for value in env_values {
        cases.push((vec!["purge".into()], Some(value), "INSHELLAH_DIR"));
    }
    for (raw, named) in [
        (vec!["purge", "--dir", ":"], "--dir"),
        (vec!["dump", "--dir", ""], "--dir"),
        (vec!["index", "--dir", ""], "--dir"),
        (vec!["index", "--prefix", ":"], "--prefix"),
        (vec!["diff", "--dir", ":", "ls"], "--dir"),
        (vec!["diff", "--scan", ""], "--scan"),
        (
            vec!["merge", "profile", "--dir", "", "--sources", "sources"],
            "--dir",
        ),
    ] {
        let mut args: Vec<String> = raw.into_iter().map(str::to_owned).collect();
        if args[0] == "index" {
            args.push(prefix.display().to_string());
        }
        cases.push((args, None, named));
    }

    for (args, env_value, named) in cases {
        fs::write(&cached, "{}").expect("write cached");
        let mut command = inshellah(root);
        command.args(&args);
        if let Some(value) = &env_value {
            command.env("INSHELLAH_DIR", value);
        }
        let output = command.output().expect("run inshellah");
        let stderr = stderr_of(&output);
        let context = format!("args = {args:?}, env = {env_value:?}, stderr = {stderr}");
        assert!(!output.status.success(), "{context}");
        assert!(stderr.contains("empty directory entry"), "{context}");
        assert!(stderr.contains(named), "{context}");
        assert!(cached.exists(), "{context}");
    }

    for (args, env_value) in [
        (vec!["complete", "--dir", "", "ls", ""], None),
        (vec!["explain", "ls", ""], Some("")),
    ] {
        let mut command = inshellah(root);
        command.args(&args);
        if let Some(value) = env_value {
            command.env("INSHELLAH_DIR", value);
        }
        let output = command.output().expect("run inshellah");
        assert!(
            output.status.success(),
            "args = {args:?}, stderr = {}",
            stderr_of(&output)
        );
    }
}

#[test]
fn unreadable_command_lists_fail_instead_of_becoming_empty() {
    let guard = TempDir::new("inshellah-unreadable-lists");
    let root = guard.path();
    let prefix = root.join("prefix");
    fs::create_dir_all(prefix.join("bin")).expect("prefix bin");
    let missing = root.join("missing-list");
    for option in ["--ignore", "--help-only"] {
        let output = inshellah(root)
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

struct RestoreMode<'path>(&'path Path);

impl Drop for RestoreMode<'_> {
    fn drop(&mut self) {
        drop(fs::set_permissions(
            self.0,
            fs::Permissions::from_mode(0o755),
        ));
    }
}

#[test]
fn unusable_prefixes_fail_instead_of_indexing_nothing() {
    let guard = TempDir::new("inshellah-unusable-prefixes");
    let root = guard.path();
    let missing = root.join("missing-prefix");
    let man_only = root.join("man-only");
    fs::create_dir_all(man_only.join("share/man")).expect("man only prefix");
    let unlistable = root.join("unlistable");
    fs::create_dir_all(unlistable.join("bin")).expect("unlistable bin");
    let unlistable_bin = unlistable.join("bin");
    let _restore = RestoreMode(&unlistable_bin);
    fs::set_permissions(&unlistable_bin, fs::Permissions::from_mode(0o000)).expect("chmod bin");

    let mut prefixes = vec![(missing, "is not a directory")];
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        prefixes.push((unlistable, "cannot read bin dir"));
    }
    for (prefix, message) in &prefixes {
        for scan in [false, true] {
            let mut command = inshellah(root);
            if scan {
                command.arg("diff").arg("--scan").arg(prefix);
            } else {
                command
                    .arg("index")
                    .arg(prefix)
                    .arg("--dir")
                    .arg(root.join("cache"));
            }
            let output = command.output().expect("run inshellah");
            let stderr = stderr_of(&output);
            let context = format!(
                "prefix = {}, scan = {scan}, stderr = {stderr}",
                prefix.display()
            );
            assert!(!output.status.success(), "{context}");
            assert!(stderr.contains(message), "{context}");
            assert!(stderr.contains(&prefix.display().to_string()), "{context}");
        }
    }

    let output = inshellah(root)
        .args(["diff", "--scan"])
        .arg(&man_only)
        .output()
        .expect("run diff --scan");
    assert!(output.status.success(), "stderr = {}", stderr_of(&output));
}

#[test]
fn manpage_rejects_missing_unreadable_and_non_file_inputs() {
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let guard = TempDir::new("inshellah-manpage");
    let root = guard.path();
    let unreadable = root.join("unreadable.1");
    fs::write(&unreadable, ".TH DEMO 1\n").expect("write manpage");
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o000)).expect("chmod manpage");
    for (input, message) in [
        (root.join("missing"), "cannot read"),
        (unreadable, "cannot read"),
        (root.to_path_buf(), "is not a file"),
    ] {
        let output = inshellah(root)
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

#[test]
fn relative_path_entries_never_resolve_children() {
    let guard = TempDir::new("inshellah-relative-path");
    let root = guard.path();
    let tools = root.join("tools");
    let marker = root.join("planted-ran");
    fs::create_dir_all(&tools).expect("tools dir");
    let planted = tools.join("nu");
    fs::write(
        &planted,
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .expect("write nu");
    fs::set_permissions(&planted, fs::Permissions::from_mode(0o755)).expect("chmod nu");
    let prefix = root.join("prefix");
    fs::create_dir_all(prefix.join("bin")).expect("prefix bin");

    // children run in /tmp, so this entry points at the planted dir from there.
    let relative = format!(
        "{}{}",
        "../".repeat(16),
        tools.strip_prefix("/").expect("absolute").display()
    );
    let path = env::join_paths(
        iter::once(PathBuf::from(relative))
            .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
    )
    .expect("join paths");
    let output = inshellah(root)
        .arg("index")
        .arg(&prefix)
        .arg("--dir")
        .arg(root.join("cache"))
        .env("PATH", path)
        .output()
        .expect("run index");
    assert!(
        !marker.exists(),
        "planted nu ran, stderr = {}",
        stderr_of(&output)
    );
}

fn write_script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().expect("script dir")).expect("script dir");
    fs::write(path, body).expect("write script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod script");
}

const FOO_HELP: &str =
    "#!/bin/sh\nprintf 'Usage: foo [OPTIONS]\\n\\nOptions:\\n  -v, --verbose  be loud\\n'\n";

#[test]
fn relative_prefixes_index_the_commands_they_hold() {
    let guard = TempDir::new("inshellah-relative-prefix");
    let root = guard.path();
    write_script(&root.join("p/bin/foo"), FOO_HELP);

    let output = inshellah(root)
        .args(["index", "./p", "--dir", "out"])
        .output()
        .expect("run index");
    let stderr = stderr_of(&output);
    assert!(output.status.success(), "stderr = {stderr}");
    assert!(
        !stderr.contains("could not be started"),
        "stderr = {stderr}"
    );
    let cached = root
        .join("out")
        .join(format!("{}.json", filename_of_command("foo")));
    assert!(cached.exists(), "stderr = {stderr}");
}

#[test]
fn help_only_sub_sections_are_not_written_from_the_parent_manpage() {
    let guard = TempDir::new("inshellah-help-only-sub-section");
    let root = guard.path();
    let prefix = root.join("p");
    write_script(&prefix.join("bin/app"), "#!/bin/sh\nexit 0\n");
    let man1 = prefix.join("share/man/man1");
    fs::create_dir_all(&man1).expect("man dir");
    fs::write(
        man1.join("app.1"),
        ".TH APP 1\n.SH NAME\napp \\- demo\n.SH SUBCOMMAND\nClone a repository.\nUsage: clone \
         [OPTIONS]\n.TP\n\\fB\\-\\-depth\\fR\nLimit history.\n",
    )
    .expect("manpage");
    let help_only = root.join("help-only");
    fs::write(&help_only, "app clone\n").expect("help-only list");

    let output = inshellah(root)
        .arg("index")
        .arg(&prefix)
        .arg("--dir")
        .arg(root.join("out"))
        .arg("--help-only")
        .arg(&help_only)
        .output()
        .expect("run index");
    assert!(output.status.success(), "stderr = {}", stderr_of(&output));
    let written = |cmd: &str| {
        root.join("out")
            .join(format!("{}.json", filename_of_command(cmd)))
            .exists()
    };
    assert!(written("app"), "stderr = {}", stderr_of(&output));
    assert!(!written("app clone"), "stderr = {}", stderr_of(&output));
}
