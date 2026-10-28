// SPDX-License-Identifier: EUPL-1.2
use crate::{
    complete::Candidate,
    dynamic::shared::{
        DynCtx,
        Words,
        run,
    },
};

fn scope_args(words: &Words, plain: &[&str]) -> Vec<String> {
    let mut args = Vec::new();
    for (long, names) in [
        ("--host", ["-H", "--host"]),
        ("--machine", ["-M", "--machine"]),
    ] {
        if let Some(value) = words.value(&names) {
            args.extend([long.to_owned(), value.to_owned()]);
        }
    }
    args.extend(
        plain
            .iter()
            .filter(|flag| words.has(&[flag]))
            .map(|flag| (*flag).to_owned()),
    );
    args
}

pub(super) fn complete(words: &Words, ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
    match words.command() {
        "journalctl" => journalctl_completions(words, ctx),
        "coredumpctl" => coredumpctl_completions(words, ctx),
        _ if !words.at_operand(ctx) => None,
        "systemctl" => systemctl_completions(words, ctx),
        "loginctl" => loginctl_completions(words, ctx),
        "machinectl" => machinectl_completions(words, ctx),
        "networkctl" => networkctl_completions(words, ctx),
        _ => None,
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
    "try-reload-or-restart",
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

fn systemctl_completions(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let verb = words.arg(0);
    if !SYSTEMCTL_UNIT_VERBS.contains(&verb) || (verb == "isolate" && words.arg_count() > 1) {
        return None;
    }
    unit_candidates(
        &scope_args(words, &["--user", "--system"]),
        words.current(),
        ctx,
    )
}

fn unit_candidates(scope: &[String], prefix: &str, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let mut args: Vec<String> = vec![ctx.bin("systemctl")];
    args.extend(scope.iter().cloned());
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
        .map(str::to_owned),
    );
    args.push(format!("{prefix}*"));
    let out = run(&args, ctx)?;
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

fn journalctl_completions(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let prev = words.value_slot(ctx)?;
    if !matches!(prev, "-u" | "--unit" | "--user-unit") {
        return None;
    }
    let mut scope = scope_args(words, &["--user"]);
    if words.has(&["--user-unit"]) && !words.has(&["--user"]) {
        scope.push("--user".into());
    }
    unit_candidates(&scope, words.current(), ctx)
}

fn coredumpctl_completions(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let unit_verbs = ["dump", "info", "debug", "list"];
    if !unit_verbs.contains(&words.arg(0)) {
        return None;
    }
    let last = words.current();
    // units are valid only as the `-u`/`--unit` value; the bare MATCH
    // positional takes a PID, COMM, or exe path. only offer units for `-u` so
    // they don't leak into the positional where they can never match a recorded
    // coredump.
    if let Some("-u" | "--unit") = words.value_slot(ctx) {
        return unit_candidates(&[], last, ctx);
    }
    if !words.at_operand(ctx) {
        return None;
    }
    let mut out: Vec<Candidate> = Vec::new();

    let mut args: Vec<String> = vec![ctx.bin("coredumpctl"), "list".into()];
    args.extend(ctx.limit_args("-n", last));
    args.extend(["--no-pager", "--no-legend"].into_iter().map(str::to_owned));
    if let Some(text) = run(&args, ctx) {
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

fn loginctl_completions(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
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
    let sub = words.arg(0);
    if sub == "activate" && words.arg_count() > 1 {
        return None;
    }
    let (list, min_fields) = if user_verbs.contains(&sub) {
        ("list-users", 2)
    } else if session_verbs.contains(&sub) {
        ("list-sessions", 3)
    } else {
        return None;
    };
    let mut args = vec![ctx.bin("loginctl")];
    args.extend(scope_args(words, &[]));
    args.extend([list.into(), "--no-pager".into(), "--no-legend".into()]);
    let out = run(&args, ctx)?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < min_fields {
            continue;
        }
        candidates.push(if list == "list-users" {
            Candidate::new(parts[1], format!("UID {}", parts[0]))
        } else {
            Candidate::new(parts[0], format!("user {}", parts[2]))
        });
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn machinectl_completions(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    const MACHINE_VERBS: &[&str] = &[
        "status",
        "show",
        "poweroff",
        "reboot",
        "pause",
        "resume",
        "terminate",
        "kill",
    ];
    const SINGLE_MACHINE_VERBS: &[&str] = &["login", "shell", "bind", "copy-to", "copy-from"];
    const IMAGE_VERBS: &[&str] = &[
        "start",
        "enable",
        "disable",
        "image-status",
        "show-image",
        "edit",
        "cat",
        "remove",
    ];
    const SINGLE_IMAGE_VERBS: &[&str] = &["clone", "rename", "read-only"];
    let sub = words.arg(0);
    let images = IMAGE_VERBS.contains(&sub) || SINGLE_IMAGE_VERBS.contains(&sub);
    let single = SINGLE_MACHINE_VERBS.contains(&sub) || SINGLE_IMAGE_VERBS.contains(&sub);
    if !(images || single || MACHINE_VERBS.contains(&sub)) || (single && words.arg_count() > 1) {
        return None;
    }
    let mut args = vec![ctx.bin("machinectl")];
    args.extend(scope_args(words, &["--system", "--user"]));
    args.extend([
        if images { "list-images" } else { "list" }.into(),
        "--no-pager".into(),
        "--no-legend".into(),
    ]);
    let out = run(&args, ctx)?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let Some(name) = parts.first() else { continue };
        let desc = parts.get(1).copied().unwrap_or("");
        candidates.push(Candidate::new(*name, desc));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn networkctl_completions(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
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
    if !verbs.contains(&words.arg(0)) {
        return None;
    }
    let out = run(
        &[
            ctx.bin("networkctl"),
            "list".into(),
            "--no-pager".into(),
            "--no-legend".into(),
        ],
        ctx,
    )?;
    let mut candidates = Vec::new();
    for line in out.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 4 {
            continue;
        }
        candidates.push(Candidate::new(
            parts[1],
            format!("{} {}", parts[2], parts[3]),
        ));
    }
    (!candidates.is_empty()).then_some(candidates)
}
