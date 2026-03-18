//! strategy-based entry extraction.
//!
//! rather than a single monolithic parser, we use multiple "strategies" that
//! each target a specific groff formatting pattern. this is necessary because
//! manpage authors use very different macro combinations for the same purpose.

use nom::{Parser, combinator::opt};

use crate::make_macro_walker;
use crate::parsers::help::{help_parser, param_parser, switch_parser};
use crate::parsers::manpage::groff::{
    GroffLine, strip_groff_escapes, strip_inline_macro_args, strip_space_macro_args,
};
use crate::parsers::manpage::{ManpageEntry, OwnedParam, OwnedSwitch};
use crate::types::{Param, Switch};

/// collect consecutive text lines, joining them with spaces.
/// returns (collected, remaining).
fn collect_text_lines(lines: &[GroffLine]) -> (String, &[GroffLine]) {
    let mut acc: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Text(t) => acc.push(t),
            _ => break,
        }
        i += 1;
    }
    (acc.join(" "), &lines[i..])
}

fn collect_description_lines(lines: &[GroffLine], start: usize) -> (String, usize) {
    let mut acc: Vec<String> = Vec::new();
    let mut i = start;
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Macro { name, .. }
                if matches!(name.as_str(), "TP" | "TQ" | "IP" | "PP" | "SH" | "SS") =>
            {
                break;
            }
            GroffLine::Text(t) => {
                acc.push(t.clone());
                i += 1;
            }
            GroffLine::Macro { name, args }
                if matches!(
                    name.as_str(),
                    "B" | "BI" | "BR" | "I" | "IR" | "IB" | "RB" | "RI"
                ) =>
            {
                let text = tag_of_macro(name, args);
                if !text.is_empty() {
                    acc.push(text);
                }
                i += 1;
            }
            GroffLine::Blank | GroffLine::Comment => {
                i += 1;
            }
            GroffLine::Macro { .. } => {
                i += 1;
            }
        }
    }
    (acc.join(" "), i)
}

fn to_owned_switch(s: Switch<'_>) -> OwnedSwitch {
    match s {
        Switch::Short(c) => OwnedSwitch::Short(c),
        Switch::Long(l) => OwnedSwitch::Long(l.to_string()),
        Switch::Both(c, l) => OwnedSwitch::Both(c, l.to_string()),
    }
}

fn to_owned_param(p: Param<'_>) -> OwnedParam {
    match p {
        Param::Mandatory(s) => OwnedParam::Mandatory(s.to_string()),
        Param::Optional(s) => OwnedParam::Optional(s.to_string()),
    }
}

/// attempt to parse a tag string (e.g. "-v, --verbose FILE") into an entry.
/// uses the nom switch_parser + param_parser from the help module.
/// returns None if the tag doesn't look like a flag definition.
pub fn parse_tag_to_entry(tag: &str, desc: String) -> Option<ManpageEntry> {
    let tag = strip_groff_escapes(tag);
    let tag = tag.trim();
    let result: nom::IResult<&str, (Switch<'_>, Option<Param<'_>>)> =
        (switch_parser, opt(param_parser)).parse(tag);
    match result {
        Ok((_, (switch, param))) => Some(ManpageEntry {
            switch: to_owned_switch(switch),
            param: param.map(to_owned_param),
            desc,
        }),
        Err(_) => None,
    }
}

/// extract tag text from a macro line.
/// .B and .I preserve spaces (single argument); .BI, .BR, .IR alternate
/// fonts and concatenate arguments.
pub fn tag_of_macro(name: &str, args: &str) -> String {
    match name {
        "B" | "I" => strip_space_macro_args(args),
        _ => strip_groff_escapes(&strip_inline_macro_args(args))
            .trim()
            .to_string(),
    }
}

// strategy a: .TP style (most common — gnu coreutils, help2man).
// .TP introduces a tagged paragraph: the next line is the "tag" (flag name)
// and subsequent text lines are the description. the tag can be plain text
// or wrapped in a formatting macro (.B, .BI, etc.).
pub fn strategy_tp(lines: &[GroffLine]) -> Vec<ManpageEntry> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let GroffLine::Macro { name, .. } = &lines[i] else {
            i += 1;
            continue;
        };
        if name != "TP" {
            i += 1;
            continue;
        }

        let (tags, body_start) = collect_tp_tags(lines, i + 1);
        if tags.is_empty() {
            i += 1;
            continue;
        }
        let (desc, new_i) = collect_description_lines(lines, body_start);
        out.extend(entries_from_tag_alternates(&tags, desc));
        i = new_i;
    }
    out
}

fn collect_tp_tags(lines: &[GroffLine], start: usize) -> (Vec<String>, usize) {
    let mut tags = Vec::new();
    let mut i = start;
    loop {
        if i >= lines.len() {
            break;
        }
        let Some(tag) = tag_from_line(&lines[i]) else {
            break;
        };
        tags.push(tag);
        i += 1;
        if i < lines.len() && matches!(&lines[i], GroffLine::Macro { name, .. } if name == "TQ") {
            i += 1;
            continue;
        }
        break;
    }
    (tags, i)
}

