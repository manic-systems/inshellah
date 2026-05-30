// SPDX-License-Identifier: EUPL-1.2
//! integration tests for the per-command dynamic completer.
//!
//! each case builds a temp dir of fake shell shims, puts it ahead of the
//! host PATH, runs `inshellah complete`, and asserts the JSON. the cache
//! dir is always empty so the dynamic path fires.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("{name}-{}-{}-{}", std::process::id(), nanos, n));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_executable(bin_dir: &Path, name: &str, script: &str) {
    let path = bin_dir.join(name);
    fs::write(&path, script).expect("write fake bin");
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod fake bin");
}

/// fake shims with the minimum output each parser in `src/dynamic.rs`
/// needs to exercise its branches. keep in lockstep with the parsers.
fn install_fakes(bin_dir: &Path) {
    fs::create_dir_all(bin_dir).expect("bin dir");

    write_executable(bin_dir, "nix", FAKE_NIX);
    write_executable(bin_dir, "systemctl", FAKE_SYSTEMCTL);
    write_executable(bin_dir, "kubectl", FAKE_KUBECTL);
    write_executable(bin_dir, "cargo", FAKE_CARGO);
    write_executable(bin_dir, "git", FAKE_GIT);
    write_executable(bin_dir, "jj", FAKE_JJ);
}

const FAKE_NIX: &str = r#"#!/bin/sh
if [ "${1:-}" = eval ]; then
  printf 'raw package description\n'
elif [ "${1:-}" = slow ]; then
  sleep 1
  printf 'header\nslow-package\n'
else
  printf 'header\nbuild\nflake#pkg\n'
fi
"#;

const FAKE_SYSTEMCTL: &str = r#"#!/bin/sh
case "$*" in
  *"g*"*)
    printf 'greetd.service loaded active running Greeter\n'
    ;;
  *)
    printf 'demo.service loaded active running Demo Unit\n'
    ;;
esac
"#;

const FAKE_KUBECTL: &str = r#"#!/bin/sh
printf '%s\n' "$*" > "$KUBECTL_ARGS_FILE"
if [ "${1:-}" = get ] && [ "${2:-}" = deployment ]; then
  printf 'deploy-a\n'
elif [ "${1:-}" = get ]; then
  printf 'pod-a\n'
fi
"#;

const FAKE_CARGO: &str = r#"#!/bin/sh
cat <<'JSON'
{"packages":[{"name":"app-lib","version":"0.1.0","targets":[{"name":"app-lib","kind":["lib"]},{"name":"app-cli","kind":["bin"]},{"name":"app-integration","kind":["test"]}]},{"name":"helper-lib","version":"0.2.0","targets":[{"name":"helper-lib","kind":["lib"]}]}]}
JSON
"#;

const FAKE_GIT: &str = r#"#!/bin/sh
case "${1:-}" in
  remote)
    printf 'origin\nupstream\n'
    ;;
  for-each-ref)
    if [ -n "${INSHELLAH_GIT_ARGS_FILE:-}" ]; then
      printf '%s\n' "$*" > "$INSHELLAH_GIT_ARGS_FILE"
    fi
    case "$*" in
      *"refs/heads refs/remotes refs/tags"*)
        printf 'main\tcommit\tMain branch\norigin/main\tcommit\tRemote main\nv1.0\tcommit\tRelease 1\n'
        ;;
      *"refs/heads"*)
        printf 'main\tMain branch\nfeature\tFeature branch\n'
        ;;
      *"refs/tags"*)
        printf 'v1.0\tRelease 1\nv2.0\tRelease 2\n'
        ;;
    esac
    ;;
  stash)
    if [ "${2:-}" = list ]; then
      printf 'stash@{0}: WIP on main: demo stash\n'
    fi
    ;;
  status)
    printf ' M src/main.rs\n?? new-file.txt\nR  old.txt -> renamed.txt\n'
    ;;
  ls-files)
    printf 'src/main.rs\nREADME.md\n'
    ;;
  config)
    printf 'submodule.demo.path deps/demo\n'
    ;;
  worktree)
    if [ "${2:-}" = list ]; then
      printf 'worktree /repo/linked\n'
    fi
    ;;
