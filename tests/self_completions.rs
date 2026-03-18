use std::process::Command;

#[test]
fn inshellah_completions_include_all_subcommands() {
    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("completions")
        .output()
        .expect("run inshellah completions");

    assert!(
        output.status.success(),
        "stderr = {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout");
    for subcommand in [
        "index",
        "manpage",
        "manpage-dir",
        "complete",
        "query",
        "dump",
        "completions",
    ] {
        let extern_name = format!("export extern \"inshellah {subcommand}\"");
        assert!(
            stdout.contains(&extern_name),
            "missing {extern_name}; stdout = {stdout}"
        );
    }
}
