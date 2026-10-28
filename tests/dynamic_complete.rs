// SPDX-License-Identifier: EUPL-1.2
//! integration tests for the per-command dynamic completer.
//!
//! each case builds a temp dir of fake shell shims, puts it ahead of the
//! host PATH, runs `inshellah complete`, and asserts the JSON. the cache
//! dir is always empty so the dynamic path fires.
#![cfg(test)]

use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{
        Path,
        PathBuf,
    },
    process::{
        self,
        Command,
    },
    sync::atomic::{
        AtomicU64,
        Ordering,
    },
    time::{
        SystemTime,
        UNIX_EPOCH,
    },
};

static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let n = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = env::temp_dir().join(format!("{name}-{}-{}-{}", process::id(), nanos, n));
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

/// fake shims with the minimum output each provider needs to exercise its
/// branches. keep in lockstep with the provider parsers.
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
elif case "$*" in *big#*) true ;; *) false ;; esac; then
  printf 'header\nbig#pkg-a\nbig#pkg-b\nbig#pkg-c\nbig#pkg-d\nbig#pkg-e\nbig#pkg-f\nbig#pkg-g\n'
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
      printf 'argv0=%s\nargs=%s\n' "$0" "$*" > "$INSHELLAH_GIT_ARGS_FILE"
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
[ "${COMPLETE:-}" = fish ] || exit 1
printf '%s\n' "$@" > "$INSHELLAH_JJ_ARGS_FILE"
case "$*" in
  *"log -r main.."*)
    printf 'main..main\tmain change\n\nmain..feature\t feature change\n'
    ;;
esac
"#;

#[derive(Debug, Clone)]
struct Cand {
    value:       String,
    description: String,
}

fn parse_output(stdout: &str) -> Option<Vec<Cand>> {
    let trimmed = stdout.trim();
    if trimmed == "null" || trimmed.is_empty() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_str(trimmed).expect("valid JSON");
    let arr = json.get("completions").unwrap_or(&json).as_array()?;
    let out: Vec<Cand> = arr
        .iter()
        .map(|item| {
            Cand {
                value:       item
                    .get("value")
                    .and_then(|field| field.as_str())
                    .unwrap_or("")
                    .to_owned(),
                description: item
                    .get("description")
                    .and_then(|field| field.as_str())
                    .unwrap_or("")
                    .to_owned(),
            }
        })
        .collect();
    Some(out)
}

struct Harness {
    root:      PathBuf,
    bin_dir:   PathBuf,
    cache_dir: PathBuf,
    /// arg-capture files (`KUBECTL_ARGS_FILE`, `INSHELLAH_GIT_ARGS_FILE`) the
    /// fakes write into. wiped between runs.
    aux_files: BTreeMap<String, PathBuf>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(fs::remove_dir_all(&self.root));
    }
}

impl Harness {
    fn new(name: &str) -> Self {
        let root = unique_temp_dir(name);
        let bin_dir = root.join("bin");
        install_fakes(&bin_dir);
        let cache_dir = root.join("cache");
        fs::create_dir_all(&cache_dir).expect("cache dir");
        let mut aux_files = BTreeMap::new();
        aux_files.insert("KUBECTL_ARGS_FILE".into(), root.join("kubectl-args.txt"));
        aux_files.insert("INSHELLAH_GIT_ARGS_FILE".into(), root.join("git-args.txt"));
        aux_files.insert("INSHELLAH_JJ_ARGS_FILE".into(), root.join("jj-args.txt"));
        Self {
            root,
            bin_dir,
            cache_dir,
            aux_files,
        }
    }

    fn run(&self, spans: &[&str], extra_env: &[(&str, &str)]) -> (String, String, bool) {
        for path in self.aux_files.values() {
            fs::write(path, "").expect("reset aux file");
        }
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_inshellah"));
        cmd.arg("complete").arg("--dir").arg(&self.cache_dir);
        // short --help budget so the static resolve doesn't sit on fakes
        // that don't respond to --help.
        cmd.arg("--timeout-ms").arg("200");
        for span in spans {
            cmd.arg(span);
        }
        // fakes ahead of the host PATH; host kept so coreutils stay
        // reachable from the fake scripts.
        let mut path = OsString::from(&self.bin_dir);
        if let Some(host) = env::var_os("PATH") {
            path.push(":");
            path.push(host);
        }
        cmd.env("PATH", path);
        for (key, aux_path) in &self.aux_files {
            cmd.env(key, OsString::from(aux_path));
        }
        cmd.env("INSHELLAH_DYNAMIC_TIMEOUT_MS", "5000");
        for (key, value) in extra_env {
            cmd.env(*key, *value);
        }
        let out = cmd.output().expect("run inshellah");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        (stdout, stderr, out.status.success())
    }

