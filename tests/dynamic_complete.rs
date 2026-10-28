// SPDX-License-Identifier: EUPL-1.2
//! integration tests for the per-command dynamic completer.
//!
//! each case builds a temp dir of fake shell shims, puts it ahead of the
//! host PATH, runs `inshellah complete`, and asserts the JSON. the cache
//! dir is empty unless a test seeds it, so the dynamic path fires.
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

use inshellah::{
    parsers::manpage::{
        ManpageEntry,
        ManpageResult,
        ManpageSubcommand,
        OwnedParam,
        OwnedSwitch,
        ParamKind,
    },
    store::write_result,
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
    for name in ["ssh", "scp", "sftp"] {
        write_executable(bin_dir, name, "#!/bin/sh\nexit 0\n");
    }
    write_executable(bin_dir, "kubectl", FAKE_KUBECTL);
    write_executable(bin_dir, "docker", FAKE_DOCKER);
    write_executable(bin_dir, "podman", FAKE_PODMAN);
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
case "$*" in
  *--no-headers*)
    printf 'argv0=%s\nargs=%s\n' "$0" "$*" > "$KUBECTL_ARGS_FILE"
    if echo " $* " | grep -q ' get deployment '; then
      printf 'deploy-a\n'
    elif echo " $* " | grep -q ' --all-namespaces '; then
      printf 'prod pod-a\ndev pod-b\n'
    elif echo " $* " | grep -q ' get '; then
      printf 'pod-a\npod-b\n'
    fi
    ;;
esac
"#;

const FAKE_DOCKER: &str = r#"#!/bin/sh
case " $* " in
  *" ps "*)
    printf 'argv0=%s\nargs=%s\n' "$0" "$*" > "$CONTAINER_ARGS_FILE"
    printf 'docker-web\tnginx:latest\ndocker-db\tpostgres:latest\n'
    ;;
  *" images "*)
    printf 'argv0=%s\nargs=%s\n' "$0" "$*" > "$CONTAINER_ARGS_FILE"
    printf 'docker-alpine:latest\t5MB\ndocker-nginx:latest\t20MB\n'
    ;;
esac
"#;

const FAKE_PODMAN: &str = r#"#!/bin/sh
case " $* " in
  *" ps "*)
    printf 'argv0=%s\nargs=%s\n' "$0" "$*" > "$CONTAINER_ARGS_FILE"
    printf 'podman-web\tquay.io/web:latest\npodman-db\tquay.io/db:latest\n'
    ;;
  *" images "*)
    printf 'argv0=%s\nargs=%s\n' "$0" "$*" > "$CONTAINER_ARGS_FILE"
    printf 'podman-alpine:latest\t6MB\npodman-busybox:latest\t2MB\n'
    ;;
esac
"#;

const FAKE_CARGO: &str = r#"#!/bin/sh
cat <<'JSON'
{"packages":[{"name":"app-lib","version":"0.1.0","targets":[{"name":"app-lib","kind":["lib"]},{"name":"app-cli","kind":["bin"]},{"name":"app-integration","kind":["test"]}]},{"name":"helper-lib","version":"0.2.0","targets":[{"name":"helper-lib","kind":["lib"]}]}]}
JSON
"#;

const FAKE_GIT: &str = r#"#!/bin/sh
if [ -n "${INSHELLAH_GIT_ARGS_FILE:-}" ]; then
  printf 'argv0=%s\nargs=%s\n' "$0" "$*" > "$INSHELLAH_GIT_ARGS_FILE"
fi
while [ "$#" -gt 0 ]; do
  case "${1:-}" in
    -C|-c|--config-env|--git-dir|--namespace|--work-tree)
      shift 2
      ;;
    -*)
      shift
      ;;
    *)
      break
      ;;
  esac