fn tag_from_line(line: &GroffLine) -> Option<String> {
    match line {
        GroffLine::Text(tag) => Some(tag.clone()),
        GroffLine::Macro { name, args }
            if matches!(
                name.as_str(),
                "B" | "I" | "BI" | "BR" | "IR" | "IB" | "RB" | "RI"
            ) =>
        {
            Some(tag_of_macro(name, args))
        }
        _ => None,
    }
}

fn entries_from_tag_alternates(tags: &[String], desc: String) -> Vec<ManpageEntry> {
    let entries: Vec<ManpageEntry> = tags
        .iter()
        .filter_map(|tag| parse_tag_to_entry(tag, desc.clone()))
        .collect();
    if entries.len() == 2
        && let Some(combined) = combine_short_long_alternates(&entries[0], &entries[1])
    {
        return vec![combined];
    }
    entries
}

fn combine_short_long_alternates(
    left: &ManpageEntry,
    right: &ManpageEntry,
) -> Option<ManpageEntry> {
    match (&left.switch, &right.switch) {
        (OwnedSwitch::Long(l), OwnedSwitch::Short(c)) => Some(ManpageEntry {
            switch: OwnedSwitch::Both(*c, l.clone()),
            param: left.param.clone().or_else(|| right.param.clone()),
            desc: left.desc.clone(),
        }),
        (OwnedSwitch::Short(c), OwnedSwitch::Long(l)) => Some(ManpageEntry {
            switch: OwnedSwitch::Both(*c, l.clone()),
            param: right.param.clone().or_else(|| left.param.clone()),
            desc: left.desc.clone(),
        }),
        _ => None,
    }
}

// strategy b: .IP style (curl, hand-written manpages).
// .IP takes an inline tag argument: .IP "-v, --verbose"
// the description follows as text lines.
make_macro_walker!(pub strategy_ip -> Vec<ManpageEntry>, on macro "IP" =>
    |lines, i, args| {
        let tag = strip_groff_escapes(args);
        let (desc, rest) = collect_text_lines(&lines[i + 1..]);
        let new_i = lines.len() - rest.len();
        parse_tag_to_entry(&tag, desc).map(|e| (e, new_i))
    }
);

// strategy c: .PP + .RS/.RE style (git, docbook-generated manpages).
// flag entries are introduced by .PP (paragraph), with the flag name as
// plain text, followed by a .RS (indent) block containing the description,
// closed by .RE (de-indent).
make_macro_walker!(pub strategy_pp_rs -> Vec<ManpageEntry>, on macro "PP" =>
    |lines, i, _args| {
        if i + 1 >= lines.len() { return None; }
        if let GroffLine::Text(tag) = &lines[i + 1] {
            let (desc, new_i) = collect_pp_rs_desc(lines, i + 2);
            parse_tag_to_entry(tag, desc).map(|e| (e, new_i))
        } else {
            None
        }
    }
);

fn collect_pp_rs_desc(lines: &[GroffLine], start: usize) -> (String, usize) {
    let mut acc: Vec<String> = Vec::new();
    let mut i = start;
    // outer: look for .RS marker or text
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Macro { name, .. } if name == "RS" => {
                i += 1;
                // inside .RS — collect until .RE or boundary macro
                while i < lines.len() {
                    match &lines[i] {
                        GroffLine::Macro { name, .. } if name == "RE" => {
                            return (acc.join(" "), i + 1);
                        }
                        GroffLine::Text(t) => {
                            acc.push(t.clone());
                            i += 1;
                        }
                        GroffLine::Macro { name, .. } if name == "PP" || name == "SH" => {
                            return (acc.join(" "), i);
                        }
                        _ => i += 1,
                    }
                }
                return (acc.join(" "), i);
            }
            GroffLine::Text(t) => {
                acc.push(t.clone());
                i += 1;
            }
            _ => return (acc.join(" "), i),
        }
    }
    (acc.join(" "), i)
}