    fn read_aux(&self, key: &str) -> String {
        let path = self.aux_files.get(key).expect("known aux key");
        fs::read_to_string(path).unwrap_or_default()
    }

    fn values(spans: &[Cand]) -> Vec<String> {
        spans.iter().map(|cand| cand.value.clone()).collect()
    }

    fn descriptions(spans: &[Cand]) -> Vec<String> {
        spans.iter().map(|cand| cand.description.clone()).collect()
    }
}

// === nix ===

#[test]
fn unknown_command_returns_null_for_file_fallback() {
    let harness = Harness::new("dyn-unknown");
    let (stdout, stderr, ok) = harness.run(&["definitely-not-inshellah-dynamic", ""], &[]);
    assert!(ok, "stderr={stderr}");
    assert_eq!(stdout.trim(), "null", "stdout={stdout}");
}

#[test]
fn nix_top_level_completions_use_get_completions_env() {
    let harness = Harness::new("dyn-nix-top");
    let (stdout, stderr, ok) = harness.run(&["nix", ""], &[]);
    assert!(ok, "stderr={stderr}");
    let cands = parse_output(&stdout).expect("non-null output");
    assert_eq!(cands[0].value, "build");
}

#[test]
fn nix_flake_pkg_gets_enriched_description() {
    let harness = Harness::new("dyn-nix-flake");
    let (stdout, _, ok) = harness.run(&["nix", "flake#pkg"], &[]);
    assert!(ok);
    let cands = parse_output(&stdout).expect("non-null output");
    assert_eq!(cands[0].description, "raw package description");
}

#[test]
fn nix_slow_completion_times_out_when_budget_is_short() {
    let harness = Harness::new("dyn-nix-slow");
    let (stdout, ..) = harness.run(&["nix", "slow", ""], &[(
        "INSHELLAH_DYNAMIC_TIMEOUT_MS",
        "50",
    )]);
    assert_eq!(stdout.trim(), "null", "stdout={stdout}");
}

#[test]
fn nix_dynamic_timeout_zero_disables_bound() {
    let harness = Harness::new("dyn-nix-no-timeout");
    let (stdout, ..) = harness.run(&["nix", "slow", ""], &[(
        "INSHELLAH_DYNAMIC_TIMEOUT_MS",
        "0",
    )]);
    let cands = parse_output(&stdout).expect("slow eventually returns");
    assert_eq!(cands[0].value, "slow-package");
}

// === systemctl ===

#[test]
fn systemctl_emits_units_only_for_unit_verbs() {
    let harness = Harness::new("dyn-systemctl");
    let (no_units, ..) = harness.run(&["systemctl", "daemon-reload", ""], &[]);
    assert!(
        parse_output(&no_units).is_none(),
        "non-unit verb should not offer units"
    );

    let (units, ..) = harness.run(&["systemctl", "status", ""], &[]);
    let cands = parse_output(&units).expect("units returned");
    assert_eq!(cands[0].value, "demo.service");

    let (prefixed_out, ..) = harness.run(&["systemctl", "start", "g"], &[]);
    let prefixed = parse_output(&prefixed_out).expect("prefix-filtered units");
    assert_eq!(prefixed[0].value, "greetd.service");
}

// === kubectl ===

#[test]
fn kubectl_resource_names_complete_with_namespace_preserved() {
    let harness = Harness::new("dyn-kubectl");
    let (stdout, ..) = harness.run(&["kubectl", "get", "pods", "-n", "prod", ""], &[]);
    let cands = parse_output(&stdout).expect("pods");
    assert_eq!(cands[0].value, "pod-a");
    let args = harness.read_aux("KUBECTL_ARGS_FILE");
    assert!(args.contains("-n prod"), "args captured: {args}");
}

#[test]
fn kubectl_rollout_uses_resource_kind() {
    let harness = Harness::new("dyn-kubectl-rollout");
    let (stdout, ..) = harness.run(&["kubectl", "rollout", "status", "deployment", ""], &[]);
    let cands = parse_output(&stdout).expect("rollout target");
    assert_eq!(cands[0].description, "deployment");
}

// === cargo ===

#[test]
fn cargo_p_completes_packages_uniquely() {
    let harness = Harness::new("dyn-cargo-p");
    let (stdout, ..) = harness.run(&["cargo", "test", "-p", ""], &[]);
    let cands = parse_output(&stdout).expect("packages");
    assert_eq!(Harness::values(&cands), vec!["app-lib", "helper-lib"]);
}