done
case "${1:-}" in
  remote)
    printf 'origin\nupstream\n'
    ;;
  for-each-ref)
    case "$*" in
      *"refs/remotes/origin"*)
        printf 'HEAD\tHead\nmain\tRemote main\nrelease/1\tRemote release\n'
        ;;
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
  rev-parse)
    printf '/repo\n%s\n' "$(cat "$(dirname "$0")/show-prefix" 2>/dev/null)"
    ;;
  status)
    printf ' M src/main.rs\0?? new-file.txt\0R  renamed.txt\0old.txt\0?? spaced name.txt\0'
    ;;
  ls-files)
    if [ "${2:-}" = -z ]; then
      printf 'src/main.rs\0README.md\0caf\303\251.txt\0'
    else
      printf 'src/main.rs\nREADME.md\n"caf\\303\\251.txt"\n'
    fi
    ;;
  config)
    printf 'submodule.demo.path\ndeps/demo\0submodule.spaced.path\ndeps/with space\0'
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
        aux_files.insert(
            "CONTAINER_ARGS_FILE".into(),
            root.join("container-args.txt"),
        );
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

/// writes a cache entry so flag arity comes from the cache, as it does for an
/// installed command. `subs` are subcommand names the entry advertises.
fn seed_command(harness: &Harness, command: &str, subs: &[&str], valued: &[&str], plain: &[&str]) {
    let switch = |name: &str| {
        if let Some(long) = name.strip_prefix("--") {
            return OwnedSwitch::Long(long.to_owned());
        }
        OwnedSwitch::Short(name.chars().nth(1).expect("short flag letter"))
    };
    let entries = valued
        .iter()
        .map(|name| (name, true))
        .chain(plain.iter().map(|name| (name, false)))
        .map(|(name, takes_value)| {
            ManpageEntry {
                switch: switch(name),
                param:  takes_value
                    .then(|| OwnedParam::new(ParamKind::Mandatory, "VALUE".to_owned())),
                desc:   String::new(),
            }
        })
        .collect();
    let result = ManpageResult {
        entries,
        subcommands: subs
            .iter()
            .map(|sub| ManpageSubcommand::new((*sub).to_owned(), String::new()))
            .collect(),
        positional_choices: Vec::new(),
        positionals: Vec::new(),
        description: String::new(),
    };
    write_result(&harness.cache_dir, command, "help", &result).expect("seed cache");
}

// === ssh/scp/sftp ===

fn sorted_values(cands: &[Cand]) -> Vec<String> {
    let mut values = Harness::values(cands);
    values.sort();
    values
}

fn ssh_home(harness: &Harness) -> PathBuf {
    let home = harness.root.join("home");
    fs::create_dir_all(home.join(".ssh")).expect("ssh dir");
    home
}

fn ssh_home_with_inventory(harness: &Harness) -> String {
    let home = ssh_home(harness);
    let ssh = home.join(".ssh");
    fs::write(
        ssh.join("config"),
        concat!(
            "Host tt-example tt-devbox\n",
            "  hOsT\ttt-mixedcase tt-spaced\n",
            "Host *.wildcard question? !negated tt-literal # inline comment\n",
            "HOST=tt-equals\n",
        ),
    )
    .expect("ssh config");
    fs::write(
        ssh.join("known_hosts"),
        concat!(
            "tt-known.example ssh-ed25519 AAAA\n",
            "@cert-authority tt-ca.example,*.tt-ca.example ssh-ed25519 AAAA\n",
            "@revoked tt-revoked.example ssh-ed25519 AAAA\n",
            "[tt-bracket.example]:2222 ssh-ed25519 AAAA\n",
            "[tt-default.example]:22 ssh-ed25519 AAAA\n",
            "tt-plain.example,[tt-comma.example]:2200 ssh-ed25519 AAAA\n",
            "|1|hashed ignored\n",
        ),
    )
    .expect("known hosts");
    home.to_string_lossy().into_owned()
}