esac
"#;

const FAKE_JJ: &str = r#"#!/bin/sh
case "${1:-}" in
  log)
    printf 'k\tworking change\nm\tmain change\n'
    ;;
  bookmark)
    if [ "${2:-}" = list ]; then
      case "$*" in
        *--all-remotes*)
          printf 'main@origin\tmain change\nfeature@upstream\tfeature change\nmain@git\tmain change\n'
          ;;
        *)
          printf 'main\tmain change\nfeature\tfeature change\nfeature\tfeature change\n'
          ;;
      esac
    fi
    ;;
  tag)
    if [ "${2:-}" = list ]; then
      printf 'v1.0\nv2.0\n'
    fi
    ;;
  git)
    if [ "${2:-}" = remote ] && [ "${3:-}" = list ]; then
      printf 'origin https://example.com/repo.git\nupstream https://example.com/upstream.git\n'
    fi
    ;;
  op|operation)
    if [ "${2:-}" = log ]; then
      printf 'abc123\tcheckout working copy\n'
    fi
    ;;
  file)
    if [ "${2:-}" = list ]; then
      printf 'src/main.rs\nREADME.md\n'
    fi
    ;;
  workspace)
    if [ "${2:-}" = list ]; then
      printf 'default\nlinked\n'
    fi
    ;;
esac
"#;

#[derive(Debug, Clone)]
struct Cand {
    value: String,
    description: String,
}

fn parse_output(stdout: &str) -> Option<Vec<Cand>> {
    let trimmed = stdout.trim();
    if trimmed == "null" || trimmed.is_empty() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_str(trimmed).expect("valid JSON");
    let arr = json.as_array()?;
    let out: Vec<Cand> = arr
        .iter()
        .map(|v| Cand {
            value: v
                .get("value")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            description: v
                .get("description")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
        })
        .collect();
    Some(out)
}

struct Harness {
    bin_dir: PathBuf,
    cache_dir: PathBuf,
    /// arg-capture files (KUBECTL_ARGS_FILE, INSHELLAH_GIT_ARGS_FILE) the
    /// fakes write into. wiped between runs.
    aux_files: BTreeMap<String, PathBuf>,
}

impl Harness {
    fn new(name: &str) -> Self {
        let root = unique_temp_dir(name);
        let bin_dir = root.join("bin");
        install_fakes(&bin_dir);
        let cache_dir = root.join("cache");
        fs::create_dir_all(&cache_dir).expect("cache dir");
        let mut aux_files = BTreeMap::new();
        aux_files.insert(
            "KUBECTL_ARGS_FILE".into(),
            root.join("kubectl-args.txt"),
        );
        aux_files.insert(
            "INSHELLAH_GIT_ARGS_FILE".into(),
            root.join("git-args.txt"),
        );
        Harness {
            bin_dir,
            cache_dir,
            aux_files,
        }
    }

    fn run(&self, spans: &[&str], extra_env: &[(&str, &str)]) -> (String, String, bool) {
        for path in self.aux_files.values() {
            let _ = fs::write(path, "");
        }
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_inshellah"));
        cmd.arg("complete").arg("--dir").arg(&self.cache_dir);
        // short --help budget so the static resolve doesn't sit on fakes
        // that don't respond to --help.
        cmd.arg("--timeout-ms").arg("200");
        for s in spans {
            cmd.arg(s);
        }
        // fakes ahead of the host PATH; host kept so coreutils stay
        // reachable from the fake scripts.
        let mut path = OsString::from(&self.bin_dir);
        if let Some(host) = std::env::var_os("PATH") {
            path.push(":");
            path.push(host);
        }
        cmd.env("PATH", path);
        for (k, v) in self.aux_files.iter() {
            cmd.env(k, OsString::from(v));
        }
        cmd.env("INSHELLAH_DYNAMIC_TIMEOUT_MS", "5000");
        for (k, v) in extra_env {
            cmd.env(*k, *v);
        }
        let out = cmd.output().expect("run inshellah");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        (stdout, stderr, out.status.success())
    }

