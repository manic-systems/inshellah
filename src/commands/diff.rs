// SPDX-License-Identifier: EUPL-1.2
//! `inshellah diff`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use inshellah::indexer::{
    find_manpage_path, list_manpages, parse_help_text, try_help, try_help_args,
};
use inshellah::parsers::manpage::{
    ManpageEntry, ManpageResult, OwnedSwitch, parse_manpage_string, read_manpage_file,
};

use super::common::find_in_path;

fn mandirs_for_bin(bin: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(prefix) = bin.parent().and_then(|p| p.parent()) {
        out.push(prefix.join("share/man"));
    }
    for p in [
        "/run/current-system/sw/share/man",
        "/usr/share/man",
        "/usr/local/share/man",
    ] {
        out.push(PathBuf::from(p));
    }
    out.into_iter().filter(|p| p.is_dir()).collect()
}

fn switch_key(e: &ManpageEntry) -> String {
    match &e.switch {
        OwnedSwitch::Both(_, l) | OwnedSwitch::Long(l) => format!("--{l}"),
        OwnedSwitch::Short(c) => format!("-{c}"),
    }
}

fn diff_sets(label: &str, man: &[String], help: &[String]) {
    let sa: BTreeSet<&str> = man.iter().map(String::as_str).collect();
    let sb: BTreeSet<&str> = help.iter().map(String::as_str).collect();
    let shared = sa.intersection(&sb).count();
    println!(
        "  {label}: {} man, {} help, {shared} shared",
        man.len(),
        help.len()
    );
    let man_only: Vec<&str> = sa.difference(&sb).copied().collect();
    let help_only: Vec<&str> = sb.difference(&sa).copied().collect();
    if !man_only.is_empty() {
        println!("    man-only:  {}", man_only.join(" "));
    }
    if !help_only.is_empty() {
        println!("    help-only: {}", help_only.join(" "));
    }
}

pub fn run(cmd_args: &[String], extra_mandirs: &[PathBuf], timeout_ms: u64) {
    let Some((base, sub_args)) = cmd_args.split_first() else {
        eprintln!("error: diff requires a CMD argument");
        std::process::exit(1);
    };
    let Some(bin) = find_in_path(base) else {
        eprintln!("error: {base} not found in PATH");
        std::process::exit(1);
    };
    let mut mandirs = mandirs_for_bin(&bin);
    mandirs.extend(extra_mandirs.iter().cloned());
    let hyphenated = if sub_args.is_empty() {
        base.clone()
    } else {
        format!("{base}-{}", sub_args.join("-"))
    };
    let full = cmd_args.join(" ");

    let man_path = find_manpage_path(&mandirs, &hyphenated);
    let man = man_path
        .as_ref()
        .and_then(|p| read_manpage_file(p).ok())
        .map(|c| parse_manpage_string(&c));
    let bin_s = bin.to_string_lossy().to_string();
    let help_text = if sub_args.is_empty() {
        try_help(&bin, timeout_ms)
    } else {
        try_help_args(&bin_s, sub_args, timeout_ms)
    };
    let help = help_text.as_deref().map(parse_help_text);

    println!("# diff {full}");
    println!(
        "  manpage: {}",
        man_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".into())
    );
    println!(
        "  help:    {}",
        if help_text.is_some() { "ok" } else { "(none)" }
    );

    let subs = |r: &Option<ManpageResult>| -> Vec<String> {
        r.as_ref()
            .map(|r| r.subcommands.iter().map(|s| s.name.clone()).collect())
            .unwrap_or_default()
    };
    let flags = |r: &Option<ManpageResult>| -> Vec<String> {
        r.as_ref()
            .map(|r| r.entries.iter().map(switch_key).collect())
            .unwrap_or_default()
    };
    let man_subs = subs(&man);
    let help_subs = subs(&help);
    diff_sets("subcommands", &man_subs, &help_subs);
    diff_sets("flags", &flags(&man), &flags(&help));

    if man_subs.is_empty() && !help_subs.is_empty() {
        let covered = help_subs
            .iter()
            .filter(|s| find_manpage_path(&mandirs, &format!("{hyphenated}-{s}")).is_some())
            .count();
        println!(
            "  GAP: manpage body has 0 subcommands, help has {}; sibling pages cover {covered}/{}",
            help_subs.len(),
            help_subs.len()
        );
    }
}

fn looks_like_unenumerated_group(r: &ManpageResult) -> bool {
    r.subcommands.is_empty()
        && r.positionals.iter().any(|(n, _)| {
            matches!(
                n.to_ascii_lowercase().as_str(),
                "command" | "commands" | "subcommand" | "subcommands"
            )
        })
}

pub fn run_scan(prefix: &Path, timeout_ms: u64) {
    let mandirs = vec![prefix.join("share/man")];
    let mut suspects = 0u32;
    let mut help_recoverable = 0u32;
    let mut sibling_covered = 0u32;
    for page in list_manpages(&mandirs) {
        let Ok(contents) = read_manpage_file(&page) else {
            continue;
        };
        let man = parse_manpage_string(&contents);
        if !looks_like_unenumerated_group(&man) {
            continue;
        }
        let stem = page
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.split('.').next().unwrap_or(n))
            .unwrap_or("");
        let toks: Vec<String> = stem.split('-').map(str::to_string).collect();
        let Some((base, sub)) = toks.split_first() else {
            continue;
        };
        let Some(bin) = find_in_path(base) else {
            continue;
        };
        suspects += 1;
        let bin_s = bin.to_string_lossy().to_string();
        let help_text = if sub.is_empty() {
            try_help(&bin, timeout_ms)
        } else {
            try_help_args(&bin_s, sub, timeout_ms)
        };
        let help_subs: Vec<String> = help_text
            .as_deref()
            .map(parse_help_text)
            .map(|r| r.subcommands.into_iter().map(|s| s.name).collect())
            .unwrap_or_default();
        if help_subs.is_empty() {
            continue;
        }
        let covered = help_subs
            .iter()
            .filter(|s| find_manpage_path(&mandirs, &format!("{stem}-{s}")).is_some())
            .count();
        let kind = if covered == help_subs.len() {
            sibling_covered += 1;
            "sibling-covered (body parser gap)"
        } else {
            help_recoverable += 1;
            "help-only"
        };
        println!(
            "{}: body=0 help={} siblings={}/{}  [{kind}]",
            toks.join(" "),
            help_subs.len(),
            covered,
            help_subs.len()
        );
    }
    eprintln!(
        "scanned: {suspects} group suspects, {sibling_covered} body-parser gaps, {help_recoverable} help-only"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use inshellah::types::Positional;

    #[test]
    fn detects_unenumerated_group_placeholder() {
        let mut result = ManpageResult::default();
        result.positionals.push((
            "subcommands".to_string(),
            Positional {
                optional: false,
                variadic: false,
            },
        ));

        assert!(looks_like_unenumerated_group(&result));
    }
}
