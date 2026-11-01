#![cfg(test)]

use std::{
    fs,
    process::Command,
};

#[path = "common/temp_dir.rs"] mod temp_dir;

use temp_dir::TempDir;

#[test]
fn manpage_command_uses_synopsis_name() {
    let tmp = TempDir::new("inshellah-manpage-cli");
    let root = tmp.path();
    let manpage = root.join("btrfs-check.8");
    fs::write(
        &manpage,
        r".SH SYNOPSIS
btrfs check [options] <device>
.SH OPTIONS
.TP
\fB\-\-repair\fR
try to repair the filesystem
",
    )
    .expect("write manpage");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("manpage")
        .arg(&manpage)
        .output()
        .expect("run inshellah manpage");

    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(
        stdout.contains("export extern \"btrfs check\""),
        "stdout = {stdout}"
    );
    assert!(
        !stdout.contains("export extern \"btrfs-check\""),
        "stdout = {stdout}"
    );
}

#[test]
fn manpage_command_strips_git_style_subcommand_prefixes() {
    let tmp = TempDir::new("inshellah-manpage-cli");
    let root = tmp.path();
    let manpage = root.join("git.1");
    fs::write(
        &manpage,
        r#".SH SYNOPSIS
git [--version] [--help] <command> [<args>]
.SH OPTIONS
.TP
\fB\-\-version\fR
show version
.SH "GIT COMMANDS"
.SS "Main porcelain commands"
.PP
.BR git-add (1)
.RS 4
Add file contents to the index.
.RE
"#,
    )
    .expect("write manpage");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("manpage")
        .arg(&manpage)
        .output()
        .expect("run inshellah manpage");

    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(
        stdout.contains("export extern \"git add\""),
        "stdout = {stdout}"
    );
    assert!(
        !stdout.contains("export extern \"git git-add\""),
        "stdout = {stdout}"
    );
}

#[test]
fn manpage_command_falls_back_when_synopsis_starts_with_prose() {
    let tmp = TempDir::new("inshellah-manpage-cli");
    let root = tmp.path();
    let manpage = root.join("ld.so.8");
    fs::write(
        &manpage,
        r".SH SYNOPSIS
The dynamic linker can be run either indirectly by running some
dynamically linked program or shared object
(in which case no command-line options
to the dynamic linker can be passed and, in the ELF case, the dynamic linker
which is stored in the
.B .interp
section of the program is executed) or directly by running:
.P
.I /lib/ld\-linux.so.*
[OPTIONS] [PROGRAM [ARGUMENTS]]
.SH OPTIONS
.TP
.BI \-\-argv0\~ string
Set argv[0] to the value string.
",
    )
    .expect("write manpage");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("manpage")
        .arg(&manpage)
        .output()
        .expect("run inshellah manpage");

    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(
        stdout.contains("export extern \"ld.so\""),
        "stdout = {stdout}"
    );
    assert!(
        !stdout.contains("export extern \"The\""),
        "stdout = {stdout}"
    );
}

#[test]
fn manpage_with_only_synopsis_positionals_prints_nothing() {
    let tmp = TempDir::new("inshellah-manpage-cli");
    let root = tmp.path();
    let manpage = root.join("jq.1");
    fs::write(&manpage, ".SH SYNOPSIS\njq <filter> <file>\n").expect("write manpage");

    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("manpage")
        .arg(&manpage)
        .output()
        .expect("run inshellah manpage");

    let stdout = String::from_utf8(output.stdout).expect("stdout");
    assert!(output.status.success());
    assert_eq!(stdout, "");
}