    fn read_aux(&self, key: &str) -> String {
        let p = self.aux_files.get(key).expect("known aux key");
        fs::read_to_string(p).unwrap_or_default()
    }

    fn values(spans: &[Cand]) -> Vec<String> {
        spans.iter().map(|c| c.value.clone()).collect()
    }

    fn descriptions(spans: &[Cand]) -> Vec<String> {
        spans.iter().map(|c| c.description.clone()).collect()
    }
}

// === nix ===

#[test]
fn nix_top_level_completions_use_get_completions_env() {
    let h = Harness::new("dyn-nix-top");
    let (stdout, stderr, ok) = h.run(&["nix", ""], &[]);
    assert!(ok, "stderr={stderr}");
    let cands = parse_output(&stdout).expect("non-null output");
    assert_eq!(cands[0].value, "build");
}

#[test]
fn nix_flake_pkg_gets_enriched_description() {
    let h = Harness::new("dyn-nix-flake");
    let (stdout, _, ok) = h.run(&["nix", "flake#pkg"], &[]);
    assert!(ok);
    let cands = parse_output(&stdout).expect("non-null output");
    assert_eq!(cands[0].description, "raw package description");
}

#[test]
fn nix_slow_completion_times_out_when_budget_is_short() {
    let h = Harness::new("dyn-nix-slow");
    let (stdout, _, _) = h.run(
        &["nix", "slow", ""],
        &[("INSHELLAH_DYNAMIC_TIMEOUT_MS", "50")],
    );
    assert!(parse_output(&stdout).is_none(), "stdout={stdout}");
}

#[test]
fn nix_dynamic_timeout_zero_disables_bound() {
    let h = Harness::new("dyn-nix-no-timeout");
    let (stdout, _, _) = h.run(
        &["nix", "slow", ""],
        &[("INSHELLAH_DYNAMIC_TIMEOUT_MS", "0")],
    );
    let cands = parse_output(&stdout).expect("slow eventually returns");
    assert_eq!(cands[0].value, "slow-package");
}

// === systemctl ===

#[test]
fn systemctl_emits_units_only_for_unit_verbs() {
    let h = Harness::new("dyn-systemctl");
    let (no_units, _, _) = h.run(&["systemctl", "daemon-reload", ""], &[]);
    assert!(
        parse_output(&no_units).is_none(),
        "non-unit verb should not offer units"
    );

    let (units, _, _) = h.run(&["systemctl", "status", ""], &[]);
    let cands = parse_output(&units).expect("units returned");
    assert_eq!(cands[0].value, "demo.service");

    let (prefixed, _, _) = h.run(&["systemctl", "start", "g"], &[]);
    let prefixed = parse_output(&prefixed).expect("prefix-filtered units");
    assert_eq!(prefixed[0].value, "greetd.service");
}

// === kubectl ===

#[test]
fn kubectl_resource_names_complete_with_namespace_preserved() {
    let h = Harness::new("dyn-kubectl");
    let (stdout, _, _) = h.run(&["kubectl", "get", "pods", "-n", "prod", ""], &[]);
    let cands = parse_output(&stdout).expect("pods");
    assert_eq!(cands[0].value, "pod-a");
    let args = h.read_aux("KUBECTL_ARGS_FILE");
    assert!(args.contains("-n prod"), "args captured: {args}");
}

#[test]
fn kubectl_rollout_uses_resource_kind() {
    let h = Harness::new("dyn-kubectl-rollout");
    let (stdout, _, _) = h.run(
        &["kubectl", "rollout", "status", "deployment", ""],
        &[],
    );
    let cands = parse_output(&stdout).expect("rollout target");
    assert_eq!(cands[0].description, "deployment");
}

// === cargo ===

#[test]
fn cargo_p_completes_packages_uniquely() {
    let h = Harness::new("dyn-cargo-p");
    let (stdout, _, _) = h.run(&["cargo", "test", "-p", ""], &[]);
    let cands = parse_output(&stdout).expect("packages");
    assert_eq!(Harness::values(&cands), vec!["app-lib", "helper-lib"]);
}

