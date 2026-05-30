// SPDX-License-Identifier: EUPL-1.2
//! per-command dynamic completion: git refs, kubectl resources, systemctl
//! units, etc. fires from `cmd_complete` when the static cache yields
//! nothing. the nushell shim is now just JSON glue; this module is the
//! source of truth for everything that used to live in its match arms.

use std::time::{Duration, Instant};

use crate::config::Config;
use crate::subprocess::{run_quiet, run_quiet_with};

/// candidates as JSON `{"value":..,"description":..}`, or None to hand
/// off to nu's file completer.
pub fn dynamic_complete(spans: &[String], cfg: &Config) -> Option<Vec<String>> {
    if spans.is_empty() {
        return None;
    }
    let deadline = if cfg.dynamic_timeout_ms == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(cfg.dynamic_timeout_ms))
    };
    let ctx = DynCtx {
        deadline,
        limit: cfg.dynamic_limit,
    };
    let raw = dispatch(spans, &ctx)?;
    let last = spans.last().map(String::as_str).unwrap_or("");
    let filtered = filter_candidates(raw, last)?;
    if filtered.is_empty() {
        None
    } else {
        Some(filtered.into_iter().map(|c| c.into_json()).collect())
    }
}

#[derive(Clone, Copy)]
struct DynCtx {
    deadline: Option<Instant>,
    limit: usize,
}

impl DynCtx {
    fn ms_left(&self) -> u64 {
        match self.deadline {
            None => u64::MAX,
            Some(d) => d
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u64::MAX as u128) as u64,
        }
    }

    fn budget_exhausted(&self) -> bool {
        match self.deadline {
            None => false,
            Some(d) => Instant::now() >= d,
        }
    }

    fn limit_args(&self, flag: &str) -> Vec<String> {
        if self.limit == 0 {
            Vec::new()
        } else {
            vec![flag.to_string(), self.limit.to_string()]
        }
    }
}

#[derive(Clone, Debug)]
struct Candidate {
    value: String,
    description: String,
}

impl Candidate {
    fn new(value: impl Into<String>, description: impl Into<String>) -> Self {
        Candidate {
            value: value.into(),
            description: description.into(),
        }
    }