/// strategy d: deroff fallback — strip all groff markup, then feed the
/// resulting plain text through the help parser.
pub fn strategy_deroff(lines: &[GroffLine]) -> Vec<ManpageEntry> {
    let mut buffer = String::with_capacity(256);
    for line in lines {
        match line {
            GroffLine::Text(text) => {
                buffer.push_str(text);
                buffer.push('\n');
            }
            GroffLine::Macro { name, args }
                if matches!(name.as_str(), "BI" | "BR" | "IR" | "B" | "I") =>
            {
                let text = strip_groff_escapes(&strip_inline_macro_args(args));
                buffer.push_str(&text);
                buffer.push('\n');
            }
            GroffLine::Blank => buffer.push('\n'),
            _ => (),
        }
    }
    match help_parser(&buffer) {
        Ok((_, result)) => result
            .entries
            .into_iter()
            .map(|e| ManpageEntry {
                switch: to_owned_switch(e.switch),
                param: e.param.map(to_owned_param),
                desc: e.desc.join(" "),
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn is_bullet_ip(args: &str) -> bool {
    !args.trim().is_empty()
}

// strategy e: nix3-style bullet .IP with .UR/.UE hyperlinks.
// nix's manpages use .IP with bullet markers for flag entries, interleaved
// with .UR/.UE hyperlink macros. the flag tag is in text lines after the
// bullet .IP, and the description follows a non-bullet .IP marker.
make_macro_walker!(pub strategy_nix -> Vec<ManpageEntry>, on macro "IP" =>
    |lines, i, args| {
        if !is_bullet_ip(args) { return None; }
        // collect tag: skip .UR/.UE macros, gather Text lines
        let mut tag_idx = i + 1;
        let mut tag_parts: Vec<String> = Vec::new();
        while tag_idx < lines.len() {
            match &lines[tag_idx] {
                GroffLine::Macro { name, .. } if name == "UR" || name == "UE" => {
                    tag_idx += 1;
                }
                GroffLine::Text(t) => {
                    tag_parts.push(t.clone());
                    tag_idx += 1;
                }
                _ => break,
            }
        }
        let tag = tag_parts.join(" ");
        let (desc, new_i) = collect_nix_desc(lines, tag_idx);
        parse_tag_to_entry(&tag, desc).map(|e| (e, new_i))
    }
);

fn collect_nix_desc(lines: &[GroffLine], start: usize) -> (String, usize) {
    if start >= lines.len() {
        return (String::new(), start);
    }
    let mut i = start;
    // require non-bullet .IP marker for description
    if let GroffLine::Macro { name, args } = &lines[i]
        && name == "IP"
        && args.trim().is_empty()
    {
        i += 1;
    } else {
        return (String::new(), start);
    }
    let mut parts: Vec<String> = Vec::new();
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Text(t) => {
                parts.push(t.clone());
                i += 1;
            }
            GroffLine::Macro { name, args } if name == "IP" => {
                if !args.trim().is_empty() {
                    // next bullet entry — stop
                    return (parts.join(" "), i);
                }
                // non-bullet .IP = continuation paragraph
                i += 1;
            }
            GroffLine::Macro { name, .. } if name == "SS" || name == "SH" => {
                return (parts.join(" "), i);
            }
            GroffLine::Macro { name, .. } if name == "RS" => {
                i = skip_rs(lines, i + 1, 1);
            }
            GroffLine::Macro { .. } => {
                i += 1;
            }
            GroffLine::Blank | GroffLine::Comment => {
                i += 1;
            }
        }
    }
    (parts.join(" "), i)
}

fn skip_rs(lines: &[GroffLine], start: usize, mut depth: usize) -> usize {
    let mut i = start;
    while i < lines.len() {
        if let GroffLine::Macro { name, .. } = &lines[i] {
            if name == "RE" {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            } else if name == "RS" {
                depth += 1;
            }
        }
        i += 1;
    }
    i
}

/// count occurrences of a specific macro in the section.
fn count_macro(name: &str, lines: &[GroffLine]) -> usize {
    lines
        .iter()
        .filter(|line| matches!(line, GroffLine::Macro { name: n, .. } if n == name))
        .count()
}

/// auto-detect and try strategies, return the one with most entries.
/// first counts macros to determine which strategies are applicable,
/// then runs all applicable ones and picks the winner by entry count.
/// if no specialized strategy produces results, falls back to deroff.
pub fn extract_entries(lines: &[GroffLine]) -> Vec<ManpageEntry> {
    let tp = count_macro("TP", lines);
    let ip = count_macro("IP", lines);
    let pp = count_macro("PP", lines);
    let rs = count_macro("RS", lines);
    let ur = count_macro("UR", lines);

    let mut specialized: Vec<(&str, Vec<ManpageEntry>)> = Vec::new();
    if tp > 0 {
        specialized.push(("TP", strategy_tp(lines)));
    }
    if ip > 0 {
        specialized.push(("IP", strategy_ip(lines)));
    }
    if pp > 0 && rs > 0 {
        specialized.push(("PP+RS", strategy_pp_rs(lines)));
    }
    if ur > 0 && ip > 0 {
        specialized.push(("nix", strategy_nix(lines)));
    }
    let candidates: Vec<(&str, Vec<ManpageEntry>)> = {
        let filtered: Vec<_> = specialized
            .into_iter()
            .filter(|(_, e)| !e.is_empty())
            .collect();
        if filtered.is_empty() {
            vec![("deroff", strategy_deroff(lines))]
        } else {
            filtered
        }
    };
    let mut best: Vec<ManpageEntry> = Vec::new();
    for (_, entries) in candidates {
        if entries.len() >= best.len() {
            best = entries;
        }
    }
    best
}