#[test]
fn cargo_bin_completes_only_bin_targets() {
    let h = Harness::new("dyn-cargo-bin");
    let (stdout, _, _) = h.run(&["cargo", "run", "--bin", ""], &[]);
    let cands = parse_output(&stdout).expect("bin targets");
    assert_eq!(Harness::values(&cands), vec!["app-cli"]);
}

// === git ===

#[test]
fn git_top_level_includes_common_verbs() {
    let h = Harness::new("dyn-git-top");
    let (stdout, _, _) = h.run(&["git", ""], &[]);
    let cands = parse_output(&stdout).expect("top-level verbs");
    let values = Harness::values(&cands);
    assert!(values.iter().any(|v| v == "remote"));
    assert!(values.iter().any(|v| v == "stash"));
}

#[test]
fn git_push_completes_remotes_when_arg_position_is_first() {
    let h = Harness::new("dyn-git-push");
    let (stdout, _, _) = h.run(&["git", "push", ""], &[]);
    let cands = parse_output(&stdout).expect("remotes");
    assert_eq!(Harness::values(&cands), vec!["origin", "upstream"]);
}

#[test]
fn git_remote_subcommand_offers_verbs_then_filters_fuzzily() {
    let h = Harness::new("dyn-git-remote");
    let (verbs, _, _) = h.run(&["git", "remote", ""], &[]);
    let cands = parse_output(&verbs).expect("verbs");
    assert_eq!(
        Harness::values(&cands),
        vec![
            "add",
            "rename",
            "remove",
            "rm",
            "set-head",
            "set-branches",
            "get-url",
            "set-url",
            "show",
            "prune",
            "update",
        ]
    );

    let (prefixed, _, _) = h.run(&["git", "remote", "sho"], &[]);
    let prefixed = parse_output(&prefixed).expect("prefix-filtered");
    assert_eq!(Harness::values(&prefixed), vec!["show"]);

    let (fuzzy, _, _) = h.run(&["git", "remote", "shw"], &[]);
    let fuzzy = parse_output(&fuzzy).expect("fuzzy-filtered");
    assert_eq!(Harness::values(&fuzzy), vec!["show"]);

    let (exact, _, _) = h.run(&["git", "remote", "show"], &[]);
    assert!(
        parse_output(&exact).is_none(),
        "exact dynamic match should disappear"
    );

    let (named, _, _) = h.run(&["git", "remote", "show", ""], &[]);
    let named = parse_output(&named).expect("remote names");
    assert_eq!(Harness::values(&named), vec!["origin", "upstream"]);
}

#[test]
fn git_fetch_then_ref() {
    let h = Harness::new("dyn-git-fetch");
    let (remotes, _, _) = h.run(&["git", "fetch", ""], &[]);
    let cands = parse_output(&remotes).expect("remotes");
    assert_eq!(Harness::values(&cands), vec!["origin", "upstream"]);

    let (refs, _, _) = h.run(&["git", "fetch", "origin", ""], &[]);
    let cands = parse_output(&refs).expect("refs");
    assert!(Harness::values(&cands).iter().any(|v| v == "main"));
}

#[test]
fn git_branch_delete_completes_branches() {
    let h = Harness::new("dyn-git-branch-d");
    let (stdout, _, _) = h.run(&["git", "branch", "-d", ""], &[]);
    let cands = parse_output(&stdout).expect("branches");
    assert_eq!(Harness::values(&cands), vec!["main", "feature"]);
}

#[test]
fn git_tag_delete_completes_tags() {
    let h = Harness::new("dyn-git-tag-d");
    let (stdout, _, _) = h.run(&["git", "tag", "-d", ""], &[]);
    let cands = parse_output(&stdout).expect("tags");
    assert_eq!(Harness::values(&cands), vec!["v1.0", "v2.0"]);
}

