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

use crate::{
    parsers::help::{description::description, helpers::get_indent, subcommands::subcommand_entry},
    types::*,
};
use nom::{IResult, Parser, character::complete::space0, combinator::opt};

use crate::make_parser;

type EntryParts<'a> = (
    &'a str,
    (Switch<'a>, Option<Param<'a>>),
    (&'a str, Vec<&'a str>),
);

// parse a single flag entry: indent + switch + optional param + description.
make_parser!(entry -> OptionEntry<'a>,
    (
        space0,
        (switch_parser, opt(param_parser)),
        description,
    )
    => |(_, (switch, param), (first, cont))
        : EntryParts<'a>|
    {
        let mut desc: Vec<&str> = Vec::with_capacity(1 + cont.len());
        if !first.trim().is_empty() { desc.push(first); }
        desc.extend(cont.into_iter().filter(|l| !l.trim().is_empty()));
        OptionEntry { switch, param, desc }
    }
);

/// dedup raw subcommands by case-insensitive name, keeping the entry with
/// the longest description. preserves first-seen ordering.
fn dedup_subcommands<'a>(raw: Vec<Subcommand<'a>>) -> Vec<Subcommand<'a>> {
    let mut by_name: HashMap<String, Subcommand<'a>> = HashMap::new();
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

/// build the final HelpResult by scanning help text with lightweight section
/// awareness. options are accepted in option-like sections and before a
/// section is known; subcommands are accepted only in command-like sections.
fn build_help_result<'a>(original: &'a str) -> HelpResult<'a> {
    let mut entries = Vec::new();
    let mut raw_subcommands: Vec<Subcommand<'a>> = Vec::new();
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

    let subcommands = dedup_subcommands(raw_subcommands);
    // cli11 positional section takes priority over the usage-line scan
    // when both are present — cli11 carries types and optionality.
    let positionals = match extract_cli11_positionals(original) {
        Ok((_, p)) if !p.is_empty() => p,
        _ => extract_usage_positionals(original)
            .map(|(_, p)| p)
            .unwrap_or_default(),
    };
    HelpResult {
        entries,
        subcommands,
        positionals,
        desc: "",
    }
}

/// top-level help parser.
pub fn help_parser(s: &str) -> IResult<&str, HelpResult<'_>> {
    Ok(("", build_help_result(s)))
}