#[test]
fn ssh_hosts_follow_user_config_includes() {
    let harness = Harness::new("dyn-ssh-includes");
    let home = ssh_home(&harness);
    let ssh = home.join(".ssh");
    fs::create_dir_all(ssh.join("conf.d")).unwrap();
    fs::write(
        ssh.join("config"),
        "Include conf.d/*.conf\nInclude loop\nInclude ~/.ssh/tilde\nHost tt-direct\n",
    )
    .unwrap();
    fs::write(ssh.join("conf.d/a.conf"), "Host tt-included-a\n").unwrap();
    fs::write(
        ssh.join("conf.d/b.conf"),
        "Host tt-included-b\nInclude ../config\n",
    )
    .unwrap();
    fs::write(ssh.join("loop"), "Include loop\nHost tt-looped\n").unwrap();
    fs::write(ssh.join("tilde"), "Host tt-tilded\n").unwrap();
    fs::write(
        ssh.join("known_hosts2"),
        "tt-legacy.example ssh-ed25519 AAAA\n|1|x|y ssh-rsa AAAA\n",
    )
    .unwrap();

    let home_dir = home.to_string_lossy().into_owned();
    let (stdout, ..) = harness.run(&["ssh", "tt-"], &[("HOME", &home_dir)]);
    let hosts = parse_output(&stdout).expect("hosts");
    assert_eq!(sorted_values(&hosts), vec![
        "tt-direct",
        "tt-included-a",
        "tt-included-b",
        "tt-legacy.example",
        "tt-looped",
        "tt-tilded",
    ]);
}

#[test]
fn scp_completes_hosts_with_a_trailing_colon_until_the_path_begins() {
    let harness = Harness::new("dyn-scp-position");
    seed_command(&harness, "scp", &[], &["-P"], &[]);
    let home = ssh_home_with_inventory(&harness);

    let (host_out, ..) = harness.run(&["scp", "tt-ex"], &[("HOME", &home)]);
    let host = parse_output(&host_out).expect("typed remote host");
    assert_eq!(Harness::values(&host)[0], "tt-example:");

    let (user_host_out, ..) = harness.run(&["scp", "bob@tt-ex"], &[("HOME", &home)]);
    let user_host = parse_output(&user_host_out).expect("user-qualified host");
    assert_eq!(Harness::values(&user_host)[0], "bob@tt-example:");

    let (after_port, ..) =
        harness.run(&["scp", "src.txt", "-P", "22", "tt-ex"], &[("HOME", &home)]);
    assert!(
        parse_output(&after_port).is_some(),
        "options may follow operands, stdout={after_port}"
    );

    for spans in [
        &["scp", "tt-example:"][..],
        &["scp", "./tt-ex"][..],
        &["scp", "-P", "tt-"][..],
        &["scp", "src.txt", "-P", "tt-"][..],
    ] {
        let (stdout, ..) = harness.run(spans, &[("HOME", &home)]);
        assert_eq!(stdout.trim(), "null", "spans={spans:?}, stdout={stdout}");
    }
}

#[test]
fn ssh_jump_chain_completes_the_last_hop_and_keeps_the_earlier_ones() {
    let harness = Harness::new("dyn-ssh-jump-chain");
    let home = ssh_home_with_inventory(&harness);
    let (stdout, ..) = harness.run(&["ssh", "-J", "bastion,tt-ex"], &[("HOME", &home)]);
    let hosts = parse_output(&stdout).expect("last hop");
    assert_eq!(Harness::values(&hosts)[0], "bastion,tt-example");

    let (user, ..) = harness.run(&["ssh", "-J", "a,bob@tt-ex"], &[("HOME", &home)]);
    let with_user = parse_output(&user).expect("last hop with a user");
    assert_eq!(Harness::values(&with_user)[0], "a,bob@tt-example");
}

