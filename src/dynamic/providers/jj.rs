// SPDX-License-Identifier: EUPL-1.2
use super::super::shared::{Candidate, DynCtx, parse_tabular, run};
const JJ_TOP_VERBS: &[&str] = &[
    "abandon",
    "absorb",
    "bookmark",
    "commit",
    "config",
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
    "desc",
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
    "absorb", "ci", "commit", "diff", "diffedit", "fix", "log", "restore", "split", "squash",
];
// a bare positional is a revision the verb rewrites
const JJ_MUTABLE_POS_VERBS: &[&str] = &[
    "abandon",
    "desc",
    "describe",
    "edit",
    "metaedit",
    "parallelize",
];
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
const JJ_OP_VERBS: &[&str] = &[
    "abandon",
    "diff",
    "integrate",
    "log",
    "restore",
    "revert",
    "show",
];
const JJ_FILE_VERBS: &[&str] = &[
    "annotate", "chmod", "list", "search", "show", "track", "untrack",
];
const JJ_WORKSPACE_VERBS: &[&str] = &["add", "forget", "list", "rename", "root", "update-stale"];
const JJ_SPARSE_VERBS: &[&str] = &["edit", "list", "reset", "set"];

pub(super) fn complete(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
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
                out.push(Candidate::new(
                    format!("{prepend}{}", c.value),
                    c.description,
                ));
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

fn jj_change_ids(ctx: &DynCtx, revset: &str) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec![
        ctx.bin("jj"),
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
    let out = run(&args, ctx)?;
    Some(parse_tabular(&out, 2, |p| Candidate::new(p[0], p[1])))
}

/// config option names for `config get`/`set`/`list`/`unset`. `leaves_only`
/// keeps just settable leaf keys (for get/set/unset), `list` also wants the
/// intermediate table prefixes, derived here. `set_only` drops defaults so
/// `unset` offers only keys actually present in a config file.
fn jj_config_keys(ctx: &DynCtx, leaves_only: bool, set_only: bool) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec![ctx.bin("jj"), "config".into(), "list".into()];
    if !set_only {
        args.push("--include-defaults".into());
    }
    args.push("-T".into());
    args.push(r#"name ++ "\n""#.into());
    let out = run(&args, ctx)?;
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
    let out = run(&[ctx.bin("jj"), "resolve".into(), "--list".into()], ctx)?;
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
    let out = run(
        &[
            ctx.bin("jj"),
            "config".into(),
            "list".into(),
            "--include-defaults".into(),
            "revset-aliases".into(),
            "-T".into(),
            r#"name ++ "\n""#.into(),
        ],
        ctx,
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
    let out = run(
        &[
            ctx.bin("jj"),
            "config".into(),
            "list".into(),
            "--include-defaults".into(),
            "template-aliases".into(),
            "-T".into(),
            r#"name ++ "\n""#.into(),
        ],
        ctx,
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
pub(crate) fn split_revset_trailing_name(s: &str) -> (&str, &str) {
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
pub(crate) fn is_revset_symbol_prefix(s: &str) -> bool {
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
    let out = run(
        &[
            ctx.bin("jj"),
            "bookmark".into(),
            "list".into(),
            "-r".into(),
            revset.into(),
            "-T".into(),
            r#"name ++ "\t" ++ if(normal_target, normal_target.description().first_line(), "") ++ "\n""#
                .into(),
        ],
        ctx,
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
    let out = run(
        &[
            ctx.bin("jj"),
            "bookmark".into(),
            "list".into(),
            "--all-remotes".into(),
            "-T".into(),
            template,
        ],
        ctx,
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
    let out = run(
        &[
            ctx.bin("jj"),
            "tag".into(),
            "list".into(),
            "--all-remotes".into(),
            "-T".into(),
            r#"name ++ "\n""#.into(),
        ],
        ctx,
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
    let out = run(
        &[ctx.bin("jj"), "git".into(), "remote".into(), "list".into()],
        ctx,
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
        ctx.bin("jj"),
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
    let out = run(&args, ctx)?;
    Some(parse_tabular(&out, 2, |p| Candidate::new(p[0], p[1])))
}

fn jj_files(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run(
        &[
            ctx.bin("jj"),
            "file".into(),
            "list".into(),
            "--ignore-working-copy".into(),
        ],
        ctx,
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
    let out = run(
        &[
            ctx.bin("jj"),
            "workspace".into(),
            "list".into(),
            "-T".into(),
            r#"name ++ "\n""#.into(),
        ],
        ctx,
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
