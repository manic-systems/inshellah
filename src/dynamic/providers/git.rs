// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::Candidate,
    dynamic::{
        Slot,
        shared::{
            DynCtx,
            parse_tabular,
            run,
        },
    },
};
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
const GIT_SUBMODULE_PATH_VERBS: &[&str] = &[
    "status",
    "init",
    "deinit",
    "update",
    "set-branch",
    "set-url",
    "summary",
    "foreach",
    "sync",
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
    match git_completion(spans, ctx.slot)? {
        GitCompletion::Static {
            values,
            description,
        } => Some(static_candidates(values, description)),
        GitCompletion::Refs => git_refs(ctx),
        GitCompletion::Branches => git_branches(ctx),
        GitCompletion::Tags => git_tags(ctx),
        GitCompletion::Remotes => git_remotes(ctx),
        GitCompletion::Stashes => git_stashes(ctx),
        GitCompletion::StatusPaths => git_status_paths(ctx),
        GitCompletion::TrackedPaths => git_tracked_paths(ctx),
        GitCompletion::Submodules => git_submodules(ctx),
        GitCompletion::Worktrees => git_worktrees(ctx),
    }
}

#[derive(Clone, Copy)]
enum GitCompletion {
    Static {
        values:      &'static [&'static str],
        description: &'static str,
    },
    Refs,
    Branches,
    Tags,
    Remotes,
    Stashes,
    StatusPaths,
    TrackedPaths,
    Submodules,
    Worktrees,
}

fn static_candidates(values: &'static [&'static str], description: &'static str) -> Vec<Candidate> {
    values
        .iter()
        .map(|value| Candidate::new(*value, description))
        .collect()
}

fn git_completion(spans: &[String], slot: Slot) -> Option<GitCompletion> {
    let span_len = spans.len();
    let sub = spans.get(1).map_or("", String::as_str);
    let prev = slot.value_flag().unwrap_or("");
    let positionals_after_sub: Vec<&str> = spans
        .iter()
        .skip(2)
        .filter(|span| !span.is_empty() && !span.starts_with('-'))
        .map(String::as_str)
        .collect();

    if sub == "restore" && (prev == "--source" || prev == "-s") {
        return Some(GitCompletion::Refs);
    }
    if !slot.positional() {
        return None;
    }
    if span_len <= 2 {
        return Some(GitCompletion::Static {
            values:      GIT_TOP_VERBS,
            description: "git subcommand",
        });
    }

    let nested = match sub {
        "worktree" => Some((GIT_WORKTREE_VERBS, "worktree subcommand")),
        "remote" => Some((GIT_REMOTE_VERBS, "remote subcommand")),
        "stash" => Some((GIT_STASH_VERBS, "stash subcommand")),
        "submodule" => Some((GIT_SUBMODULE_VERBS, "submodule subcommand")),
        "bisect" => Some((GIT_BISECT_VERBS, "bisect subcommand")),
        _ => None,
    };
    if let Some((values, description)) = nested {
        if span_len <= 3 {
            return Some(GitCompletion::Static {
                values,
                description,
            });
        }
        return nested_verb_completion(sub, spans, &positionals_after_sub);
    }
    if matches!(sub, "fetch" | "push" | "pull") && span_len >= 3 {
        if positionals_after_sub.is_empty() {
            return Some(GitCompletion::Remotes);
        }
        return Some(GitCompletion::Refs);
    }
    if sub == "tag" && span_len >= 3 {
        let delete_or_verify = ["-d", "--delete", "-v", "--verify"]
            .iter()
            .any(|flag| spans.iter().any(|span| span == flag));
        if delete_or_verify {
            return Some(GitCompletion::Tags);
        }
        if span_len >= 4 {
            return Some(GitCompletion::Refs);
        }
        return Some(GitCompletion::Tags);
    }
    if matches!(sub, "add" | "restore") && span_len >= 3 {
        return Some(GitCompletion::StatusPaths);
    }
    if sub == "rm" && span_len >= 3 {
        return Some(GitCompletion::TrackedPaths);
    }
    if sub == "mv" && span_len >= 3 {
        return positionals_after_sub
            .is_empty()
            .then_some(GitCompletion::TrackedPaths);
    }
    if sub == "checkout" && span_len >= 3 {
        if ["-b", "-B", "--orphan"].contains(&prev) {
            return None;
        }
        return Some(GitCompletion::Refs);
    }
    if sub == "switch" && span_len >= 3 {
        if ["-c", "-C", "--create", "--force-create", "--orphan"].contains(&prev) {
            return None;
        }
        return Some(GitCompletion::Branches);
    }
    if GIT_BRANCH_VERBS.contains(&sub) && span_len >= 3 {
        return Some(GitCompletion::Branches);
    }
    if GIT_REF_VERBS.contains(&sub) && span_len >= 3 {
        return Some(GitCompletion::Refs);
    }
    None
}