#[test]
fn ssh_clustered_short_flags_follow_the_cached_arity() {
    let harness = Harness::new("dyn-ssh-clusters");
    seed_command(&harness, "ssh", &[], &["-J", "-L", "-p"], &[
        "-f", "-N", "-v",
    ]);
    let home = ssh_home_with_inventory(&harness);
    let (host, ..) = harness.run(&["ssh", "-fNL", "8080:h:80", "tt-"], &[("HOME", &home)]);
    assert!(parse_output(&host).is_some(), "stdout={host}");

    let (value, ..) = harness.run(&["ssh", "-fNL", ""], &[("HOME", &home)]);
    assert_eq!(value.trim(), "null", "cluster value is not a host");

    let (attached, ..) = harness.run(&["ssh", "-fNp22", "tt-"], &[("HOME", &home)]);
    assert!(parse_output(&attached).is_some(), "stdout={attached}");

    let (jump, ..) = harness.run(&["ssh", "-vJ", "tt-ex"], &[("HOME", &home)]);
    let hosts = parse_output(&jump).expect("jump hosts after a cluster");
    assert_eq!(Harness::values(&hosts)[0], "tt-example");
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

#[test]
fn kubectl_cluster_selectors_are_forwarded_wherever_they_are_typed() {
    let harness = Harness::new("dyn-kubectl-selectors");
    let (stdout, ..) = harness.run(
        &[
            "kubectl",
            "--context",
            "dev",
            "-n",
            "prod",
            "get",
            "--cluster",
            "c1",
            "--user=bob",
            "-s",
            "https://api",
            "pods",
            "",
        ],
        &[],
    );
    let cands = parse_output(&stdout).expect("pods");
    assert_eq!(Harness::values(&cands), vec!["pod-a", "pod-b"]);
    let args = harness.read_aux("KUBECTL_ARGS_FILE");
    for forwarded in [
        "--context dev",
        "-n prod",
        "--cluster c1",
        "--user bob",
        "-s https://api",
    ] {
        assert!(args.contains(forwarded), "args captured: {args}");
    }
}

#[test]
fn kubectl_typed_prefix_matches_names_past_the_row_limit() {
    let harness = Harness::new("dyn-kubectl-limit-prefix");
    let (stdout, stderr, ok) = harness.run(&["kubectl", "get", "pods", "pod-b"], &[(
        "INSHELLAH_DYNAMIC_LIMIT",
        "1",
    )]);
    assert!(ok, "stderr={stderr}");
    let candidates = parse_output(&stdout).expect("prefix match beyond the limit");
    assert_eq!(Harness::values(&candidates), vec!["pod-b"]);
}

#[test]
fn kubectl_scrapes_a_missing_subcommand_before_reading_its_flags() {
    let harness = Harness::new("dyn-kubectl-scraped-arity");
    write_executable(
        &harness.bin_dir,
        "kubectl",
        r#"#!/bin/sh
case "$*" in
  "--help") printf 'kubectl controls the cluster.\n\nCommands:\n  get  Display resources\n' ;;
  "get --help") printf 'Display one or many resources.\n\nOptions:\n    -A, --all-namespaces=false:\n\tIf present, list the requested object(s) across all namespaces.\n\n    -o, --output=\047\047:\n\tOutput format.\n\nUsage:\n  kubectl get [(-o|--output=)json|yaml] [options]\n' ;;
  *--no-headers*) printf 'args=%s\n' "$*" > "$KUBECTL_ARGS_FILE"; printf 'pod-a\n' ;;
esac
"#,
    );

    let (stdout, ..) = harness.run(&["kubectl", "get", "-o", "wide", "pods", ""], &[]);
    assert!(parse_output(&stdout).is_some(), "stdout={stdout}");
    let args = harness.read_aux("KUBECTL_ARGS_FILE");
    assert!(args.contains("args=get pods "), "args captured: {args}");
}

// === docker / podman ===

#[test]
fn container_global_options_locate_the_verb() {
    let harness = Harness::new("dyn-container-global-options");
    let (stdout, ..) = harness.run(&["docker", "--context", "prod", "exec", ""], &[]);
    let candidates = parse_output(&stdout).expect("containers");
    assert_eq!(Harness::values(&candidates), vec![
        "docker-web",
        "docker-db"
    ]);
    let args = harness.read_aux("CONTAINER_ARGS_FILE");
    assert!(args.contains("--context prod ps -a"), "args={args}");
    assert!(args.contains("--last 200"), "args={args}");

    let (remote_out, ..) = harness.run(&["podman", "--connection", "remote", "exec", ""], &[]);
    let remote = parse_output(&remote_out).expect("remote containers");
    assert_eq!(Harness::values(&remote), vec!["podman-web", "podman-db"]);
    let remote_args = harness.read_aux("CONTAINER_ARGS_FILE");
    assert!(
        remote_args.contains("--connection remote ps -a"),
        "args={remote_args}"
    );
}

#[test]
fn container_cached_subcommand_flags_decide_which_word_is_the_operand() {
    let harness = Harness::new("dyn-container-cached-arity");
    seed_command(&harness, "docker", &["logs"], &[], &[]);
    seed_command(&harness, "docker logs", &[], &["-n"], &[]);

    let (stdout, ..) = harness.run(&["docker", "logs", "-n", "20", ""], &[]);
    let candidates = parse_output(&stdout).expect("container operand");
    assert_eq!(Harness::values(&candidates)[0], "docker-web");
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
    assert_eq!(Harness::values(&cands), vec![
        "deps/demo",
        "deps/with space"
    ]);
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
        "renamed.txt",
        "spaced name.txt"
    ]);
}

