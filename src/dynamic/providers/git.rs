// SPDX-License-Identifier: EUPL-1.2
use super::super::shared::{Candidate, DynCtx, parse_tabular, run};
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

pub(super) fn complete(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
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
            "status",
            "init",
            "deinit",
            "update",
            "set-branch",
            "set-url",
            "summary",
            "foreach",
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
    let mut args: Vec<String> = vec![ctx.bin("git"), "for-each-ref".into()];
    args.extend(ctx.limit_args("--count"));
    args.push("--format=%(refname:short)%09%(objecttype)%09%(contents:subject)".into());
    args.extend(
        ["refs/heads", "refs/remotes", "refs/tags"]
            .into_iter()
            .map(str::to_string),
    );
    let out = run(&args, ctx)?;
    Some(parse_tabular(&out, 3, |p| Candidate::new(p[0], p[2])))
}

fn git_branches(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec![ctx.bin("git"), "for-each-ref".into()];
    args.extend(ctx.limit_args("--count"));
    args.push("--format=%(refname:short)%09%(contents:subject)".into());
    args.push("refs/heads".into());
    let out = run(&args, ctx)?;
    Some(parse_tabular(&out, 2, |p| Candidate::new(p[0], p[1])))
}

fn git_tags(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec![ctx.bin("git"), "for-each-ref".into()];
    args.extend(ctx.limit_args("--count"));
    args.push("--format=%(refname:short)%09%(contents:subject)".into());
    args.push("refs/tags".into());
    let out = run(&args, ctx)?;
    Some(parse_tabular(&out, 2, |p| Candidate::new(p[0], p[1])))
}

fn git_remotes(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run(&[ctx.bin("git"), "remote".into()], ctx)?;
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
    let mut args: Vec<String> = vec![ctx.bin("git"), "stash".into(), "list".into()];
    args.extend(ctx.limit_args("-n"));
    let out = run(&args, ctx)?;
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
    let out = run(
        &[
            ctx.bin("git"),
            "status".into(),
            "--porcelain".into(),
            "-uall".into(),
        ],
        ctx,
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
    let out = run(&[ctx.bin("git"), "ls-files".into()], ctx)?;
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
    let out = run(
        &[
            ctx.bin("git"),
            "config".into(),
            "--file".into(),
            ".gitmodules".into(),
            "--get-regexp".into(),
            "^submodule\\..*\\.path$".into(),
        ],
        ctx,
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
    let out = run(
        &[
            ctx.bin("git"),
            "worktree".into(),
            "list".into(),
            "--porcelain".into(),
        ],
        ctx,
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