fn nested_verb_completion(
    sub: &str,
    spans: &[String],
    positionals_after_sub: &[&str],
) -> Option<GitCompletion> {
    let verb = spans.get(2).map_or("", String::as_str);
    match sub {
        "worktree" if ["remove", "move", "lock", "unlock", "repair"].contains(&verb) => {
            Some(GitCompletion::Worktrees)
        },
        "worktree" if verb == "add" && spans.len() >= 5 => Some(GitCompletion::Refs),
        "remote" if GIT_REMOTE_VERBS.contains(&verb) && verb != "add" => {
            Some(GitCompletion::Remotes)
        },
        "stash"
            if ["show", "drop", "pop", "apply", "store"].contains(&verb)
                || (verb == "branch" && positionals_after_sub.len() >= 2) =>
        {
            Some(GitCompletion::Stashes)
        },
        "submodule" if GIT_SUBMODULE_PATH_VERBS.contains(&verb) => Some(GitCompletion::Submodules),
        "bisect" if ["bad", "good", "new", "old", "skip", "reset", "start"].contains(&verb) => {
            Some(GitCompletion::Refs)
        },
        _ => None,
    }
}

fn git_refs(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec![ctx.bin("git"), "for-each-ref".into()];
    args.extend(ctx.limit_args("--count"));
    args.push("--format=%(refname:short)%09%(objecttype)%09%(contents:subject)".into());
    args.extend(
        ["refs/heads", "refs/remotes", "refs/tags"]
            .into_iter()
            .map(str::to_owned),
    );
    let out = run(&args, ctx)?;
    Some(parse_tabular(&out, |parts| {
        match parts {
            [name, _, subject, ..] => Some(Candidate::new(*name, *subject)),
            _ => None,
        }
    }))
}

fn git_branches(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec![ctx.bin("git"), "for-each-ref".into()];
    args.extend(ctx.limit_args("--count"));
    args.push("--format=%(refname:short)%09%(contents:subject)".into());
    args.push("refs/heads".into());
    let out = run(&args, ctx)?;
    Some(parse_tabular(&out, |parts| {
        match parts {
            [name, subject, ..] => Some(Candidate::new(*name, *subject)),
            _ => None,
        }
    }))
}

fn git_tags(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec![ctx.bin("git"), "for-each-ref".into()];
    args.extend(ctx.limit_args("--count"));
    args.push("--format=%(refname:short)%09%(contents:subject)".into());
    args.push("refs/tags".into());
    let out = run(&args, ctx)?;
    Some(parse_tabular(&out, |parts| {
        match parts {
            [name, subject, ..] => Some(Candidate::new(*name, *subject)),
            _ => None,
        }
    }))
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
        let Some((stash, rest)) = line.split_once(": ") else {
            continue;
        };
        if !stash.starts_with("stash@{") {
            continue;
        }
        let desc = rest.trim();
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
        let Some(raw) = line.get(3..) else {
            continue;
        };
        let path = if let Some((_, after)) = raw.split_once(" -> ") {
            after.to_owned()
        } else {
            raw.to_owned()
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
