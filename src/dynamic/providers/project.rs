// SPDX-License-Identifier: EUPL-1.2
use super::super::shared::{Candidate, DynCtx, run};

pub(super) fn complete(spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
    match spans[0].as_str() {
        "npm" | "pnpm" | "yarn" => npm_like_completions(spans, ctx),
        "make" => make_completions(spans, ctx),
        "just" => just_completions(spans, ctx),
        "cargo" => cargo_completions(spans, ctx),
        _ => None,
    }
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
        let Some(colon) = line.find(':') else {
            continue;
        };
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
    let out = run(
        &[ctx.bin("just"), "--list".into(), "--unsorted".into()],
        ctx,
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
    let out = run(
        &[
            ctx.bin("cargo"),
            "metadata".into(),
            "--no-deps".into(),
            "--format-version".into(),
            "1".into(),
        ],
        ctx,
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
