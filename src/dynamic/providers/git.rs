// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::Candidate,
    dynamic::shared::{
        DynCtx,
        Words,
        parse_tabular,
        run,
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
    let prefix = words.current();
    match git_completion(words, ctx)? {
        GitCompletion::Static {
            values,
            description,
        } => Some(static_candidates(values, description)),
        GitCompletion::Refs => git_refs(&git, ctx, prefix),
        GitCompletion::PushDestinations(remote) => git_push_destinations(&git, ctx, remote, prefix),
        GitCompletion::RemoteBranches(remote) => git_remote_branches(&git, ctx, remote, prefix),
        GitCompletion::Branches => git_branches(&git, ctx, prefix),
        GitCompletion::Tags => git_tags(&git, ctx, prefix),
        GitCompletion::Remotes => git_remotes(&git, ctx),
        GitCompletion::Stashes => git_stashes(&git, ctx, prefix),
        GitCompletion::StatusPaths => git_status_paths(&git, ctx),
        GitCompletion::TrackedPaths => git_tracked_paths(&git, ctx),
        GitCompletion::Submodules => git_submodules(&git, ctx),
        GitCompletion::Worktrees => git_worktrees(&git, ctx),
    }
}

#[derive(Clone, Copy)]
enum GitCompletion<'span> {
    Static {
        values:      &'static [&'static str],
        description: &'static str,
    },
    Refs,
    PushDestinations(&'span str),
    RemoteBranches(&'span str),
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

fn git_completion<'span>(words: &Words<'span>, ctx: &DynCtx) -> Option<GitCompletion<'span>> {
    let sub = words.arg(0);
    let prev = ctx.slot.value_flag().unwrap_or("");
    let args_after_sub = words.arg_count().saturating_sub(1);
    let boundary = words.past_dashes();

    if sub == "restore" && (prev == "--source" || prev == "-s") {
        return Some(GitCompletion::Refs);
    }
    if sub == "push" && prev == "--repo" {
        return Some(GitCompletion::Remotes);
    }
    if !words.at_operand(ctx) {
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
        "fetch" if words.has(&["--all"]) => None,
        "fetch" if words.has(&["-m", "--multiple"]) => Some(GitCompletion::Remotes),
        "fetch" | "push" | "pull" if args_after_sub == 0 && words.value(&["--repo"]).is_none() => {
            Some(GitCompletion::Remotes)
        },
        "push" if words.current().contains(':') => {
            let remote = words.value(&["--repo"]).unwrap_or_else(|| words.arg(1));
            Some(GitCompletion::PushDestinations(remote))
        },
        "fetch" | "pull" => {
            let remote = words.value(&["--repo"]).unwrap_or_else(|| words.arg(1));
            Some(GitCompletion::RemoteBranches(remote))
        },
        "push" => Some(GitCompletion::Refs),
        "tag" if boundary => None,
        "tag" if words.has(&["-d", "--delete", "-v", "--verify"]) => Some(GitCompletion::Tags),
        "tag" if args_after_sub >= 1 => Some(GitCompletion::Refs),
        "tag" => Some(GitCompletion::Tags),
        "add" | "restore" => Some(GitCompletion::StatusPaths),
        "rm" => Some(GitCompletion::TrackedPaths),
        "mv" => (args_after_sub == 0).then_some(GitCompletion::TrackedPaths),
        "checkout" if ["-b", "-B", "--orphan"].contains(&prev) => None,
        "switch" if ["-c", "-C", "--create", "--force-create", "--orphan"].contains(&prev) => None,
        // a revision may precede the file, and files are offered alongside
        "blame" => (args_after_sub == 0 && !boundary).then_some(GitCompletion::Refs),
        _ if GIT_BRANCH_VERBS.contains(&sub) => Some(GitCompletion::Branches),
        _ if GIT_REF_VERBS.contains(&sub) => (!boundary).then_some(GitCompletion::Refs),
        _ => None,
    }
}

