// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::Candidate,
    dynamic::{
        Slot,
        shared::{
            DynCtx,
            Words,
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

pub(super) fn complete(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut git = vec![ctx.bin("git")];
    for flag in ["-C", "--git-dir", "--work-tree"] {
        if let Some(value) = words.global_value(&[flag]) {
            git.extend([flag.to_owned(), value.to_owned()]);
        }
    }
    match git_completion(words, ctx.slot)? {
        GitCompletion::Static {
            values,
            description,
        } => Some(static_candidates(values, description)),
        GitCompletion::Refs => git_refs(&git, ctx, words.current()),
        GitCompletion::Branches => git_branches(&git, ctx, words.current()),
        GitCompletion::Tags => git_tags(&git, ctx, words.current()),
        GitCompletion::Remotes => git_remotes(&git, ctx),
        GitCompletion::Stashes => git_stashes(&git, ctx, words.current()),
        GitCompletion::StatusPaths => git_status_paths(&git, ctx),
        GitCompletion::TrackedPaths => git_tracked_paths(&git, ctx),
        GitCompletion::Submodules => git_submodules(&git, ctx),
        GitCompletion::Worktrees => git_worktrees(&git, ctx),
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

fn git_completion(words: &Words, slot: Slot) -> Option<GitCompletion> {
    let sub = words.arg(0);
    let prev = slot.value_flag().unwrap_or("");
    let args_after_sub = words.arg_count().saturating_sub(1);

    if sub == "restore" && (prev == "--source" || prev == "-s") {
        return Some(GitCompletion::Refs);
    }
    if !slot.positional() {
        return None;
    }
    if words.arg_count() == 0 {
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
        if args_after_sub == 0 {
            return Some(GitCompletion::Static {
                values,
                description,
            });
        }
        return nested_verb_completion(sub, words);
    }

    match sub {
        "fetch" | "push" | "pull" if args_after_sub == 0 => Some(GitCompletion::Remotes),
        "fetch" | "push" | "pull" => Some(GitCompletion::Refs),
        "tag" if words.has(&["-d", "--delete", "-v", "--verify"]) => Some(GitCompletion::Tags),
        "tag" if args_after_sub >= 1 => Some(GitCompletion::Refs),
        "tag" => Some(GitCompletion::Tags),
        "add" | "restore" => Some(GitCompletion::StatusPaths),
        "rm" => Some(GitCompletion::TrackedPaths),
        "mv" => (args_after_sub == 0).then_some(GitCompletion::TrackedPaths),
        "checkout" if ["-b", "-B", "--orphan"].contains(&prev) => None,
        "switch" if ["-c", "-C", "--create", "--force-create", "--orphan"].contains(&prev) => None,
        "switch" => Some(GitCompletion::Branches),
        _ if GIT_BRANCH_VERBS.contains(&sub) => Some(GitCompletion::Branches),
        _ if GIT_REF_VERBS.contains(&sub) => Some(GitCompletion::Refs),
        _ => None,
    }
}

fn nested_verb_completion(sub: &str, words: &Words) -> Option<GitCompletion> {
    let verb = words.arg(1);
    match sub {
        "worktree" if ["remove", "move", "lock", "unlock", "repair"].contains(&verb) => {
            Some(GitCompletion::Worktrees)
        },
        "worktree" if verb == "add" && words.arg_count() >= 3 => Some(GitCompletion::Refs),
        "remote" if GIT_REMOTE_VERBS.contains(&verb) && verb != "add" => {
            Some(GitCompletion::Remotes)
        },
        "stash"
            if ["show", "drop", "pop", "apply", "store"].contains(&verb)
                || (verb == "branch" && words.arg_count() >= 3) =>
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

fn command(git: &[String], args: &[&str]) -> Vec<String> {
    git.iter()
        .cloned()
        .chain(args.iter().map(|arg| (*arg).to_owned()))
        .collect()
}

fn git_refs(git: &[String], ctx: &DynCtx, prefix: &str) -> Option<Vec<Candidate>> {
    let mut args = command(git, &["for-each-ref"]);
    args.extend(ctx.limit_args("--count", prefix));
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

fn git_branches(git: &[String], ctx: &DynCtx, prefix: &str) -> Option<Vec<Candidate>> {
    let mut args = command(git, &["for-each-ref"]);
    args.extend(ctx.limit_args("--count", prefix));
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

fn git_tags(git: &[String], ctx: &DynCtx, prefix: &str) -> Option<Vec<Candidate>> {
    let mut args = command(git, &["for-each-ref"]);
    args.extend(ctx.limit_args("--count", prefix));
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

fn git_remotes(git: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run(&command(git, &["remote"]), ctx)?;
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

fn git_stashes(git: &[String], ctx: &DynCtx, prefix: &str) -> Option<Vec<Candidate>> {
    let mut args = command(git, &["stash", "list"]);
    args.extend(ctx.limit_args("-n", prefix));
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

fn git_status_paths(git: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run(&command(git, &["status", "--porcelain", "-uall"]), ctx)?;
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

fn git_tracked_paths(git: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run(&command(git, &["ls-files"]), ctx)?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        if line.is_empty() {
            continue;
        }
        candidates.push(Candidate::new(line, "tracked file"));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn git_submodules(git: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run(
        &command(git, &[
            "config",
            "--file",
            ".gitmodules",
            "--get-regexp",
            "^submodule\\..*\\.path$",
        ]),
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

fn git_worktrees(git: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run(&command(git, &["worktree", "list", "--porcelain"]), ctx)?;
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
