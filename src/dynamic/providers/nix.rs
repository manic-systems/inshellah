// SPDX-License-Identifier: EUPL-1.2
use super::super::shared::{Candidate, DynCtx, run_with};
pub(super) fn complete(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if spans.len() < 2 {
        return None;
    }
    if ctx.budget_exhausted() {
        return None;
    }
    let nix_index = spans.len() - 1;
    let last = spans.last().map(String::as_str).unwrap_or("");
    let cmd_args = ctx.command_spans(spans);
    let raw = run_with(&cmd_args, ctx, |cmd| {
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
    // bare `nixpkgs#` makes nix return the whole attribute set (tens of
    // thousands of entries), bound it like every other provider to avoid
    // shipping and fuzzy-scoring megabytes per keystroke.
    if ctx.limit != 0 && lines.len() > ctx.limit {
        lines.truncate(ctx.limit);
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

pub(crate) fn looks_like_flake_pkg(s: &str) -> bool {
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
        ctx.bin("nix"),
        "eval".into(),
        "--raw".into(),
        "--impure".into(),
        installable.into(),
        "--apply".into(),
        "f: f.meta.description".into(),
    ];
    let out = run_with(&args, ctx, |cmd| {
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
