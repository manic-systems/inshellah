use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_temp_dir(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()))
}

#[test]
fn manpage_command_uses_synopsis_name() {
    let root = unique_temp_dir("inshellah-manpage-cli");
    fs::create_dir_all(&root).expect("temp dir");
    let manpage = root.join("btrfs-check.8");
    fs::write(
        &manpage,
        r#".SH SYNOPSIS
btrfs check [options] <device>
.SH OPTIONS
.TP
\fB\-\-repair\fR
try to repair the filesystem
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
        stdout.contains("export extern \"btrfs check\""),
        "stdout = {stdout}"
    );
    assert!(
        !stdout.contains("export extern \"btrfs-check\""),
        "stdout = {stdout}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn manpage_command_strips_git_style_subcommand_prefixes() {
    let root = unique_temp_dir("inshellah-manpage-cli");
    fs::create_dir_all(&root).expect("temp dir");
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

    let _ = fs::remove_dir_all(root);
}

#[test]
fn manpage_command_falls_back_when_synopsis_starts_with_prose() {
    let root = unique_temp_dir("inshellah-manpage-cli");
    fs::create_dir_all(&root).expect("temp dir");
    let manpage = root.join("ld.so.8");
    fs::write(
        &manpage,
        r#".SH SYNOPSIS
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
        stdout.contains("export extern \"ld.so\""),
        "stdout = {stdout}"
    );
    assert!(
        !stdout.contains("export extern \"The\""),
        "stdout = {stdout}"
    );

    let _ = fs::remove_dir_all(root);
}