fn nested_verb_completion<'span>(sub: &str, words: &Words<'span>) -> Option<GitCompletion<'span>> {
    let verb = words.arg(1);
    let index = words.arg_count().saturating_sub(2);
    match (sub, verb, index) {
        ("worktree", "remove" | "move" | "lock" | "unlock", 0) => Some(GitCompletion::Worktrees),
        ("worktree", "add", 1..) => Some(GitCompletion::Refs),
        ("remote", "set-head", 1) | ("remote", "set-branches", 1..) => {
            Some(GitCompletion::RemoteBranches(words.arg(2)))
        },
        (
            "remote",
            "remove" | "rm" | "rename" | "set-head" | "set-branches" | "get-url" | "set-url"
            | "show" | "prune",
            0,
        )
        | ("remote", "update", _) => Some(GitCompletion::Remotes),
        ("stash", "show" | "drop" | "pop" | "apply", 0) | ("stash", "branch", 1) => {
            Some(GitCompletion::Stashes)
        },
        ("submodule", ..) if GIT_SUBMODULE_PATH_VERBS.contains(&verb) => {
            Some(GitCompletion::Submodules)
        },
        ("bisect", "bad" | "good" | "new" | "old" | "skip" | "reset" | "start", _)
            if !words.past_dashes() =>
        {
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

fn git_remote_branches(
    git: &[String],
    ctx: &DynCtx,
    remote: &str,
    prefix: &str,
) -> Option<Vec<Candidate>> {
    if remote.is_empty() {
        return None;
    }
    let mut args = command(git, &["for-each-ref"]);
    args.extend(ctx.limit_args("--count", prefix));
    args.push("--format=%(refname:lstrip=3)%09%(contents:subject)".into());
    args.push(format!("refs/remotes/{remote}"));
    let out = run(&args, ctx)?;
    let branches = parse_tabular(&out, |parts| {
        match parts {
            [name, subject, ..] if *name != "HEAD" => Some(Candidate::new(*name, *subject)),
            _ => None,
        }
    });
    (!branches.is_empty()).then_some(branches)
}

fn git_push_destinations(
    git: &[String],
    ctx: &DynCtx,
    remote: &str,
    prefix: &str,
) -> Option<Vec<Candidate>> {
    let (source, typed) = prefix.rsplit_once(':')?;
    let branches = git_remote_branches(git, ctx, remote, typed)?;
    Some(
        branches
            .into_iter()
            .map(|branch| Candidate::new(format!("{source}:{}", branch.value), branch.description))
            .collect(),
    )
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

fn repo_location(git: &[String], ctx: &DynCtx) -> Option<(String, String)> {
    let out = run(
        &command(git, &["rev-parse", "--show-toplevel", "--show-prefix"]),
        ctx,
    )?;
    let mut lines = out.lines();
    let top = lines.next()?.to_owned();
    let prefix = lines.next().unwrap_or("").to_owned();
    Some((top, prefix))
}

fn relative_to_cwd(path: &str, prefix: &str) -> String {
    let cwd: Vec<&str> = prefix.split('/').filter(|part| !part.is_empty()).collect();
    let parts: Vec<&str> = path.split('/').collect();
    let directories = &parts[..parts.len() - 1];
    let common = cwd
        .iter()
        .zip(directories)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = "../".repeat(cwd.len() - common);
    relative.push_str(&parts[common..].join("/"));
    relative
}

fn git_status_paths(git: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let prefix = repo_location(git, ctx)
        .map(|(_, prefix)| prefix)
        .unwrap_or_default();
    let out = run(
        &command(git, &["status", "--porcelain=v1", "-z", "-uall"]),
        ctx,
    )?;
    let mut candidates = Vec::new();
    let mut entries = out.split('\0');
    while let Some(entry) = entries.next() {
        let Some(path) = entry.get(3..).filter(|path| !path.is_empty()) else {
            continue;
        };
        if entry.get(..2).is_some_and(|xy| xy.contains(['R', 'C'])) {
            entries.next();
        }
        candidates.push(Candidate::new(
            relative_to_cwd(path, &prefix),
            "changed path",
        ));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn git_tracked_paths(git: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let out = run(&command(git, &["ls-files", "-z"]), ctx)?;
    let mut candidates = Vec::new();
    for path in out.split('\0') {
        if path.is_empty() {
            continue;
        }
        candidates.push(Candidate::new(path, "tracked file"));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn git_submodules(git: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let (top, prefix) = repo_location(git, ctx)?;
    let gitmodules = format!("{top}/.gitmodules");
    let out = run(
        &command(git, &[
            "config",
            "--file",
            &gitmodules,
            "--null",
            "--get-regexp",
            "^submodule\\..*\\.path$",
        ]),
        ctx,
    )?;
    let mut candidates = Vec::new();
    for record in out.split('\0') {
        let Some((_, path)) = record.split_once('\n') else {
            continue;
        };
        candidates.push(Candidate::new(relative_to_cwd(path, &prefix), "submodule"));
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