#[test]
fn git_stash_apply_completes_stashes() {
    let h = Harness::new("dyn-git-stash");
    let (stdout, _, _) = h.run(&["git", "stash", "apply", ""], &[]);
    let cands = parse_output(&stdout).expect("stashes");
    assert_eq!(Harness::values(&cands), vec!["stash@{0}"]);
}

#[test]
fn git_submodule_update_completes_submodule_paths() {
    let h = Harness::new("dyn-git-submodule");
    let (stdout, _, _) = h.run(&["git", "submodule", "update", ""], &[]);
    let cands = parse_output(&stdout).expect("submodules");
    assert_eq!(Harness::values(&cands), vec!["deps/demo"]);
}

#[test]
fn git_bisect_offers_subcommands_then_refs() {
    let h = Harness::new("dyn-git-bisect");
    let (verbs, _, _) = h.run(&["git", "bisect", ""], &[]);
    let cands = parse_output(&verbs).expect("bisect verbs");
    assert!(Harness::values(&cands).iter().any(|v| v == "good"));

    let (refs, _, _) = h.run(&["git", "bisect", "good", ""], &[]);
    let cands = parse_output(&refs).expect("refs");
    assert!(Harness::values(&cands).iter().any(|v| v == "main"));
}

#[test]
fn git_add_completes_changed_paths_including_renames() {
    let h = Harness::new("dyn-git-add");
    let (stdout, _, _) = h.run(&["git", "add", ""], &[]);
    let cands = parse_output(&stdout).expect("changed paths");
    assert_eq!(
        Harness::values(&cands),
        vec!["src/main.rs", "new-file.txt", "renamed.txt"]
    );
}

#[test]
fn git_rm_completes_tracked_paths() {
    let h = Harness::new("dyn-git-rm");
    let (stdout, _, _) = h.run(&["git", "rm", ""], &[]);
    let cands = parse_output(&stdout).expect("tracked");
    assert_eq!(Harness::values(&cands), vec!["src/main.rs", "README.md"]);
}

#[test]
fn git_worktree_add_first_arg_falls_through_to_files() {
    let h = Harness::new("dyn-git-worktree-add");
    let (stdout, _, _) = h.run(&["git", "worktree", "add", ""], &[]);
    assert!(
        parse_output(&stdout).is_none(),
        "worktree add at first positional should hand off"
    );
}

#[test]
fn git_worktree_remove_completes_existing_worktrees() {
    let h = Harness::new("dyn-git-worktree-rm");
    let (stdout, _, _) = h.run(&["git", "worktree", "remove", ""], &[]);
    let cands = parse_output(&stdout).expect("worktrees");
    assert_eq!(cands[0].value, "/repo/linked");
}

#[test]
fn git_dynamic_limit_zero_omits_count_flag() {
    let h = Harness::new("dyn-git-limit-zero");
    let (_stdout, _, _) = h.run(
        &["git", "fetch", "origin", ""],
        &[("INSHELLAH_DYNAMIC_LIMIT", "0")],
    );
    let captured = h.read_aux("INSHELLAH_GIT_ARGS_FILE");
    assert!(
        !captured.contains("--count"),
        "dynamic limit 0 should omit --count, got: {captured}"
    );
}

// === jj ===

#[test]
fn jj_top_level_includes_common_verbs() {
    let h = Harness::new("dyn-jj-top");
    let (stdout, _, _) = h.run(&["jj", ""], &[]);
    let cands = parse_output(&stdout).expect("top-level verbs");
    let values = Harness::values(&cands);
    assert!(values.iter().any(|v| v == "bookmark"));
    assert!(values.iter().any(|v| v == "git"));
}

#[test]
fn jj_bookmark_delete_dedupes_local_bookmarks() {
    let h = Harness::new("dyn-jj-bookmark-delete");
    let (stdout, _, _) = h.run(&["jj", "bookmark", "delete", ""], &[]);
    let cands = parse_output(&stdout).expect("bookmarks");
    assert_eq!(Harness::values(&cands), vec!["main", "feature"]);
    assert_eq!(
        Harness::descriptions(&cands),
        vec!["main change", "feature change"]
    );
}

