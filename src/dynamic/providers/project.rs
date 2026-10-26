// SPDX-License-Identifier: EUPL-1.2
use std::collections::HashSet;
use std::fmt;
use std::fs;

use serde_core::de::{Deserialize, Deserializer, IgnoredAny, MapAccess, Visitor};

use crate::complete::Candidate;
use crate::dynamic::shared::{DynCtx, run};

pub(super) fn complete(spans: &[String], ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
    match spans[0].as_str() {
        "cargo" => cargo_completions(ctx),
        _ if !ctx.slot.positional() => None,
        "npm" | "pnpm" | "yarn" => npm_like_completions(spans, ctx),
        "make" => make_completions(spans, ctx),
        "just" => just_completions(spans, ctx),
        "zig" => zig_completions(spans, ctx),
        _ => None,
    }
}

fn npm_like_completions(spans: &[String], _ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let cmd = spans[0].as_str();
    let sub = spans.get(1).map_or("", String::as_str);
    let span_len = spans.len();
    let wants = (cmd == "yarn" && span_len == 2)
        || ((sub == "run" || sub == "run-script") && span_len == 3);
    if !wants {
        return None;
    }
    let contents = fs::read_to_string("package.json").ok()?;
    let Package(scripts) = serde_json::from_str(&contents).ok()?;
    let Scripts(candidates) = scripts?;
    (!candidates.is_empty()).then_some(candidates)
}

// serde_json's `Value` sorts object keys, and scripts list in package.json order
struct Package(Option<Scripts>);

struct Scripts(Vec<Candidate>);

impl<'de> Deserialize<'de> for Package {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PackageVisitor;

        impl<'de> Visitor<'de> for PackageVisitor {
            type Value = Package;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a package.json object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Package, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut scripts = None;
                while let Some(key) = map.next_key::<String>()? {
                    if key == "scripts" {
                        scripts = Some(map.next_value()?);
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                Ok(Package(scripts))
            }
        }

        deserializer.deserialize_map(PackageVisitor)
    }
}

impl<'de> Deserialize<'de> for Scripts {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ScriptsVisitor;

        impl<'de> Visitor<'de> for ScriptsVisitor {
            type Value = Scripts;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object of scripts")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Scripts, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut candidates = Vec::new();
                while let Some((name, command)) = map.next_entry::<String, serde_json::Value>()? {
                    candidates.push(Candidate::new(name, command.as_str().unwrap_or("")));
                }
                Ok(Scripts(candidates))
            }
        }

        deserializer.deserialize_map(ScriptsVisitor)
    }
}

fn make_completions(spans: &[String], _ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if spans.len() > 2 {
        return None;
    }
    let contents = fs::read_to_string("Makefile").ok()?;
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for line in contents.lines() {
        let Some((lhs, _)) = line.split_once(':') else {
            continue;
        };
        if lhs.is_empty() || lhs.starts_with('.') {
            continue;
        }
        if !lhs.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '/' || ch == '-'
        }) {
            continue;
        }
        if seen.insert(lhs.to_owned()) {
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
    // skip the "Available recipes:" header. lines are `name [args]  # desc`,
    // ASCII names only.
    for line in out.lines().skip(1) {
        let trimmed = line.trim_start();
        let mut iter = trimmed.split_whitespace();
        let Some(name) = iter.next() else { continue };
        if !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        {
            continue;
        }
        let desc = match trimmed.split_once('#') {
            Some((_, after)) => after.trim().to_owned(),
            None => String::new(),
        };
        candidates.push(Candidate::new(name, desc));
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn cargo_completions(ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let prev = ctx.slot.value_flag().unwrap_or("");
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
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    if want_package {
        for pkg in packages {
            let name = pkg
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            let version = pkg
                .get("version")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if name.is_empty() {
                continue;
            }
            if seen.insert(name.to_owned()) {
                candidates.push(Candidate::new(name, version));
            }
        }
    } else {
        let kind = prev.trim_start_matches('-');
        for pkg in packages {
            let Some(targets) = pkg.get("targets").and_then(|value| value.as_array()) else {
                continue;
            };
            for target in targets {
                let Some(kinds) = target.get("kind").and_then(|value| value.as_array()) else {
                    continue;
                };
                let kind_strs: Vec<&str> =
                    kinds.iter().filter_map(|value| value.as_str()).collect();
                if !kind_strs.contains(&kind) {
                    continue;
                }
                let name = target
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                if seen.insert(name.to_owned()) {
                    candidates.push(Candidate::new(name, kind_strs.join(",")));
                }
            }
        }
    }
    (!candidates.is_empty()).then_some(candidates)
}

fn zig_completions(spans: &[String], ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if spans.get(1).map(String::as_str) != Some("build") {
        return None;
    }

    let out = run(
        &[ctx.bin("zig"), "build".into(), "--list-steps".into()],
        ctx,
    )?;

    let mut candidates = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim_start();
        let mut iter = trimmed.split_whitespace();
        let name = iter.next()?;
        candidates.push(Candidate::new(name, iter.collect::<Vec<&str>>().join(" ")));
    }

    (!candidates.is_empty()).then_some(candidates)
}