#[test]
fn git_rm_completes_tracked_paths() {
    let harness = Harness::new("dyn-git-rm");
    let (stdout, ..) = harness.run(&["git", "rm", ""], &[]);
    let cands = parse_output(&stdout).expect("tracked");
    assert_eq!(Harness::values(&cands), vec![
        "src/main.rs",
        "README.md",
        "caf\u{e9}.txt"
    ]);
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
fn git_global_repository_selectors_are_forwarded_to_queries() {
    let cases: &[(&[&str], &str)] = &[
        (&["git", "-C", "/repo", "fetch", "origin", ""], "-C /repo"),
        (
            &["git", "--git-dir", "/repo/.git", "fetch", "origin", ""],
            "--git-dir /repo/.git",
        ),
        (
            &["git", "--work-tree", "/repo", "fetch", "origin", ""],
            "--work-tree /repo",
        ),
        (
            &["git", "--git-dir=/repo/.git", "fetch", "origin", ""],
            "--git-dir /repo/.git",
        ),
    ];
    for (index, (spans, expected)) in cases.iter().enumerate() {
        let harness = Harness::new(&format!("dyn-git-global-scope-{index}"));
        let (stdout, stderr, ok) = harness.run(spans, &[]);
        assert!(ok, "stderr={stderr}");
        assert!(parse_output(&stdout).is_some(), "spans={spans:?}");
        let captured = harness.read_aux("INSHELLAH_GIT_ARGS_FILE");
        assert!(captured.contains(expected), "captured={captured}");
        assert!(captured.contains("for-each-ref"), "captured={captured}");
    }
}

#[test]
fn git_push_refspec_destination_offers_the_remotes_branches() {
    let harness = Harness::new("dyn-git-push-destination");
    let (all_out, ..) = harness.run(&["git", "push", "origin", "main:"], &[]);
    let all = parse_output(&all_out).expect("destination branches");
    assert_eq!(Harness::values(&all), vec!["main:main", "main:release/1"]);

    let (partial_out, ..) = harness.run(&["git", "push", "origin", "+HEAD:re"], &[]);
    let partial = parse_output(&partial_out).expect("partial destination");
    assert_eq!(Harness::values(&partial), vec!["+HEAD:release/1"]);

    let (unknown, ..) = harness.run(&["git", "push", "upstream", "main:"], &[]);
    assert_eq!(unknown.trim(), "null", "no local branches as a fallback");

    let (source_out, ..) = harness.run(&["git", "push", "origin", "ma"], &[]);
    let source = parse_output(&source_out).expect("source refs");
    assert!(Harness::values(&source).iter().any(|name| name == "main"));
}

#[test]
fn git_add_and_submodule_paths_are_relative_to_the_working_directory() {
    let harness = Harness::new("dyn-git-subdir-paths");
    fs::write(harness.bin_dir.join("show-prefix"), "src/").unwrap();
    let (add_out, ..) = harness.run(&["git", "add", ""], &[]);
    let add = parse_output(&add_out).expect("changed paths");
    assert_eq!(Harness::values(&add), vec![
        "main.rs",
        "../new-file.txt",
        "../renamed.txt",
        "../spaced name.txt"
    ]);
    let captured = harness.read_aux("INSHELLAH_GIT_ARGS_FILE");
    assert!(
        captured.contains("status --porcelain=v1 -z -uall"),
        "captured={captured}"
    );

    let (submodules_out, ..) = harness.run(&["git", "submodule", "update", ""], &[]);
    let submodules = parse_output(&submodules_out).expect("submodule paths");
    assert_eq!(Harness::values(&submodules), vec![
        "../deps/demo",
        "../deps/with space"
    ]);
    let submodule_args = harness.read_aux("INSHELLAH_GIT_ARGS_FILE");
    assert!(
        submodule_args.contains("--file /repo/.gitmodules --null"),
        "captured={submodule_args}"
    );
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
