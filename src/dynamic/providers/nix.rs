// SPDX-License-Identifier: EUPL-1.2
use crate::complete::Candidate;
use crate::dynamic::shared::{DynCtx, run_with};
pub(super) fn complete(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if spans.len() < 2 {
        return None;
    }
    if ctx.budget_exhausted() {
        return None;
    }
    let nix_index = spans.len() - 1;
    let last = spans.last().map_or("", String::as_str);
    let cmd_args = ctx.command_spans(spans);
    let raw = run_with(&cmd_args, ctx, |cmd| {
        cmd.env("NIX_GET_COMPLETIONS", nix_index.to_string());
    })?;
    let mut rows: Vec<Candidate> = raw
        .split('\n')
        .skip(1)
        .filter_map(parse_nix_completion_line)
        .collect();
    if rows.is_empty() {
        return None;
    }
    // bare `nixpkgs#` makes nix return the whole attribute set (tens of
    // thousands of entries), bound it like every other provider to avoid
    // shipping and fuzzy-scoring megabytes per keystroke.
    if ctx.limit != 0 && rows.len() > ctx.limit {
        rows.truncate(ctx.limit);
    }
    let enrich = rows.len() < 6 && looks_like_flake_pkg(last);
    if !enrich {
        return Some(rows);
    }
    for row in &mut rows {
        if row.description.is_empty() {
            row.description = nix_eval_description(&row.value, ctx).unwrap_or_default();
        }
    }
    Some(rows)
}

fn parse_nix_completion_line(line: &str) -> Option<Candidate> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (value, description) = trimmed
        .split_once('\t')
        .map_or((trimmed, ""), |(value, description)| {
            (value.trim(), description.trim())
        });
    (!value.is_empty()).then(|| Candidate::new(value, description))
}

pub fn looks_like_flake_pkg(word: &str) -> bool {
    let Some((lhs, rhs)) = word.split_once('#') else {
        return false;
    };
    is_flake_ident(lhs) && is_flake_ident(rhs)
}

fn is_flake_ident(ident: &str) -> bool {
    let mut chars = ident.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
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
        Some(trimmed.to_owned())
    }
}
