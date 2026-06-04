// SPDX-License-Identifier: EUPL-1.2
mod description;
mod helpers;
mod options;
mod positionals;
mod subcommands;

pub use options::{param_parser, parse_usage_flags, switch_parser};
pub use positionals::{
    extract_cli11_positionals, extract_usage_positionals, parse_usage_args, skip_command_name,
};

use std::collections::HashMap;

use crate::parsers::help::{
    description::description, helpers::get_indent, subcommands::subcommand_entry,
};
use crate::parsers::manpage::{ManpageEntry, ManpageResult, ManpageSubcommand, OwnedParam, OwnedSwitch};
use nom::{IResult, Parser, character::complete::space0, combinator::opt};

use crate::make_parser;

type EntryParts<'a> = (
    &'a str,
    (OwnedSwitch, Option<OwnedParam>),
    (&'a str, Vec<&'a str>),
);

make_parser!(entry -> ManpageEntry,
    (
        space0,
        (switch_parser, opt(param_parser)),
        description,
    )
    => |(_, (switch, param), (first, cont))
        : EntryParts<'a>|
    {
        let mut lines: Vec<&str> = Vec::with_capacity(1 + cont.len());
        if !first.trim().is_empty() { lines.push(first); }
        lines.extend(cont.into_iter().filter(|l| !l.trim().is_empty()));
        let desc = lines
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        ManpageEntry { switch, param, desc }
    }
);

/// dedup by case-insensitive name, longest desc wins, first-seen order.
fn dedup_subcommands(raw: Vec<ManpageSubcommand>) -> Vec<ManpageSubcommand> {
    let mut by_name: HashMap<String, ManpageSubcommand> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for sc in raw {
        let key = sc.name.to_ascii_lowercase();
        match by_name.get(&key) {
            Some(prev) if prev.desc.len() >= sc.desc.len() => {}
            _ => {
                if !by_name.contains_key(&key) {
                    order.push(key.clone());
                }
                by_name.insert(key, sc);
            }
        }
    }
    order
        .into_iter()
        .map(|k| by_name.remove(&k).unwrap())
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HelpSection {
    Unknown,
    Options,
    Commands,
    Other,
}

fn classify_section_line(line: &str) -> Option<HelpSection> {
    let (idx, indent) = get_indent(line);
    if indent > 4 {
        return None;
    }
    let trimmed = line[idx..].trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_colon = trimmed.trim_end_matches(':').trim();
    let lower = without_colon.to_ascii_lowercase();

    if lower.starts_with("usage") {
        return Some(HelpSection::Unknown);
    }
    if lower.starts_with("valid arguments")
        || lower.contains(" is one of the following")
        || lower.contains(" defaults to")
        || lower == "examples"
        || lower == "example"
    {
        return Some(HelpSection::Other);
    }
    let command_header = matches!(lower.as_str(), "command" | "commands" | "subcommands")
        || lower.ends_with(" commands")
        || lower.ends_with(" subcommands");
    if command_header && !lower.contains("option") && !lower.contains("flag") {
        return Some(HelpSection::Commands);
    }
    if lower.contains("argument")
        || lower == "args"
        || lower == "positionals"
        || lower == "positional arguments"
    {
        return Some(HelpSection::Other);
    }
    if lower.contains("option") || lower.contains("flag") || trimmed.ends_with(':') {
        return Some(HelpSection::Options);
    }
    None
}

fn consume_line(s: &str) -> &str {
    match s.find('\n') {
        Some(idx) => &s[idx + 1..],
        None => "",
    }
}

fn parser_made_progress(original: &str, rem: &str) -> bool {
    rem.len() < original.len()
}

/// options match in option-like sections and before any section is known,
/// subcommands only in command-like sections.
fn build_help_result(original: &str) -> ManpageResult {
    let mut entries: Vec<ManpageEntry> = Vec::new();
    let mut raw_subcommands: Vec<ManpageSubcommand> = Vec::new();
    let mut section = HelpSection::Unknown;
    let mut rem = original;

    while !rem.is_empty() {
        let line = rem.split_once('\n').map(|(line, _)| line).unwrap_or(rem);
        if let Some(next_section) = classify_section_line(line) {
            section = next_section;
            rem = consume_line(rem);
            continue;
        }

        if matches!(section, HelpSection::Unknown | HelpSection::Options)
            && let Ok((next, parsed)) = entry(rem)
            && parser_made_progress(rem, next)
        {
            entries.push(parsed);
            rem = next;
            continue;
        }

        if section == HelpSection::Commands
            && let Ok((next, parsed)) = subcommand_entry(rem)
            && parser_made_progress(rem, next)
        {
            raw_subcommands.push(parsed);
            rem = next;
            continue;
        }

        rem = consume_line(rem);
    }

    // recursive --help probes dispatch on the lowercase name.
    let subcommands = dedup_subcommands(raw_subcommands)
        .into_iter()
        .map(|sc| ManpageSubcommand {
            name: sc.name.to_ascii_lowercase(),
            desc: sc.desc,
            aliases: sc
                .aliases
                .iter()
                .map(|a| a.to_ascii_lowercase())
                .collect(),
        })
        .collect();
    // cli11 positional section carries types and optionality, prefer it over
    // the usage-line scan.
    let positionals_raw = match extract_cli11_positionals(original) {
        Ok((_, p)) if !p.is_empty() => p,
        _ => extract_usage_positionals(original)
            .map(|(_, p)| p)
            .unwrap_or_default(),
    };
    let positionals = positionals_raw
        .into_iter()
        .map(|(k, v)| (k.to_ascii_lowercase(), v))
        .collect();
    let mut result = ManpageResult {
        entries,
        subcommands,
        positional_choices: Vec::new(),
        positionals,
        description: String::new(),
    };
    result.normalize();
    result
}

pub fn help_parser(s: &str) -> IResult<&str, ManpageResult> {
    Ok(("", build_help_result(s)))
}