#[test]
fn cargo_bin_completes_only_bin_targets() {
    let harness = Harness::new("dyn-cargo-bin");
    let (stdout, ..) = harness.run(&["cargo", "run", "--bin", ""], &[]);
    let cands = parse_output(&stdout).expect("bin targets");
    assert_eq!(Harness::values(&cands), vec!["app-cli"]);
}

// === git ===

#[test]
fn git_top_level_includes_common_verbs() {
    let harness = Harness::new("dyn-git-top");
    let (stdout, ..) = harness.run(&["git", ""], &[]);
    let cands = parse_output(&stdout).expect("top-level verbs");
    let values = Harness::values(&cands);
    assert!(values.iter().any(|value| value == "remote"));
    assert!(values.iter().any(|value| value == "stash"));
}

#[test]
fn git_push_completes_remotes_when_arg_position_is_first() {
    let harness = Harness::new("dyn-git-push");
    let (stdout, ..) = harness.run(&["git", "push", ""], &[]);
    let cands = parse_output(&stdout).expect("remotes");
    assert_eq!(Harness::values(&cands), vec!["origin", "upstream"]);
}

#[test]
fn git_remote_subcommand_offers_verbs_then_filters_fuzzily() {
    let harness = Harness::new("dyn-git-remote");
    let (verbs, ..) = harness.run(&["git", "remote", ""], &[]);
    let cands = parse_output(&verbs).expect("verbs");
    assert_eq!(Harness::values(&cands), vec![
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
    ]);

    let (prefixed_out, ..) = harness.run(&["git", "remote", "sho"], &[]);
    let prefixed = parse_output(&prefixed_out).expect("prefix-filtered");
    assert_eq!(Harness::values(&prefixed), vec!["show"]);

    let (fuzzy_out, ..) = harness.run(&["git", "remote", "shw"], &[]);
    let fuzzy = parse_output(&fuzzy_out).expect("fuzzy-filtered");
    assert_eq!(Harness::values(&fuzzy), vec!["show"]);

    let (exact, ..) = harness.run(&["git", "remote", "show"], &[]);
    assert_eq!(exact.trim(), "null", "stdout={exact}");

    let (named_out, ..) = harness.run(&["git", "remote", "show", ""], &[]);
    let named = parse_output(&named_out).expect("remote names");
    assert_eq!(Harness::values(&named), vec!["origin", "upstream"]);
}

#[test]
fn git_fetch_then_ref() {
    let harness = Harness::new("dyn-git-fetch");
    let (remotes, ..) = harness.run(&["git", "fetch", ""], &[]);
    let cands = parse_output(&remotes).expect("remotes");
    assert_eq!(Harness::values(&cands), vec!["origin", "upstream"]);

    let (refs, ..) = harness.run(&["git", "fetch", "origin", ""], &[]);
    let ref_cands = parse_output(&refs).expect("refs");
    assert!(
        Harness::values(&ref_cands)
            .iter()
            .any(|value| value == "main")
    );
}

#[test]
fn git_branch_delete_completes_branches() {
    let harness = Harness::new("dyn-git-branch-d");
    let (stdout, ..) = harness.run(&["git", "branch", "-d", ""], &[]);
    let cands = parse_output(&stdout).expect("branches");
    assert_eq!(Harness::values(&cands), vec!["main", "feature"]);
}

#[test]
fn git_tag_delete_completes_tags() {
    let harness = Harness::new("dyn-git-tag-d");
    let (stdout, ..) = harness.run(&["git", "tag", "-d", ""], &[]);
    let cands = parse_output(&stdout).expect("tags");
    assert_eq!(Harness::values(&cands), vec!["v1.0", "v2.0"]);
}

#[test]
fn git_stash_apply_completes_stashes() {
    let harness = Harness::new("dyn-git-stash");
    let (stdout, ..) = harness.run(&["git", "stash", "apply", ""], &[]);
    let cands = parse_output(&stdout).expect("stashes");
    assert_eq!(Harness::values(&cands), vec!["stash@{0}"]);
}

#[test]
fn git_submodule_update_completes_submodule_paths() {
    let harness = Harness::new("dyn-git-submodule");
    let (stdout, ..) = harness.run(&["git", "submodule", "update", ""], &[]);
    let cands = parse_output(&stdout).expect("submodules");
    assert_eq!(Harness::values(&cands), vec!["deps/demo"]);
}

