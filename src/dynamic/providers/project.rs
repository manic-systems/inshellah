// SPDX-License-Identifier: EUPL-1.2
use std::{
    collections::HashSet,
    fmt,
    fs,
    path::Path,
};

use serde_core::de::{
    Deserialize,
    Deserializer,
    IgnoredAny,
    MapAccess,
    Visitor,
};

use crate::{
    complete::Candidate,
    dynamic::shared::{
        DynCtx,
        Words,
        run,
    },
};

pub(super) fn complete(words: &Words, ctx: &DynCtx<'_>) -> Option<Vec<Candidate>> {
    match words.command() {
        "cargo" => cargo_completions(words, ctx),
        _ if !words.at_operand(ctx) => None,
        "npm" | "pnpm" | "yarn" => npm_like_completions(words),
        "make" => make_completions(words),
        "just" => just_completions(words, ctx),
        "zig" => zig_completions(words, ctx),
        _ => None,
    }
}

fn npm_like_completions(words: &Words) -> Option<Vec<Candidate>> {
    let wants = (words.command() == "yarn" && words.arg_count() == 0)
        || (matches!(words.arg(0), "run" | "run-script") && words.arg_count() == 1);
    if !wants {
        return None;
    }
    let dir = words
        .value(&["--prefix", "-C", "--dir", "--cwd"])
        .unwrap_or(".");
    let contents = fs::read_to_string(Path::new(dir).join("package.json")).ok()?;
    let Package(scripts) = serde_json::from_str(&contents).ok()?;
    let Scripts(candidates) = scripts?;
    (!candidates.is_empty()).then_some(candidates)
}

// serde_json's `Value` sorts object keys, and scripts list in package.json
// order
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

fn make_completions(words: &Words) -> Option<Vec<Candidate>> {
    let dir = Path::new(words.value(&["-C", "--directory"]).unwrap_or("."));
    let contents = match words.value(&["-f", "--file", "--makefile"]) {
        Some(file) => fs::read_to_string(dir.join(file)).ok()?,
        None => {
            ["GNUmakefile", "makefile", "Makefile"]
                .iter()
                .find_map(|name| fs::read_to_string(dir.join(name)).ok())?
        },
    };
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

fn just_completions(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if words.arg_count() > 0 {
        return None;
    }
    let mut args = vec![ctx.bin("just")];
    for (long, short) in [("--justfile", "-f"), ("--working-directory", "-d")] {
        if let Some(value) = words.value(&[long, short]) {
            args.extend([long.to_owned(), value.to_owned()]);
        }
    }
    args.extend(["--list".to_owned(), "--unsorted".to_owned()]);
    let out = run(&args, ctx)?;
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

fn cargo_completions(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    let prev = ctx.slot.value_flag().unwrap_or("");
    let target_flags = ["--bin", "--example", "--test", "--bench"];
    let want_package = prev == "-p" || prev == "--package";
    let want_target = target_flags.contains(&prev);
    if !want_package && !want_target {
        return None;
    }
    let mut args = vec![ctx.bin("cargo")];
    if let Some(dir) = words.value(&["-C"]) {
        args.extend(["-C".to_owned(), dir.to_owned()]);
    }
    args.extend(["metadata", "--no-deps", "--format-version", "1"].map(str::to_owned));
    if let Some(manifest) = words.value(&["--manifest-path"]) {
        args.extend(["--manifest-path".to_owned(), manifest.to_owned()]);
    }
    let out = run(&args, ctx)?;
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

pub(super) const ZIG_BUILD_FLAGS: &[&str] = &[
    "--build-file",
    "--build-runner",
    "--cache-dir",
    "--global-cache-dir",
    "--zig-lib-dir",
];

fn zig_completions(words: &Words, ctx: &DynCtx) -> Option<Vec<Candidate>> {
    if words.arg(0) != "build"
        || words.past_dashes()
        || words.current().starts_with('-')
        || !words.at_operand(ctx)
    {
        return None;
    }

    let mut args = vec![ctx.bin("zig"), "build".into()];
    for flag in ZIG_BUILD_FLAGS {
        if let Some(value) = words.value(&[flag]) {
            args.extend([(*flag).to_owned(), value.to_owned()]);
        }
    }
    args.push("--list-steps".into());
    let out = run(&args, ctx)?;

    let mut candidates = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim_start();
        let mut iter = trimmed.split_whitespace();
        let Some(name) = iter.next() else { continue };
        candidates.push(Candidate::new(name, iter.collect::<Vec<&str>>().join(" ")));
    }

    (!candidates.is_empty()).then_some(candidates)
}
