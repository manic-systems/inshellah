// SPDX-License-Identifier: EUPL-1.2
//! the index BFS recurses into discovered subcommands. `self_listed` drops a
//! child that merely echoes its parent's menu, but a tool that invents fresh
//! subcommand names at every level walks past that guard — so without a hard
//! budget the work is breadth^depth and a single binary can write millions of
//! cache files. this exercises that exact shape and asserts the per-root node
//! budget (`INSHELLAH_MAX_INDEX_NODES`) bounds it.
#![cfg(test)]

use std::{
    fs,
    process::Command,
    time::Instant,
};

#[path = "common/exec.rs"] mod exec;
#[path = "common/temp_dir.rs"] mod temp_dir;

use temp_dir::TempDir;

#[test]
fn index_fanout_is_bounded_by_per_root_budget() {
    let tmp = TempDir::new("inshellah-fanout");
    let root = tmp.path();
    let bindir = root.join("prefix/bin");
    let cache = root.join("cache");
    fs::create_dir_all(&bindir).expect("bindir");

    // pathological tool: prints a *fresh* set of subcommand names keyed by the
    // arg count (depth), so the dispatched leaf never reappears in the child's
    // list (defeats `self_listed`). uncapped, this fans out to breadth^depth.
    // posix sh + /bin/sh shebang so it runs in the nix build sandbox.
    let hydra = bindir.join("hydra");
    exec::write_executable_file(
        &hydra,
        r#"#!/bin/sh
n=0
for a in "$@"; do case "$a" in --help|-h) ;; *) n=$((n+1)) ;; esac; done
echo "Usage: hydra [OPTIONS] COMMAND"
echo ""
echo "Commands:"
i=1
while [ "$i" -le 8 ]; do echo "  lvl${n}sub$i   level $n child $i"; i=$((i+1)); done
"#,
    );

    let nu = bindir.join("nu");
    exec::write_executable_file(&nu, "#!/bin/sh\nprintf '%s\\n' '[\"nu\"]'\n");

    let budget = 40_usize;
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_inshellah"))
        .arg("index")
        .arg(root.join("prefix"))
        .arg("--dir")
        .arg(&cache)
        .arg("--timeout-ms")
        .arg("300")
        .env("INSHELLAH_MAX_INDEX_NODES", budget.to_string())
        .env("PATH", &bindir)
        .output()
        .expect("run inshellah index");
    let elapsed = started.elapsed();

    assert!(
        output.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // the budget bounds enqueued children per root; the root itself is one
    // extra node. anything near it proves the cap engaged — an uncapped run
    // would write thousands (8^10) before the deadline.
    let files = fs::read_dir(&cache)
        .expect("read cache")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|x| x == "json" || x == "nu")
        })
        .count();
    assert!(
        files <= budget + 5,
        "fan-out budget breached: {files} cache files written (budget {budget})"
    );
    assert!(
        files > 1,
        "expected the tree to be partially indexed, got {files}"
    );

    // the cap must announce itself rather than silently truncate.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("budget") && stderr.contains("hydra"),
        "expected a truncation warning on stderr, got: {stderr}"
    );

    // and it must terminate promptly — an uncapped run pegs the cpu until the
    // deadline. generous ceiling to stay reliable under loaded CI.
    assert!(
        elapsed.as_secs() < 20,
        "index took too long ({elapsed:?}); budget may not be engaging"
    );
}