#[test]
fn git_bisect_offers_subcommands_then_refs() {
    let harness = Harness::new("dyn-git-bisect");
    let (verbs, ..) = harness.run(&["git", "bisect", ""], &[]);
    let cands = parse_output(&verbs).expect("bisect verbs");
    assert!(Harness::values(&cands).iter().any(|value| value == "good"));

    let (refs, ..) = harness.run(&["git", "bisect", "good", ""], &[]);
    let ref_cands = parse_output(&refs).expect("refs");
    assert!(
        Harness::values(&ref_cands)
            .iter()
            .any(|value| value == "main")
    );
}

#[test]
fn git_add_completes_changed_paths_including_renames() {
    let harness = Harness::new("dyn-git-add");
    let (stdout, ..) = harness.run(&["git", "add", ""], &[]);
    let cands = parse_output(&stdout).expect("changed paths");
    assert_eq!(Harness::values(&cands), vec![
        "src/main.rs",
        "new-file.txt",
        "renamed.txt"
    ]);
}

#[test]
fn git_rm_completes_tracked_paths() {
    let harness = Harness::new("dyn-git-rm");
    let (stdout, ..) = harness.run(&["git", "rm", ""], &[]);
    let cands = parse_output(&stdout).expect("tracked");
    assert_eq!(Harness::values(&cands), vec!["src/main.rs", "README.md"]);
}

#[test]
fn git_worktree_add_first_arg_falls_through_to_files() {
    let harness = Harness::new("dyn-git-worktree-add");
    let (stdout, ..) = harness.run(&["git", "worktree", "add", ""], &[]);
    assert_eq!(stdout.trim(), "null", "stdout={stdout}");
}

#[test]
fn git_worktree_remove_completes_existing_worktrees() {
    let harness = Harness::new("dyn-git-worktree-rm");
    let (stdout, ..) = harness.run(&["git", "worktree", "remove", ""], &[]);
    let cands = parse_output(&stdout).expect("worktrees");
    assert_eq!(cands[0].value, "/repo/linked");
}

#[test]
fn git_dynamic_limit_zero_omits_count_flag() {
    let harness = Harness::new("dyn-git-limit-zero");
    let (_stdout, ..) = harness.run(&["git", "fetch", "origin", ""], &[(
        "INSHELLAH_DYNAMIC_LIMIT",
        "0",
    )]);
    let captured = harness.read_aux("INSHELLAH_GIT_ARGS_FILE");
    assert!(
        !captured.contains("--count"),
        "dynamic limit 0 should omit --count, got: {captured}"
    );
}

#[test]
fn git_dynamic_limit_is_passed_to_native_provider() {
    let harness = Harness::new("dyn-git-limit");
    let (_stdout, ..) = harness.run(&["git", "fetch", "origin", ""], &[(
        "INSHELLAH_DYNAMIC_LIMIT",
        "7",
    )]);
    let captured = harness.read_aux("INSHELLAH_GIT_ARGS_FILE");
    assert!(
        captured.contains("--count 7"),
        "dynamic limit should be passed to git, got: {captured}"
    );
}

#[test]
fn git_explicit_path_after_elevation_uses_dynamic_provider() {
    for wrapper in ["sudo", "doas"] {
        let harness = Harness::new(&format!("dyn-git-{wrapper}-path"));
        let git_path = harness.bin_dir.join("git").to_string_lossy().into_owned();
        let (stdout, stderr, ok) = harness.run(&[wrapper, &git_path, "fetch", "origin", ""], &[]);
        assert!(ok, "{wrapper} stderr={stderr}");
        let cands = parse_output(&stdout).expect("refs");
        assert!(Harness::values(&cands).iter().any(|value| value == "main"));

        let captured = harness.read_aux("INSHELLAH_GIT_ARGS_FILE");
        assert!(
            captured.contains(&format!("argv0={git_path}")),
            "{wrapper} should invoke explicit git path, got: {captured}"
        );
    }
}

// === jj ===

#[test]
fn jj_defers_to_native_completer() {
    let harness = Harness::new("dyn-jj-native");
    let (stdout, ..) = harness.run(&["jj", "-R", "repo", "log", "-r", "main.."], &[]);
    let cands = parse_output(&stdout).expect("native candidates");
    assert_eq!(Harness::values(&cands), vec!["main..main", "main..feature"]);
    assert_eq!(Harness::descriptions(&cands), vec![
        "main change",
        "feature change"
    ]);
    assert_eq!(
        harness.read_aux("INSHELLAH_JJ_ARGS_FILE"),
        "--\njj\n-R\nrepo\nlog\n-r\nmain..\n"
    );

    let (empty, ..) = harness.run(&["jj", "nothing", ""], &[]);
    assert!(parse_output(&empty).is_none());
}