    fn into_json(self) -> String {
        let mut out = String::with_capacity(self.value.len() + self.description.len() + 30);
        out.push_str(r#"{"value":""#);
        push_json_escaped(&mut out, &self.value);
        out.push_str(r#"","description":""#);
        push_json_escaped(&mut out, &self.description);
        out.push_str(r#""}"#);
        out
    }
}

fn push_json_escaped(out: &mut String, s: &str) {
    use std::fmt::Write as _;
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

fn dispatch(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let cmd = spans[0].as_str();
    match cmd {
        "nix" => nix_completions(spans, ctx),
        "systemctl" => systemctl_completions(spans, ctx),
        "journalctl" => journalctl_completions(spans, ctx),
        "coredumpctl" => coredumpctl_completions(spans, ctx),
        "loginctl" => loginctl_completions(spans, ctx),
        "machinectl" => machinectl_completions(spans, ctx),
        "networkctl" => networkctl_completions(spans, ctx),
        "hostnamectl" | "timedatectl" | "localectl" => None,
        "ssh" | "scp" | "sftp" => ssh_completions(spans, ctx),
        "docker" | "podman" => docker_like_completions(spans, ctx),
        "kubectl" => kubectl_completions(spans, ctx),
        "git" => git_completions(spans, ctx),
        "jj" => jj_completions(spans, ctx),
        "npm" | "pnpm" | "yarn" => npm_like_completions(spans, ctx),
        "make" => make_completions(spans, ctx),
        "just" => just_completions(spans, ctx),
        "cargo" => cargo_completions(spans, ctx),
        "kill" | "pkill" => kill_completions(spans, ctx),
        _ => None,
    }
}

/// drops exact-match subcommand/external candidates so a typed-out word
/// doesn't get echoed back and mask downstream completers.
fn filter_candidates(items: Vec<Candidate>, prefix: &str) -> Option<Vec<Candidate>> {
    if items.is_empty() {
        return None;
    }
    if prefix.is_empty() {
        return Some(items);
    }
    let mut scored: Vec<(i32, usize, Candidate)> = Vec::with_capacity(items.len());
    for (idx, item) in items.into_iter().enumerate() {
        let score = fuzzy_score(prefix, &item.value);
        if score <= 0 {
            continue;
        }
        let desc_lc = item.description.to_ascii_lowercase();
        let exact_command = item.value == prefix
            && (desc_lc.contains("subcommand") || desc_lc == "external command");
        if exact_command {
            continue;
        }
        scored.push((score, idx, item));
    }
    if scored.is_empty() {
        return None;
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    Some(scored.into_iter().map(|(_, _, c)| c).collect())
}

/// duplicate of `main.rs::fuzzy_score`. binary symbols aren't reachable
/// from the lib crate, so changes here must mirror there.
fn fuzzy_score(needle: &str, haystack: &str) -> i32 {
    let needle_len = needle.len();
    let haystack_len = haystack.len();
    if needle_len == 0 {
        return 1;
    }
    if needle_len > haystack_len {
        return 0;
    }
    if needle == haystack {
        return 1000;
    }

    let needle = needle.as_bytes();
    let haystack = haystack.as_bytes();
    if starts_with_ignore_ascii_case(haystack, needle) {
        return 900 + (needle_len as i32 * 100 / haystack_len as i32);
    }

    let mut needle_idx = 0usize;
    let mut score = 0i32;
    let mut prev_match: Option<usize> = None;

    for (hay_idx, &c) in haystack.iter().enumerate() {
        if needle_idx >= needle_len {
            break;
        }
        if c.eq_ignore_ascii_case(&needle[needle_idx]) {
            let boundary = hay_idx == 0
                || haystack[hay_idx - 1] == b'-'
                || haystack[hay_idx - 1] == b'_'
                || (haystack[hay_idx - 1].is_ascii_lowercase()
                    && haystack[hay_idx].is_ascii_uppercase());
            let consecutive = prev_match == Some(hay_idx.saturating_sub(1));
            score += if boundary { 50 } else { 10 };
            if consecutive {
                score += 20;
            }
            needle_idx += 1;
            prev_match = Some(hay_idx);
            continue;
        }
    }

    if needle_idx == needle_len { score } else { 0 }
}

fn starts_with_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len()
        && haystack
            .iter()
            .zip(needle)
            .all(|(&hay, &needle)| hay.eq_ignore_ascii_case(&needle))
}

// === per-command branches ===

/// `NIX_GET_COMPLETIONS=N` is nix's documented completions protocol; the
/// first stdout line is a `kind` header that the loop below skips.
fn nix_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if spans.len() < 2 {
        return None;
    }
    if ctx.budget_exhausted() {
        return None;
    }
    let nix_index = spans.len() - 1;
    let last = spans.last().map(String::as_str).unwrap_or("");
    let cmd_args: Vec<String> = spans.to_vec();
    let raw = run_quiet_with(&cmd_args, ctx.ms_left(), |cmd| {
        cmd.env("NIX_GET_COMPLETIONS", nix_index.to_string());
    })?;
    let mut lines: Vec<String> = raw
        .split('\n')
        .skip(1)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let enrich = lines.len() < 6 && looks_like_flake_pkg(last);
    if !enrich {
        return Some(
            lines
                .drain(..)
                .map(|v| Candidate::new(v, String::new()))
                .collect(),
        );
    }
    let mut out: Vec<Candidate> = Vec::with_capacity(lines.len());
    for line in lines {
        let desc = nix_eval_description(&line, ctx).unwrap_or_default();
        out.push(Candidate::new(line, desc));
    }
    Some(out)
}

fn looks_like_flake_pkg(s: &str) -> bool {
    let Some((lhs, rhs)) = s.split_once('#') else {
        return false;
    };
    is_flake_ident(lhs) && is_flake_ident(rhs)
}

fn is_flake_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn nix_eval_description(installable: &str, ctx: &DynCtx) -> Option<String> {
    if ctx.budget_exhausted() {
        return None;
    }
    let args: Vec<String> = vec![
        "nix".into(),
        "eval".into(),
        "--raw".into(),
        "--impure".into(),
        installable.into(),
        "--apply".into(),
        "f: f.meta.description".into(),
    ];
    let out = run_quiet_with(&args, ctx.ms_left(), |cmd| {
        cmd.env("NIX_ALLOW_UNFREE", "1");
        cmd.env("NIX_ALLOW_BROKEN", "1");
    })?;
    let trimmed = out.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

const SYSTEMCTL_UNIT_VERBS: &[&str] = &[
    "status",
    "show",
    "cat",
    "help",
    "start",
    "stop",
    "restart",
    "reload",
    "try-restart",
    "reload-or-restart",
    "reload-or-try-restart",
    "isolate",
    "kill",
    "reset-failed",
    "enable",
    "disable",
    "reenable",
    "preset",
    "mask",
    "unmask",
    "is-active",
    "is-failed",
    "is-enabled",
    "edit",
];

fn systemctl_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let verb = first_positional(&spans[1..])?;
    if !SYSTEMCTL_UNIT_VERBS.contains(&verb) || spans.len() < 3 {
        return None;
    }
    let scope = if spans.iter().any(|s| s == "--user") {
        &["--user"][..]
    } else {
        &[]
    };
    let last = spans.last().map(String::as_str).unwrap_or("");
    unit_candidates(scope, last, ctx)
}

fn first_positional(args: &[String]) -> Option<&str> {
    args.iter()
        .find(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
}

fn unit_candidates(scope: &[&str], prefix: &str, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec!["systemctl".into()];
    args.extend(scope.iter().map(|s| s.to_string()));
    args.extend(
        [
            "list-units",
            "--all",
            "--no-pager",
            "--plain",
            "--full",
            "--no-legend",
        ]
        .into_iter()
        .map(str::to_string),
    );
    args.push(format!("{prefix}*"));
    let out = run_quiet(&args, ctx.ms_left())?;
    let mut candidates = Vec::new();
    // UNIT LOAD ACTIVE SUB DESCRIPTION...
    for line in out.lines() {
        let mut parts = line.split_whitespace();
        let Some(unit) = parts.next() else { continue };
        if parts.clone().count() < 4 {
            continue;
        }
        let mut rest = parts;
        let _load = rest.next();
        let _active = rest.next();
        let _sub = rest.next();
        let desc = rest.collect::<Vec<_>>().join(" ");
        candidates.push(Candidate::new(unit, desc.trim()));
    }
    if candidates.is_empty() {
        None
    } else {
        Some(candidates)
    }
}

fn journalctl_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if spans.len() < 2 {
        return None;
    }
    let prev = if spans.len() >= 2 {
        spans[spans.len() - 2].as_str()
    } else {
        ""
    };
    if prev != "--unit" && prev != "-u" {
        return None;
    }
    let user_scope = spans
        .iter()
        .any(|s| s == "--user-unit" || s == "--user");
    let scope: &[&str] = if user_scope { &["--user"] } else { &[] };
    let last = spans.last().map(String::as_str).unwrap_or("");
    unit_candidates(scope, last, ctx)
}

fn coredumpctl_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let unit_verbs = ["dump", "info", "debug", "list"];
    let verb = spans.get(1)?;
    if !unit_verbs.contains(&verb.as_str()) || spans.len() < 3 {
        return None;
    }
    let last = spans.last().map(String::as_str).unwrap_or("");
    let mut out = unit_candidates(&[], last, ctx).unwrap_or_default();

    let mut args: Vec<String> = vec!["coredumpctl".into(), "list".into()];
    args.extend(ctx.limit_args("-n"));
    args.extend(["--no-pager", "--no-legend"].into_iter().map(str::to_string));
    if let Some(text) = run_quiet(&args, ctx.ms_left()) {
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 5 {
                continue;
            }
            let pid = parts[4];
            let tail = parts.get(9).copied().unwrap_or("");
            out.push(Candidate::new(pid, format!("PID {pid} {tail}")));
        }
    }
    (!out.is_empty()).then_some(out)
}

fn loginctl_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let user_verbs = [
        "user-status",
        "show-user",
        "enable-linger",
        "disable-linger",
        "kill-user",
        "terminate-user",
    ];
    let session_verbs = [
        "session-status",
        "show-session",
        "activate",
        "lock-session",
        "unlock-session",
        "terminate-session",
        "kill-session",
    ];
    let sub = spans.get(1).map(String::as_str).unwrap_or("");
    if user_verbs.contains(&sub) && spans.len() >= 3 {
        let out = run_quiet(
            &[
                "loginctl".into(),
                "list-users".into(),
                "--no-pager".into(),
                "--no-legend".into(),
            ],
            ctx.ms_left(),
        )?;
        let mut candidates = Vec::new();
        for line in out.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            candidates.push(Candidate::new(parts[1], format!("UID {}", parts[0])));
        }
        (!candidates.is_empty()).then_some(candidates)
    } else if session_verbs.contains(&sub) && spans.len() >= 3 {
        let out = run_quiet(
            &[
                "loginctl".into(),
                "list-sessions".into(),
                "--no-pager".into(),
                "--no-legend".into(),
            ],
            ctx.ms_left(),
        )?;
        let mut candidates = Vec::new();
        for line in out.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 3 {
                continue;
            }
            candidates.push(Candidate::new(parts[0], format!("user {}", parts[2])));
        }
        (!candidates.is_empty()).then_some(candidates)
    } else {
        None
    }
}

