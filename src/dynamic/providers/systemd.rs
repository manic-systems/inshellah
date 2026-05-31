// SPDX-License-Identifier: EUPL-1.2
use super::super::shared::{Candidate, DynCtx, run};

pub(super) fn complete(spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
    match spans[0].as_str() {
        "systemctl" => systemctl_completions(spans, ctx),
        "journalctl" => journalctl_completions(spans, ctx),
        "coredumpctl" => coredumpctl_completions(spans, ctx),
        "loginctl" => loginctl_completions(spans, ctx),
        "machinectl" => machinectl_completions(spans, ctx),
        "networkctl" => networkctl_completions(spans, ctx),
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
    let mut args: Vec<String> = vec![ctx.bin("systemctl")];
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
    let user_scope = spans.iter().any(|s| s == "--user-unit" || s == "--user");
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

    let mut args: Vec<String> = vec![ctx.bin("coredumpctl"), "list".into()];
    args.extend(ctx.limit_args("-n"));
    args.extend(
        ["--no-pager", "--no-legend"]
            .into_iter()
            .map(str::to_string),
    );
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
        let out = run(
            &[
                ctx.bin("loginctl"),
                "list-users".into(),
                "--no-pager".into(),
                "--no-legend".into(),
            ],
            ctx,
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
        let out = run(
            &[
                ctx.bin("loginctl"),
                "list-sessions".into(),
                "--no-pager".into(),
                "--no-legend".into(),
            ],
            ctx,
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
        "status",
        "show",
        "start",
        "login",
        "shell",
        "enable",
        "disable",
        "poweroff",
        "reboot",
        "terminate",
        "kill",
        "bind",
        "copy-to",
        "copy-from",
    ];
    let sub = spans.get(1).map(String::as_str).unwrap_or("");
    if !verbs.contains(&sub) || spans.len() < 3 {
        return None;
    }
    let out = run(
        &[
            ctx.bin("machinectl"),
            "list".into(),
            "--no-pager".into(),
            "--no-legend".into(),
        ],
        ctx,
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