#[test]
fn jj_bookmark_track_completes_remote_bookmarks_excluding_at_git() {
    let h = Harness::new("dyn-jj-bookmark-track");
    let (stdout, _, _) = h.run(&["jj", "bookmark", "track", ""], &[]);
    let cands = parse_output(&stdout).expect("remote bookmarks");
    assert_eq!(
        Harness::values(&cands),
        vec!["main@origin", "feature@upstream"]
    );
    assert_eq!(
        Harness::descriptions(&cands),
        vec!["main change", "feature change"]
    );
}

#[test]
fn jj_git_push_bookmark_completes_local_bookmarks() {
    let h = Harness::new("dyn-jj-push-bookmark");
    let (stdout, _, _) = h.run(&["jj", "git", "push", "--bookmark", ""], &[]);
    let cands = parse_output(&stdout).expect("push bookmark names");
    assert_eq!(Harness::values(&cands), vec!["main", "feature"]);

    let (short, _, _) = h.run(&["jj", "git", "push", "-b", ""], &[]);
    let short = parse_output(&short).expect("push bookmark with short flag");
    assert_eq!(Harness::values(&short), vec!["main", "feature"]);
}

#[test]
fn jj_tag_delete_completes_tags() {
    let h = Harness::new("dyn-jj-tag");
    let (stdout, _, _) = h.run(&["jj", "tag", "delete", ""], &[]);
    let cands = parse_output(&stdout).expect("tags");
    assert_eq!(Harness::values(&cands), vec!["v1.0", "v2.0"]);
}

#[test]
fn jj_git_fetch_completes_remotes() {
    let h = Harness::new("dyn-jj-fetch");
    let (stdout, _, _) = h.run(&["jj", "git", "fetch", ""], &[]);
    let cands = parse_output(&stdout).expect("remotes");
    assert_eq!(Harness::values(&cands), vec!["origin", "upstream"]);
}

#[test]
fn jj_git_remote_subcommands_and_removal_target() {
    let h = Harness::new("dyn-jj-remote");
    let (verbs, _, _) = h.run(&["jj", "git", "remote", ""], &[]);
    let cands = parse_output(&verbs).expect("verbs");
    assert_eq!(
        Harness::values(&cands),
        vec!["add", "list", "remove", "rename", "set-url"]
    );

    let (targets, _, _) = h.run(&["jj", "git", "remote", "remove", ""], &[]);
    let targets = parse_output(&targets).expect("targets");
    assert_eq!(Harness::values(&targets), vec!["origin", "upstream"]);
}

#[test]
fn jj_rebase_destination_completes_revisions() {
    let h = Harness::new("dyn-jj-rebase");
    let (stdout, _, _) = h.run(&["jj", "rebase", "-d", ""], &[]);
    let cands = parse_output(&stdout).expect("revisions");
    assert_eq!(
        Harness::values(&cands),
        vec![
            "main",
            "feature",
            "v1.0",
            "v2.0",
            "k",
            "m",
            "main@origin",
            "feature@upstream",
        ]
    );
}

#[test]
fn jj_op_restore_completes_operations() {
    let h = Harness::new("dyn-jj-op");
    let (stdout, _, _) = h.run(&["jj", "op", "restore", ""], &[]);
    let cands = parse_output(&stdout).expect("operations");
    assert_eq!(Harness::values(&cands), vec!["abc123"]);
}

#[test]
fn jj_file_show_completes_repo_files() {
    let h = Harness::new("dyn-jj-file");
    let (stdout, _, _) = h.run(&["jj", "file", "show", ""], &[]);
    let cands = parse_output(&stdout).expect("files");
    assert_eq!(Harness::values(&cands), vec!["src/main.rs", "README.md"]);
}

#[test]
fn jj_workspace_forget_completes_workspaces() {
    let h = Harness::new("dyn-jj-workspace");
    let (stdout, _, _) = h.run(&["jj", "workspace", "forget", ""], &[]);
    let cands = parse_output(&stdout).expect("workspaces");
    assert_eq!(Harness::values(&cands), vec!["default", "linked"]);
}