fn machinectl_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let verbs = [
        "status", "show", "start", "login", "shell", "enable", "disable", "poweroff", "reboot",
        "terminate", "kill", "bind", "copy-to", "copy-from",
    ];
    let sub = spans.get(1).map(String::as_str).unwrap_or("");
    if !verbs.contains(&sub) || spans.len() < 3 {
        return None;
    }
    let out = run_quiet(
        &[
            "machinectl".into(),
            "list".into(),
            "--no-pager".into(),
            "--no-legend".into(),
        ],
        ctx.ms_left(),
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some(name) = parts.first() else { continue };
        let desc = parts.get(1).copied().unwrap_or("");
        candidates.push(Candidate::new(*name, desc));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn networkctl_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let verbs = [
        "status",
        "show",
        "up",
        "down",
        "renew",
        "forcerenew",
        "reconfigure",
        "delete",
    ];
    let sub = spans.get(1).map(String::as_str).unwrap_or("");
    if !verbs.contains(&sub) || spans.len() < 3 {
        return None;
    }
    let out = run_quiet(
        &[
            "networkctl".into(),
            "list".into(),
            "--no-pager".into(),
            "--no-legend".into(),
        ],
        ctx.ms_left(),
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        candidates.push(Candidate::new(parts[1], format!("{} {}", parts[2], parts[3])));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn ssh_completions(_spans: &[String], _ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Candidate> = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let cfg_path = std::path::PathBuf::from(&home).join(".ssh/config");
        if let Ok(contents) = std::fs::read_to_string(&cfg_path) {
            for line in contents.lines() {
                let trimmed = line.trim_start();
                let rest = trimmed
                    .strip_prefix("Host ")
                    .or_else(|| trimmed.strip_prefix("host "))
                    .or_else(|| trimmed.strip_prefix("HOST "));
                let Some(rest) = rest else { continue };
                for host in rest.split_whitespace() {
                    if host.contains('*') || host.is_empty() {
                        continue;
                    }
                    if seen.insert(host.to_string()) {
                        out.push(Candidate::new(host, ""));
                    }
                }
            }
        }
        let kh_path = std::path::PathBuf::from(&home).join(".ssh/known_hosts");
        if let Ok(contents) = std::fs::read_to_string(&kh_path) {
            for line in contents.lines() {
                let Some(first) = line.split_whitespace().next() else {
                    continue;
                };
                if first.starts_with('|') || first.starts_with('[') {
                    continue;
                }
                for host in first.split(',') {
                    if host.is_empty() {
                        continue;
                    }
                    if seen.insert(host.to_string()) {
                        out.push(Candidate::new(host, ""));
                    }
                }
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

const DOCKER_CONTAINER_VERBS: &[&str] = &[
    "exec", "logs", "inspect", "start", "stop", "restart", "rm", "kill", "attach", "cp", "top",
    "wait", "pause", "unpause", "port", "commit", "diff", "export",
];
const DOCKER_IMAGE_VERBS: &[&str] = &["run", "rmi", "tag", "push", "pull", "history", "save", "create"];

fn docker_like_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let cmd = spans[0].clone();
    let sub = spans.get(1).map(String::as_str).unwrap_or("");
    if DOCKER_CONTAINER_VERBS.contains(&sub) {
        let mut args = vec![cmd, "ps".into()];
        args.extend(ctx.limit_args("--last"));
        args.extend(
            ["--format", "{{.Names}}\t{{.Image}}"]
                .into_iter()
                .map(str::to_string),
        );
        let out = run_quiet(&args, ctx.ms_left())?;
        let mut candidates = Vec::new();
        for line in out.lines() {
            let mut parts = line.splitn(2, '\t');
            let Some(name) = parts.next() else { continue };
            let image = parts.next().unwrap_or("");
            candidates.push(Candidate::new(name, image));
        }
        (!candidates.is_empty()).then_some(candidates)
    } else if DOCKER_IMAGE_VERBS.contains(&sub) {
        let args: Vec<String> = vec![
            cmd,
            "images".into(),
            "--format".into(),
            "{{.Repository}}:{{.Tag}}\t{{.Size}}".into(),
        ];
        let out = run_quiet(&args, ctx.ms_left())?;
        let mut candidates = Vec::new();
        for line in out.lines() {
            let mut parts = line.splitn(2, '\t');
            let Some(repo) = parts.next() else { continue };
            let size = parts.next().unwrap_or("");
            if repo.ends_with(":<none>") {
                continue;
            }
            candidates.push(Candidate::new(repo, size));
        }
        (!candidates.is_empty()).then_some(candidates)
    } else {
        None
    }
}

fn kubectl_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let sub = spans.get(1).map(String::as_str).unwrap_or("");
    let resource_verbs = [
        "get", "describe", "delete", "edit", "scale", "annotate", "label",
    ];
    if resource_verbs.contains(&sub) && spans.len() >= 4 {
        let kind = spans.get(2).map(String::as_str).unwrap_or("");
        return kubectl_names(kind, spans, ctx);
    }
    if (sub == "logs" || sub == "exec" || sub == "port-forward") && spans.len() >= 3 {
        return kubectl_names("pods", spans, ctx);
    }
    if sub == "rollout" && spans.len() >= 5 {
        let action = spans.get(2).map(String::as_str).unwrap_or("");
        let kind = spans.get(3).map(String::as_str).unwrap_or("");
        if matches!(
            action,
            "history" | "pause" | "restart" | "resume" | "status" | "undo"
        ) {
            return kubectl_names(kind, spans, ctx);
        }
    }
    None
}

struct KubeScope {
    args: Vec<String>,
    all: bool,
}

fn kubectl_scope(spans: &[String]) -> KubeScope {
    let all_namespaces = spans.iter().any(|s| s == "-A" || s == "--all-namespaces");
    if all_namespaces {
        return KubeScope {
            args: vec!["--all-namespaces".into()],
            all: true,
        };
    }
    let mut namespace: Option<String> = None;
    let mut i = 0usize;
    while i < spans.len() {
        let s = &spans[i];
        if let Some(rest) = s.strip_prefix("--namespace=") {
            namespace = Some(rest.to_string());
        } else if (s == "-n" || s == "--namespace") && i + 1 < spans.len() {
            namespace = Some(spans[i + 1].clone());
            i += 1;
        }
        i += 1;
    }
    match namespace {
        Some(ns) if !ns.is_empty() => KubeScope {
            args: vec!["-n".into(), ns],
            all: false,
        },
        _ => KubeScope {
            args: Vec::new(),
            all: false,
        },
    }
}

fn kubectl_names(kind: &str, spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if kind.is_empty() || kind.starts_with('-') {
        return None;
    }
    let scope = kubectl_scope(spans);
    let columns = if scope.all {
        "custom-columns=NAMESPACE:.metadata.namespace,NAME:.metadata.name"
    } else {
        "custom-columns=NAME:.metadata.name"
    };
    let mut args: Vec<String> = vec!["kubectl".into(), "get".into(), kind.into()];
    args.extend(scope.args.iter().cloned());
    args.extend(
        ["--no-headers", "-o", columns]
            .into_iter()
            .map(str::to_string),
    );
    let out = run_quiet(&args, ctx.ms_left())?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if scope.all {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            candidates.push(Candidate::new(parts[1], format!("{kind} in {}", parts[0])));
        } else {
            candidates.push(Candidate::new(line, kind));
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}

const GIT_TOP_VERBS: &[&str] = &[
    "add",
    "bisect",
    "branch",
    "checkout",
    "cherry-pick",
    "clone",
    "commit",
    "diff",
    "fetch",
    "grep",
    "init",
    "log",
    "merge",
    "mv",
    "pull",
    "push",
    "rebase",
    "reflog",
    "remote",
    "reset",
    "restore",
    "revert",
    "rm",
    "show",
    "stash",
    "status",
    "submodule",
    "switch",
    "tag",
    "worktree",
];
const GIT_REF_VERBS: &[&str] = &[
    "checkout",
    "merge",
    "rebase",
    "log",
    "diff",
    "show",
    "reset",
    "cherry-pick",
    "revert",
    "tag",
    "blame",
    "bisect",
];
const GIT_BRANCH_VERBS: &[&str] = &["switch", "branch"];
const GIT_REMOTE_VERBS: &[&str] = &[
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
];
const GIT_STASH_VERBS: &[&str] = &[
    "push", "save", "list", "show", "drop", "pop", "apply", "branch", "clear", "create", "store",
];
const GIT_SUBMODULE_VERBS: &[&str] = &[
    "add",
    "status",
    "init",
    "deinit",
    "update",
    "set-branch",
    "set-url",
    "summary",
    "foreach",
    "sync",
    "absorbgitdirs",
];
const GIT_BISECT_VERBS: &[&str] = &[
    "start",
    "bad",
    "good",
    "new",
    "old",
    "terms",
    "skip",
    "next",
    "reset",
    "visualize",
    "view",
    "replay",
    "log",
    "run",
];
const GIT_WORKTREE_VERBS: &[&str] = &[
    "add", "list", "lock", "move", "prune", "remove", "repair", "unlock",
];

fn git_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let span_len = spans.len();
    let sub = spans.get(1).map(String::as_str).unwrap_or("");
    let prev = if span_len >= 2 {
        spans[span_len - 2].as_str()
    } else {
        ""
    };
    let positionals_after_sub: Vec<&str> = spans
        .iter()
        .skip(2)
        .filter(|s| !s.is_empty() && !s.starts_with('-'))
        .map(String::as_str)
        .collect();

    if span_len <= 2 {
        return Some(
            GIT_TOP_VERBS
                .iter()
                .map(|v| Candidate::new(*v, "git subcommand"))
                .collect(),
        );
    }

    if sub == "worktree" {
        let verb = spans.get(2).map(String::as_str).unwrap_or("");
        if span_len <= 3 {
            return Some(
                GIT_WORKTREE_VERBS
                    .iter()
                    .map(|v| Candidate::new(*v, "worktree subcommand"))
                    .collect(),
            );
        }
        if ["remove", "move", "lock", "unlock", "repair"].contains(&verb) {
            return git_worktrees(ctx);
        }
        if verb == "add" && span_len >= 5 {
            return git_refs(ctx);
        }
        return None;
    }
    if sub == "remote" && span_len >= 3 {
        let verb = spans.get(2).map(String::as_str).unwrap_or("");
        if span_len <= 3 {
            return Some(
                GIT_REMOTE_VERBS
                    .iter()
                    .map(|v| Candidate::new(*v, "remote subcommand"))
                    .collect(),
            );
        }
        if GIT_REMOTE_VERBS.contains(&verb) && verb != "add" {
            return git_remotes(ctx);
        }
        return None;
    }
    if matches!(sub, "fetch" | "push" | "pull") && span_len >= 3 {
        if positionals_after_sub.is_empty() {
            return git_remotes(ctx);
        } else {
            return git_refs(ctx);
        }
    }
    if sub == "stash" && span_len >= 3 {
        let verb = spans.get(2).map(String::as_str).unwrap_or("");
        if span_len <= 3 {
            return Some(
                GIT_STASH_VERBS
                    .iter()
                    .map(|v| Candidate::new(*v, "stash subcommand"))
                    .collect(),
            );
        }
        if ["show", "drop", "pop", "apply", "store"].contains(&verb) {
            return git_stashes(ctx);
        }
        if verb == "branch" && positionals_after_sub.len() >= 2 {
            return git_stashes(ctx);
        }
        return None;
    }
    if sub == "submodule" && span_len >= 3 {
        let verb = spans.get(2).map(String::as_str).unwrap_or("");
        if span_len <= 3 {
            return Some(
                GIT_SUBMODULE_VERBS
                    .iter()
                    .map(|v| Candidate::new(*v, "submodule subcommand"))
                    .collect(),
            );
        }
        if [
            "status", "init", "deinit", "update", "set-branch", "set-url", "summary", "foreach",
            "sync",
        ]
        .contains(&verb)
        {
            return git_submodules(ctx);
        }
        return None;
    }
    if sub == "bisect" && span_len >= 3 {
        let verb = spans.get(2).map(String::as_str).unwrap_or("");
        if span_len <= 3 {
            return Some(
                GIT_BISECT_VERBS
                    .iter()
                    .map(|v| Candidate::new(*v, "bisect subcommand"))
                    .collect(),
            );
        }
        if ["bad", "good", "new", "old", "skip", "reset", "start"].contains(&verb) {
            return git_refs(ctx);
        }
        return None;
    }
    if sub == "tag" && span_len >= 3 {
        let delete_or_verify = ["-d", "--delete", "-v", "--verify"]
            .iter()
            .any(|f| spans.iter().any(|s| s == f));
        if delete_or_verify {
            return git_tags(ctx);
        }
        if span_len >= 4 {
            return git_refs(ctx);
        }
        return git_tags(ctx);
    }
    if sub == "add" && span_len >= 3 {
        return git_status_paths(ctx);
    }
    if sub == "restore" && span_len >= 3 {
        if prev == "--source" || prev == "-s" {
            return git_refs(ctx);
        }
        return git_status_paths(ctx);
    }
    if sub == "rm" && span_len >= 3 {
        return git_tracked_paths(ctx);
    }
    if sub == "mv" && span_len >= 3 {
        return if positionals_after_sub.is_empty() {
            git_tracked_paths(ctx)
        } else {
            None
        };
    }
    if sub == "checkout" && span_len >= 3 {
        if ["-b", "-B", "--orphan"].contains(&prev) {
            return None;
        }
        return git_refs(ctx);
    }
    if sub == "switch" && span_len >= 3 {
        if ["-c", "-C", "--create", "--force-create", "--orphan"].contains(&prev) {
            return None;
        }
        return git_branches(ctx);
    }
    if GIT_BRANCH_VERBS.contains(&sub) && span_len >= 3 {
        return git_branches(ctx);
    }
    if GIT_REF_VERBS.contains(&sub) && span_len >= 3 {
        return git_refs(ctx);
    }
    None
}

fn git_refs(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec!["git".into(), "for-each-ref".into()];
    args.extend(ctx.limit_args("--count"));
    args.push("--format=%(refname:short)%09%(objecttype)%09%(contents:subject)".into());
    args.extend(
        ["refs/heads", "refs/remotes", "refs/tags"]
            .into_iter()
            .map(str::to_string),
    );
    let out = run_quiet(&args, ctx.ms_left())?;
    Some(parse_tabular(&out, 3, |p| Candidate::new(p[0], p[2])))
}

fn git_branches(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec!["git".into(), "for-each-ref".into()];
    args.extend(ctx.limit_args("--count"));
    args.push("--format=%(refname:short)%09%(contents:subject)".into());
    args.push("refs/heads".into());
    let out = run_quiet(&args, ctx.ms_left())?;
    Some(parse_tabular(&out, 2, |p| Candidate::new(p[0], p[1])))
}

fn git_tags(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec!["git".into(), "for-each-ref".into()];
    args.extend(ctx.limit_args("--count"));
    args.push("--format=%(refname:short)%09%(contents:subject)".into());
    args.push("refs/tags".into());
    let out = run_quiet(&args, ctx.ms_left())?;
    Some(parse_tabular(&out, 2, |p| Candidate::new(p[0], p[1])))
}

fn git_remotes(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run_quiet(&["git".into(), "remote".into()], ctx.ms_left())?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        candidates.push(Candidate::new(trimmed, "remote"));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn git_stashes(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec!["git".into(), "stash".into(), "list".into()];
    args.extend(ctx.limit_args("-n"));
    let out = run_quiet(&args, ctx.ms_left())?;
    let mut candidates = Vec::new();
    // stash@{N}: WIP on branch: subject
    for line in out.lines() {
        let Some(idx) = line.find(": ") else { continue };
        let stash = &line[..idx];
        if !stash.starts_with("stash@{") {
            continue;
        }
        let desc = line[idx + 2..].trim();
        candidates.push(Candidate::new(stash, desc));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn git_status_paths(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run_quiet(
        &[
            "git".into(),
            "status".into(),
            "--porcelain".into(),
            "-uall".into(),
        ],
        ctx.ms_left(),
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        if line.len() < 3 {
            continue;
        }
        let raw = &line[3..];
        let path = if let Some((_, after)) = raw.split_once(" -> ") {
            after.to_string()
        } else {
            raw.to_string()
        };
        candidates.push(Candidate::new(path, "changed path"));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn git_tracked_paths(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run_quiet(&["git".into(), "ls-files".into()], ctx.ms_left())?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        if line.is_empty() {
            continue;
        }
        candidates.push(Candidate::new(line, "tracked file"));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn git_submodules(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run_quiet(
        &[
            "git".into(),
            "config".into(),
            "--file".into(),
            ".gitmodules".into(),
            "--get-regexp".into(),
            "^submodule\\..*\\.path$".into(),
        ],
        ctx.ms_left(),
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        candidates.push(Candidate::new(parts[1], "submodule"));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn git_worktrees(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run_quiet(
        &[
            "git".into(),
            "worktree".into(),
            "list".into(),
            "--porcelain".into(),
        ],
        ctx.ms_left(),
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            let path = rest.trim();
            if !path.is_empty() {
                candidates.push(Candidate::new(path, ""));
            }
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}

const JJ_TOP_VERBS: &[&str] = &[
    "abandon",
    "absorb",
    "bookmark",
    "commit",
    "describe",
    "diff",
    "diffedit",
    "duplicate",
    "edit",
    "evolog",
    "file",
    "git",
    "interdiff",
    "log",
    "new",
    "operation",
    "op",
    "rebase",
    "resolve",
    "restore",
    "revert",
    "show",
    "sparse",
    "split",
    "squash",
    "status",
    "tag",
    "undo",
    "workspace",
    "b",
    "ci",
    "desc",
    "st",
];
const JJ_REV_FLAGS: &[&str] = &[
    "-r",
    "--revision",
    "--revisions",
    "--from",
    "--to",
    "--into",
    "-s",
    "--source",
    "-d",
    "--destination",
    "--insert-after",
    "--insert-before",
    "-A",
    "-B",
    "--before",
    "--after",
    "--onto",
    "--change",
];
// rewriting verbs draw their primary revset args from mutable(). their
// destination/anchor flags (below) still draw from all().
const JJ_MUTABLE_VERBS: &[&str] = &[
    "abandon",
    "describe",
    "edit",
    "metaedit",
    "parallelize",
    "rebase",
    "split",
    "squash",
];
const JJ_DEST_FLAGS: &[&str] = &[
    "-d",
    "--destination",
    "--onto",
    "--insert-after",
    "--insert-before",
    "-A",
    "-B",
    "--after",
    "--before",
];
// a bare positional is a fileset, not a revision
const JJ_FILE_POS_VERBS: &[&str] = &[
    "absorb", "commit", "diff", "diffedit", "fix", "log", "restore", "split", "squash",
];
// a bare positional is a revision the verb rewrites
const JJ_MUTABLE_POS_VERBS: &[&str] =
    &["abandon", "describe", "edit", "metaedit", "parallelize"];
// a bare positional is a revision drawn from all()
const JJ_ALL_POS_VERBS: &[&str] = &["duplicate", "new", "show"];
const JJ_CONFIG_VERBS: &[&str] = &["edit", "get", "list", "path", "set", "unset"];
const JJ_BOOKMARK_VERBS: &[&str] = &[
    "advance", "create", "delete", "forget", "list", "move", "rename", "set", "track", "untrack",
];
const JJ_GIT_VERBS: &[&str] = &[
    "clone",
    "colocation",
    "export",
    "fetch",
    "import",
    "init",
    "push",
    "remote",
    "root",
];
const JJ_REMOTE_VERBS: &[&str] = &["add", "list", "remove", "rename", "set-url"];
const JJ_OP_VERBS: &[&str] = &["abandon", "diff", "integrate", "log", "restore", "revert", "show"];
const JJ_FILE_VERBS: &[&str] = &[
    "annotate", "chmod", "list", "search", "show", "track", "untrack",
];
const JJ_WORKSPACE_VERBS: &[&str] = &["add", "forget", "list", "rename", "root", "update-stale"];
const JJ_SPARSE_VERBS: &[&str] = &["edit", "list", "reset", "set"];

fn jj_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let span_len = spans.len();
    let sub = spans.get(1).map(String::as_str).unwrap_or("");
    let prev = if span_len >= 2 {
        spans[span_len - 2].as_str()
    } else {
        ""
    };
    let last = spans.last().map(String::as_str).unwrap_or("");

    // rebase's `-b`/`--branch` is a revset; git push's `-b`/`--bookmark` is
    // a local bookmark name. disambiguate on the subcommand.
    if prev == "--branch" || (prev == "-b" && sub == "rebase") {
        return jj_revs(ctx, last, "mutable()");
    }
    if JJ_REV_FLAGS.contains(&prev) {
        return jj_revs(ctx, last, jj_flag_revset(sub, prev));
    }
    if prev == "-T" || prev == "--template" {
        return jj_templates(ctx);
    }
    if prev == "--remote" {
        return jj_remotes(ctx);
    }
    if prev == "--bookmark" || prev == "-b" {
        return jj_bookmarks(ctx, "all()");
    }
    if prev == "--at-operation" || prev == "--at-op" {
        return jj_ops(ctx);
    }
    if span_len <= 2 {
        return Some(
            JJ_TOP_VERBS
                .iter()
                .map(|v| Candidate::new(*v, "jj subcommand"))
                .collect(),
        );
    }
    if sub == "bookmark" || sub == "b" {
        let verb = spans.get(2).map(String::as_str).unwrap_or("");
        if span_len <= 3 {
            return Some(
                JJ_BOOKMARK_VERBS
                    .iter()
                    .map(|v| Candidate::new(*v, "bookmark subcommand"))
                    .collect(),
            );
        }
        if ["delete", "forget", "move", "rename", "set", "advance"].contains(&verb) {
            return jj_bookmarks(ctx, "all()");
        }
        if verb == "track" {
            return jj_remote_bookmarks(ctx, Some(false));
        }
        if verb == "untrack" {
            return jj_remote_bookmarks(ctx, Some(true));
        }
        return None;
    }
    if sub == "config" {
        let verb = spans.get(2).map(String::as_str).unwrap_or("");
        if span_len <= 3 {
            return Some(
                JJ_CONFIG_VERBS
                    .iter()
                    .map(|v| Candidate::new(*v, "config subcommand"))
                    .collect(),
            );
        }
        return match verb {
            "get" | "g" | "set" | "s" => jj_config_keys(ctx, true, false),
            "unset" | "u" => jj_config_keys(ctx, true, true),
            "list" | "l" => jj_config_keys(ctx, false, false),
            _ => None,
        };
    }
    if sub == "tag" {
        let verb = spans.get(2).map(String::as_str).unwrap_or("");
        if span_len <= 3 {
            return Some(
                ["delete", "list", "set"]
                    .iter()
                    .map(|v| Candidate::new(*v, "tag subcommand"))
                    .collect(),
            );
        }
        if ["delete", "set"].contains(&verb) {
            return jj_tags(ctx);
        }
        return None;
    }
    if sub == "git" {
        let git_verb = spans.get(2).map(String::as_str).unwrap_or("");
        let remote_verb = spans.get(3).map(String::as_str).unwrap_or("");
        if span_len <= 3 {
            return Some(
                JJ_GIT_VERBS
                    .iter()
                    .map(|v| Candidate::new(*v, "jj git subcommand"))
                    .collect(),
            );
        }
        if git_verb == "remote" {
            if span_len <= 4 {
                return Some(
                    JJ_REMOTE_VERBS
                        .iter()
                        .map(|v| Candidate::new(*v, "remote subcommand"))
                        .collect(),
                );
            }
            if ["remove", "rename", "set-url"].contains(&remote_verb) {
                return jj_remotes(ctx);
            }
            return None;
        }
        if matches!(git_verb, "fetch" | "push") {
            return jj_remotes(ctx);
        }
        return None;
    }
    if sub == "operation" || sub == "op" {
        let verb = spans.get(2).map(String::as_str).unwrap_or("");
        if span_len <= 3 {
            return Some(
                JJ_OP_VERBS
                    .iter()
                    .map(|v| Candidate::new(*v, "operation subcommand"))
                    .collect(),
            );
        }
        if ["abandon", "diff", "integrate", "restore", "revert", "show"].contains(&verb) {
            return jj_ops(ctx);
        }
        return None;
    }
    if sub == "file" {
        let verb = spans.get(2).map(String::as_str).unwrap_or("");
        if span_len <= 3 {
            return Some(
                JJ_FILE_VERBS
                    .iter()
                    .map(|v| Candidate::new(*v, "file subcommand"))
                    .collect(),
            );
        }
        if ["annotate", "chmod", "list", "search", "show", "untrack"].contains(&verb) {
            return jj_files(ctx);
        }
        return None;
    }
    if sub == "workspace" {
        let verb = spans.get(2).map(String::as_str).unwrap_or("");
        if span_len <= 3 {
            return Some(
                JJ_WORKSPACE_VERBS
                    .iter()
                    .map(|v| Candidate::new(*v, "workspace subcommand"))
                    .collect(),
            );
        }
        if ["forget", "update-stale"].contains(&verb) {
            return jj_workspaces(ctx);
        }
        return None;
    }
    if sub == "sparse" {
        if span_len <= 3 {
            return Some(
                JJ_SPARSE_VERBS
                    .iter()
                    .map(|v| Candidate::new(*v, "sparse subcommand"))
                    .collect(),
            );
        }
        return None;
    }
    // bare positional argument: file, revision, or nothing, per verb.
    if span_len >= 3 {
        if JJ_FILE_POS_VERBS.contains(&sub) {
            return jj_files(ctx);
        }
        if sub == "resolve" {
            return jj_conflicted_files(ctx);
        }
        if JJ_MUTABLE_POS_VERBS.contains(&sub) {
            return jj_revs(ctx, last, "mutable()");
        }
        if JJ_ALL_POS_VERBS.contains(&sub) {
            return jj_revs(ctx, last, "all()");
        }
    }
    None
}

/// rewriting verbs pull their primary revsets from `mutable()`. the same
/// verb's destination/anchor flags still pull from `all()`.
fn jj_flag_revset(sub: &str, flag: &str) -> &'static str {
    if JJ_MUTABLE_VERBS.contains(&sub) && !JJ_DEST_FLAGS.contains(&flag) {
        "mutable()"
    } else {
        "all()"
    }
}

/// jj completes the trailing symbol of a (possibly compound) revset and
/// re-prefixes each candidate with the rest of the expression
fn jj_revs(ctx: &DynCtx, partial: &str, revset: &str) -> Option<Vec<Candidate>> {
    let (prepend, _) = split_revset_trailing_name(partial);
    let mutable = revset != "all()";
    let groups = if mutable {
        vec![
            jj_bookmarks(ctx, revset),
            jj_change_ids(ctx, revset),
            jj_revset_aliases(ctx),
        ]
    } else {
        vec![
            jj_bookmarks(ctx, revset),
            jj_tags(ctx),
            jj_change_ids(ctx, revset),
            jj_remote_bookmarks(ctx, None),
            jj_revset_aliases(ctx),
        ]
    };
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Candidate> = Vec::new();
    for group in groups.into_iter().flatten() {
        for c in group {
            if !seen.insert(c.value.clone()) {
                continue;
            }
            if prepend.is_empty() {
                out.push(c);
            } else {
                out.push(Candidate::new(format!("{prepend}{}", c.value), c.description));
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

fn jj_change_ids(ctx: &DynCtx, revset: &str) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec![
        "jj".into(),
        "log".into(),
        "--ignore-working-copy".into(),
        "--no-graph".into(),
    ];
    args.extend(ctx.limit_args("-n"));
    args.extend(
        [
            "-r",
            revset,
            "-T",
            r#"change_id.shortest() ++ "\t" ++ description.first_line() ++ "\n""#,
        ]
        .into_iter()
        .map(str::to_string),
    );
    let out = run_quiet(&args, ctx.ms_left())?;
    Some(parse_tabular(&out, 2, |p| Candidate::new(p[0], p[1])))
}

/// config option names for `config get`/`set`/`list`/`unset`. `leaves_only`
/// keeps just settable leaf keys (for get/set/unset), `list` also wants the
/// intermediate table prefixes, derived here. `set_only` drops defaults so
/// `unset` offers only keys actually present in a config file.
fn jj_config_keys(ctx: &DynCtx, leaves_only: bool, set_only: bool) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec!["jj".into(), "config".into(), "list".into()];
    if !set_only {
        args.push("--include-defaults".into());
    }
    args.push("-T".into());
    args.push(r#"name ++ "\n""#.into());
    let out = run_quiet(&args, ctx.ms_left())?;
    let mut seen = std::collections::HashSet::new();
    let mut candidates = Vec::new();
    for line in out.lines() {
        let name = line.trim();
        // skip keys with quoted segments (e.g. `colors."error heading".fg`)
        if name.is_empty() || name.contains('"') {
            continue;
        }
        if !leaves_only {
            // emit every dotted prefix as its own candidate
            let mut idx = 0;
            while let Some(dot) = name[idx..].find('.') {
                let end = idx + dot;
                let prefix = &name[..end];
                if seen.insert(prefix.to_string()) {
                    candidates.push(Candidate::new(prefix, "config table"));
                }
                idx = end + 1;
            }
        }
        if seen.insert(name.to_string()) {
            candidates.push(Candidate::new(name, "config key"));
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}

/// files with unresolved conflicts, for `jj resolve`
fn jj_conflicted_files(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run_quiet(
        &["jj".into(), "resolve".into(), "--list".into()],
        ctx.ms_left(),
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        // "<path>    N-sided conflict ..."
        let Some(path) = line.split_whitespace().next() else {
            continue;
        };
        if path.is_empty() {
            continue;
        }
        candidates.push(Candidate::new(path, "conflict"));
    }
    (!candidates.is_empty()).then_some(candidates)
}

/// nullary revset aliases (e.g. `trunk`), matching jj's symbol-name set.
/// parameterised aliases come through quoted (`"trunk()"`) and are skipped.
fn jj_revset_aliases(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run_quiet(
        &[
            "jj".into(),
            "config".into(),
            "list".into(),
            "--include-defaults".into(),
            "revset-aliases".into(),
            "-T".into(),
            r#"name ++ "\n""#.into(),
        ],
        ctx.ms_left(),
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let Some(name) = line.trim().strip_prefix("revset-aliases.") else {
            continue;
        };
        if name.is_empty() || name.starts_with('"') {
            continue;
        }
        candidates.push(Candidate::new(name, "revset alias"));
    }
    (!candidates.is_empty()).then_some(candidates)
}

/// named templates for `-T`/`--template`. jj stores builtins and user
/// templates alike under `template-aliases.*`; parameterised ones come
/// through quoted (`"format_short_id(id)"`) and are skipped, leaving the
/// same nullary set jj's own completer offers.
fn jj_templates(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run_quiet(
        &[
            "jj".into(),
            "config".into(),
            "list".into(),
            "--include-defaults".into(),
            "template-aliases".into(),
            "-T".into(),
            r#"name ++ "\n""#.into(),
        ],
        ctx.ms_left(),
    )?;
    let mut seen = std::collections::HashSet::new();
    let mut candidates = Vec::new();
    for line in out.lines() {
        let Some(name) = line.trim().strip_prefix("template-aliases.") else {
            continue;
        };
        if name.is_empty() || name.starts_with('"') {
            continue;
        }
        if seen.insert(name.to_string()) {
            candidates.push(Candidate::new(name, "template"));
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}

/// mirrors jj's `split_revset_trailing_name`: locate where the trailing
/// symbol of a compound revset starts so candidates can be re-prefixed
/// with everything before it. returns `(prefix, trailing_symbol)`; when
/// the tail doesn't look like a symbol the whole string is the symbol.
fn split_revset_trailing_name(s: &str) -> (&str, &str) {
    let after_op = s
        .rsplit_once([':', '~', '|', '&', '(', ','])
        .map(|(_, rest)| rest)
        .unwrap_or(s);
    let after_range = after_op
        .rsplit_once("..")
        .map(|(_, rest)| rest)
        .unwrap_or(after_op);
    let tail = after_range.trim_start();
    if is_revset_symbol_prefix(tail) {
        (&s[..s.len() - tail.len()], tail)
    } else {
        ("", s)
    }
}

/// a partially-typed revset symbol: word chars, `_` and `/`, with single
/// `@ . + -` separators between (never leading, never doubled). a trailing
/// separator is allowed so `main@` completes the remote part.
fn is_revset_symbol_prefix(s: &str) -> bool {
    let is_sep = |c: char| matches!(c, '@' | '.' | '+' | '-');
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '/';
    let mut last_was_sep = true;
    for c in s.chars() {
        if is_word(c) {
            last_was_sep = false;
        } else if is_sep(c) {
            if last_was_sep {
                return false;
            }
            last_was_sep = true;
        } else {
            return false;
        }
    }
    true
}

/// local bookmarks; `revset` (`all()`/`mutable()`) scopes them by their
/// local target so `mutable()` callers don't offer immutable bookmarks.
fn jj_bookmarks(ctx: &DynCtx, revset: &str) -> Option<Vec<Candidate>> {
    let out = run_quiet(
        &[
            "jj".into(),
            "bookmark".into(),
            "list".into(),
            "-r".into(),
            revset.into(),
            "-T".into(),
            r#"name ++ "\t" ++ if(normal_target, normal_target.description().first_line(), "") ++ "\n""#
                .into(),
        ],
        ctx.ms_left(),
    )?;
    let mut seen = std::collections::HashSet::new();
    let mut candidates = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(2, '\t');
        let Some(name) = parts.next() else { continue };
        if name.is_empty() {
            continue;
        }
        let desc = parts.next().unwrap_or("");
        if seen.insert(name.to_string()) {
            candidates.push(Candidate::new(name, desc));
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}

/// remote bookmarks as `name@remote`. `tracked` filters by tracking state
/// (`Some(true)` for `untrack`, `Some(false)` for `track`, `None` for any).
/// the synthetic `@git` remote is always excluded.
fn jj_remote_bookmarks(ctx: &DynCtx, tracked: Option<bool>) -> Option<Vec<Candidate>> {
    let cond = match tracked {
        Some(true) => "&& tracked",
        Some(false) => "&& !tracked",
        None => "",
    };
    let template = format!(
        r#"if(remote && remote != "git" {cond}, name ++ "@" ++ remote ++ "\t" ++ if(normal_target, normal_target.description().first_line(), "") ++ "\n", "")"#
    );
    let out = run_quiet(
        &[
            "jj".into(),
            "bookmark".into(),
            "list".into(),
            "--all-remotes".into(),
            "-T".into(),
            template,
        ],
        ctx.ms_left(),
    )?;
    let mut seen = std::collections::HashSet::new();
    let mut candidates = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(2, '\t');
        let Some(name) = parts.next() else { continue };
        if name.is_empty() || name.ends_with("@git") {
            continue;
        }
        let desc = parts.next().unwrap_or("");
        if seen.insert(name.to_string()) {
            candidates.push(Candidate::new(name, desc));
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn jj_tags(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run_quiet(
        &[
            "jj".into(),
            "tag".into(),
            "list".into(),
            "--all-remotes".into(),
            "-T".into(),
            r#"name ++ "\n""#.into(),
        ],
        ctx.ms_left(),
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        candidates.push(Candidate::new(trimmed, "tag"));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn jj_remotes(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run_quiet(
        &["jj".into(), "git".into(), "remote".into(), "list".into()],
        ctx.ms_left(),
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some(name) = parts.first() else { continue };
        let desc = parts.get(1).copied().unwrap_or("remote");
        candidates.push(Candidate::new(*name, desc));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn jj_ops(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec![
        "jj".into(),
        "op".into(),
        "log".into(),
        "--ignore-working-copy".into(),
        "--no-graph".into(),
    ];
    args.extend(ctx.limit_args("-n"));
    args.extend(
        [
            "-T",
            r#"id.short() ++ "\t" ++ description.first_line() ++ "\n""#,
        ]
        .into_iter()
        .map(str::to_string),
    );
    let out = run_quiet(&args, ctx.ms_left())?;
    Some(parse_tabular(&out, 2, |p| Candidate::new(p[0], p[1])))
}

fn jj_files(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run_quiet(
        &[
            "jj".into(),
            "file".into(),
            "list".into(),
            "--ignore-working-copy".into(),
        ],
        ctx.ms_left(),
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        candidates.push(Candidate::new(trimmed, "repo file"));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn jj_workspaces(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run_quiet(
        &[
            "jj".into(),
            "workspace".into(),
            "list".into(),
            "-T".into(),
            r#"name ++ "\n""#.into(),
        ],
        ctx.ms_left(),
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        candidates.push(Candidate::new(trimmed, "workspace"));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn npm_like_completions(spans: &[String], _ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let cmd = spans[0].as_str();
    let sub = spans.get(1).map(String::as_str).unwrap_or("");
    let span_len = spans.len();
    let wants = (cmd == "yarn" && span_len == 2)
        || ((sub == "run" || sub == "run-script") && span_len == 3);
    if !wants {
        return None;
    }
    let contents = std::fs::read_to_string("package.json").ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let scripts = parsed.get("scripts")?.as_object()?;
    let mut candidates = Vec::new();
    for (name, value) in scripts {
        let desc = value.as_str().unwrap_or("");
        candidates.push(Candidate::new(name, desc));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn make_completions(spans: &[String], _ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if spans.len() > 2 {
        return None;
    }
    let contents = std::fs::read_to_string("Makefile").ok()?;
    let mut seen = std::collections::HashSet::new();
    let mut candidates = Vec::new();
    for line in contents.lines() {
        let Some(colon) = line.find(':') else { continue };
        let lhs = &line[..colon];
        if lhs.is_empty() || lhs.starts_with('.') {
            continue;
        }
        if !lhs
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '/' || c == '-')
        {
            continue;
        }
        if seen.insert(lhs.to_string()) {
            candidates.push(Candidate::new(lhs, ""));
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn just_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if spans.len() > 2 {
        return None;
    }
    let out = run_quiet(
        &["just".into(), "--list".into(), "--unsorted".into()],
        ctx.ms_left(),
    )?;
    let mut candidates = Vec::new();
    // first line is the "Available recipes:" header. recipe lines look
    // like `name [args]   # description`; only ASCII names accepted.
    for line in out.lines().skip(1) {
        let trimmed = line.trim_start();
        let mut iter = trimmed.split_whitespace();
        let Some(name) = iter.next() else { continue };
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            continue;
        }
        let desc = match trimmed.split_once('#') {
            Some((_, after)) => after.trim().to_string(),
            None => String::new(),
        };
        candidates.push(Candidate::new(name, desc));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn cargo_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let prev = if spans.len() >= 2 {
        spans[spans.len() - 2].as_str()
    } else {
        ""
    };
    let target_flags = ["--bin", "--example", "--test", "--bench"];
    let want_package = prev == "-p" || prev == "--package";
    let want_target = target_flags.contains(&prev);
    if !want_package && !want_target {
        return None;
    }
    let out = run_quiet(
        &[
            "cargo".into(),
            "metadata".into(),
            "--no-deps".into(),
            "--format-version".into(),
            "1".into(),
        ],
        ctx.ms_left(),
    )?;
    let parsed: serde_json::Value = serde_json::from_str(&out).ok()?;
    let packages = parsed.get("packages")?.as_array()?;
    let mut seen = std::collections::HashSet::new();
    let mut candidates = Vec::new();
    if want_package {
        for pkg in packages {
            let name = pkg.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let version = pkg.get("version").and_then(|v| v.as_str()).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            if seen.insert(name.to_string()) {
                candidates.push(Candidate::new(name, version));
            }
        }
    } else {
        let kind = prev.trim_start_matches('-');
        for pkg in packages {
            let Some(targets) = pkg.get("targets").and_then(|v| v.as_array()) else {
                continue;
            };
            for t in targets {
                let kinds = match t.get("kind").and_then(|v| v.as_array()) {
                    Some(a) => a,
                    None => continue,
                };
                let kind_strs: Vec<&str> = kinds.iter().filter_map(|v| v.as_str()).collect();
                if !kind_strs.contains(&kind) {
                    continue;
                }
                let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                if seen.insert(name.to_string()) {
                    candidates.push(Candidate::new(name, kind_strs.join(",")));
                }
            }
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn kill_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let by_pid = spans[0] == "kill";
    let out = run_quiet(
        &[
            "ps".into(),
            "-eo".into(),
            "pid,comm".into(),
            "--no-headers".into(),
        ],
        ctx.ms_left(),
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim();
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let pid = parts[0];
        let comm = parts[1..].join(" ");
        if by_pid {
            candidates.push(Candidate::new(pid, comm));
        } else {
            candidates.push(Candidate::new(comm, pid));
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}

/// shared tab-split helper for `git for-each-ref` and `jj` template
/// outputs. drops lines with fewer than `min_parts` columns.
fn parse_tabular<F>(out: &str, min_parts: usize, mk: F) -> Vec<Candidate>
where
    F: Fn(&[&str]) -> Candidate,
{
    let mut candidates = Vec::new();
    for line in out.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < min_parts {
            continue;
        }
        candidates.push(mk(&parts));
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn unknown_command_returns_none() {
        let cfg = cfg();
        assert!(dynamic_complete(&["zzznotacommand".into()], &cfg).is_none());
    }

    #[test]
    fn empty_spans_returns_none() {
        let cfg = cfg();
        assert!(dynamic_complete(&[], &cfg).is_none());
    }

    #[test]
    fn nix_too_few_spans_returns_none() {
        let cfg = cfg();
        assert!(dynamic_complete(&["nix".into()], &cfg).is_none());
    }

    #[test]
    fn fuzzy_filter_drops_exact_subcommand_matches() {
        let items = vec![Candidate::new("show", "remote subcommand")];
        assert!(filter_candidates(items, "show").is_none());
    }

    #[test]
    fn fuzzy_filter_keeps_exact_when_description_does_not_mark_subcommand() {
        let items = vec![Candidate::new("show", "shows things")];
        let filtered = filter_candidates(items, "show").expect("non-empty");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].value, "show");
    }

    #[test]
    fn fuzzy_filter_empty_prefix_keeps_all() {
        let items = vec![
            Candidate::new("first", "a"),
            Candidate::new("second", "b"),
        ];
        let filtered = filter_candidates(items, "").expect("non-empty");
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn kubectl_scope_parses_namespace_flag() {
        let spans = vec![
            "kubectl".into(),
            "get".into(),
            "pods".into(),
            "-n".into(),
            "prod".into(),
            "".into(),
        ];
        let scope = kubectl_scope(&spans);
        assert!(!scope.all);
        assert_eq!(scope.args, vec!["-n".to_string(), "prod".to_string()]);
    }

    #[test]
    fn kubectl_scope_detects_all_namespaces() {
        let spans = vec![
            "kubectl".into(),
            "get".into(),
            "pods".into(),
            "-A".into(),
            "".into(),
        ];
        let scope = kubectl_scope(&spans);
        assert!(scope.all);
    }

    #[test]
    fn revset_split_plain_symbol_has_no_prefix() {
        assert_eq!(split_revset_trailing_name("main"), ("", "main"));
        assert_eq!(split_revset_trailing_name(""), ("", ""));
    }

    #[test]
    fn revset_split_compound_keeps_prefix() {
        assert_eq!(split_revset_trailing_name("main & dev"), ("main & ", "dev"));
        assert_eq!(split_revset_trailing_name("ancestors("), ("ancestors(", ""));
        assert_eq!(split_revset_trailing_name("a..b"), ("a..", "b"));
        assert_eq!(split_revset_trailing_name("x|y~z"), ("x|y~", "z"));
    }

    #[test]
    fn revset_symbol_prefix_accepts_remote_and_rejects_leading_sep() {
        assert!(is_revset_symbol_prefix(""));
        assert!(is_revset_symbol_prefix("main"));
        assert!(is_revset_symbol_prefix("main@"));
        assert!(is_revset_symbol_prefix("main@origin"));
        assert!(is_revset_symbol_prefix("feature/x"));
        assert!(!is_revset_symbol_prefix("@"));
        assert!(!is_revset_symbol_prefix("a@@b"));
        assert!(!is_revset_symbol_prefix("a b"));
    }

    #[test]
    fn flake_pkg_pattern_matches_flake_pkg() {
        assert!(looks_like_flake_pkg("nixpkgs#hello"));
        assert!(!looks_like_flake_pkg("hello"));
        assert!(!looks_like_flake_pkg("#hello"));
        assert!(!looks_like_flake_pkg("nixpkgs#"));
    }
}
