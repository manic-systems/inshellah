//! COMMANDS section subcommand extraction. some manpages (systemctl) list
//! subcommands as .PP + bold name + .RS/.RE description blocks:
//!   .PP
//!   \fBstart\fR \fIUNIT\fR...
//!   .RS 4
//!   Start (activate) one or more units.
//!   .RE

use crate::parsers::manpage::{
    ManpageSubcommand,
    desc,
    groff::{
        GroffLine,
        strip_groff_escapes,
        strip_inline_macro_args,
    },
};

fn is_valid_subcmd(name: &str) -> bool {
    name.len() >= 2
        && !name.starts_with('-')
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
}

/// subcommand name from bold groff text:
///   "\fBlist\-units\fR [\fIPATTERN\fR...]" -> "list-units"
fn extract_bold_command_name(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if let Some(after) = trimmed.strip_prefix("\\fB")
        && !after.is_empty()
    {
        // segment between leading \fB and the next \
        let name_part = after.split_once('\\').map_or(after, |(head, _)| head);
        let reconstructed = format!("\\fB{name_part}\\fR");
        let name = normalize_command_token(strip_groff_escapes(&reconstructed).trim());
        if is_valid_subcmd(&name) {
            return Some(name);
        }
        return None;
    }
    let stripped = strip_groff_escapes(trimmed);
    let first_word = stripped.split_whitespace().next().unwrap_or("");
    let name = normalize_command_token(first_word);
    is_valid_subcmd(&name).then_some(name)
}

fn normalize_command_token(token: &str) -> String {
    let trimmed = token.trim();
    trimmed
        .split_once('(')
        .map_or(trimmed, |(head, _)| head)
        .trim_end_matches(',')
        .to_owned()
}

fn extract_command_name_from_line(line: &GroffLine) -> Option<String> {
    match line {
        GroffLine::Text(tag) => extract_bold_command_name(tag),
        GroffLine::Macro { name, args }
            if matches!(
                name.as_str(),
                "B" | "BI" | "BR" | "I" | "IR" | "IB" | "RB" | "RI"
            ) =>
        {
            let rendered = strip_groff_escapes(&strip_inline_macro_args(args));
            extract_bold_command_name(&rendered)
        },
        GroffLine::Macro { .. } | GroffLine::Blank | GroffLine::Comment => None,
    }
}

/// extract subcommand name+description pairs. handles two tagged-list layouts:
/// `.PP` + bold name + `.RS/.RE` (systemctl, git) and `.TP` + `.B name` + body
/// (help2man).
///
/// only top-level entries are mined: `.PP`/`.TP` tags nested in an `.RS` block
/// are a command's own option/value sublists, not sibling commands; `.RS` depth
/// keeps those out.
pub fn extract_subcommands_from_commands(lines: &[GroffLine]) -> Vec<ManpageSubcommand> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut rs_depth: u32 = 0;
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Macro { name, .. } if name == "RS" => {
                rs_depth += 1;
                i += 1;
            },
            GroffLine::Macro { name, .. } if name == "RE" => {
                rs_depth = rs_depth.saturating_sub(1);
                i += 1;
            },
            GroffLine::Macro { name, .. } if rs_depth == 0 && (name == "PP" || name == "TP") => {
                i += 1;
                if i >= lines.len() {
                    continue;
                }
                if let Some(cmd_name) = extract_command_name_from_line(&lines[i]) {
                    let (desc, new_i) = collect_subcmd_desc(lines, i + 1);
                    let short_desc = first_sentence(&desc);
                    out.push(ManpageSubcommand::new(
                        cmd_name.to_ascii_lowercase(),
                        short_desc,
                    ));
                    i = new_i;
                } else {
                    i += 1;
                }
            },
            GroffLine::Macro { .. }
            | GroffLine::Text(_)
            | GroffLine::Blank
            | GroffLine::Comment => {
                i += 1;
            },
        }
    }
    // a tool may repeat a command name across `.TP` tags (bash repeats `bind`
    // once per builtin flag); dedup so the cache holds one entry.
    dedup_by_name(out)
}

/// one entry per case-insensitive name, longest description wins, first-seen
/// order preserved.
fn dedup_by_name(raw: Vec<ManpageSubcommand>) -> Vec<ManpageSubcommand> {
    use std::collections::HashMap;
    let mut best: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<ManpageSubcommand> = Vec::with_capacity(raw.len());
    for sc in raw {
        let key = sc.name.to_ascii_lowercase();
        if let Some(&idx) = best.get(&key) {
            if sc.desc.len() > out[idx].desc.len() {
                out[idx].desc = sc.desc;
            }
        } else {
            best.insert(key, out.len());
            out.push(sc);
        }
    }
    out
}

/// jj/clap group manpages list children in `.SH SUBCOMMANDS` as `.TP`
/// cross-references: a term line like `jj\-bookmark\-advance(1)` plus a
/// description. child name = xref with the shared parent prefix and the `(N)`
/// suffix stripped, so `set-url` survives intact.
pub fn extract_subcommand_xrefs(lines: &[GroffLine]) -> Vec<ManpageSubcommand> {
    let mut raw: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !matches!(&lines[i], GroffLine::Macro { name, .. } if name == "TP") {
            i += 1;
            continue;
        }
        i += 1;
        let Some(GroffLine::Text(raw_term)) = lines.get(i) else {
            continue;
        };
        i += 1;
        // require the "...(N)" xref shape so this can't misfire on other tools'
        // `.SH SUBCOMMAND` layouts.
        let rendered = strip_groff_escapes(raw_term);
        let term = rendered.trim();
        let Some((xref, section)) = term
            .strip_suffix(')')
            .and_then(|inner| inner.rsplit_once('('))
        else {
            continue;
        };
        if !section.bytes().all(|byte| byte.is_ascii_digit()) || section.is_empty() {
            continue;
        }
        let token = xref.trim();
        if token.is_empty() || token.contains(char::is_whitespace) {
            continue;
        }
        let (desc, new_i) = collect_xref_desc(lines, i);
        i = new_i;
        raw.push((token.to_owned(), first_sentence(&desc)));
    }
    // strip the shared prefix (e.g. "jj-bookmark-").
    let prefix = shared_dash_prefix(raw.iter().map(|(token, _)| token.as_str()));
    raw.into_iter()
        .filter_map(|(token, desc)| {
            let child = token.strip_prefix(&prefix).unwrap_or(&token);
            is_valid_subcmd(child).then(|| ManpageSubcommand::new(child.to_ascii_lowercase(), desc))
        })
        .collect()
}

/// longest common prefix truncated to the last `-`, so the remainder is a
/// whole subcommand name. empty for fewer than two tokens.
fn shared_dash_prefix<'src>(tokens: impl Iterator<Item = &'src str>) -> String {
    let all: Vec<&str> = tokens.collect();
    let Some((first, rest)) = all.split_first() else {
        return String::new();
    };
    if rest.is_empty() {
        return String::new();
    }
    let mut len = first.len();
    for token in rest {
        len = first
            .bytes()
            .zip(token.bytes())
            .take(len)
            .take_while(|(lhs, rhs)| lhs == rhs)
            .count();
    }
    let Some(common) = first.get(..first.floor_char_boundary(len)) else {
        return String::new();
    };
    common
        .rsplit_once('-')
        .map_or_else(String::new, |(head, _)| format!("{head}-"))
}

/// xref description: text lines until the next .TP/.SH/.SS. Text is already
/// groff-stripped at classify time, so no inline-macro rendering here.
fn collect_xref_desc(lines: &[GroffLine], start: usize) -> (String, usize) {
    desc::collect(lines, start, desc::DescOpts {
        boundaries:    &["TP", "SH", "SS"],
        skip_rs:       false,
        stop_on_blank: false,
        tags:          desc::TagMacros::None,
    })
}

/// subcommand description: handles .RS/.RE blocks, stops at the next
/// .PP/.SH/.SS.
fn collect_subcmd_desc(lines: &[GroffLine], start: usize) -> (String, usize) {
    let mut acc: Vec<String> = Vec::new();
    let mut i = start;
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Macro { name, .. } if name == "RS" => {
                i += 1;
                while i < lines.len() {
                    match &lines[i] {
                        GroffLine::Macro { name: inner, .. } if inner == "RE" => {
                            return (acc.join(" "), i + 1);
                        },
                        GroffLine::Text(text) => {
                            acc.push(text.clone());
                            i += 1;
                        },
                        GroffLine::Macro { name: inner, .. }
                            if inner == "PP" || inner == "SH" || inner == "SS" =>
                        {
                            return (acc.join(" "), i);
                        },
                        GroffLine::Macro { .. } | GroffLine::Blank | GroffLine::Comment => i += 1,
                    }
                }
                return (acc.join(" "), i);
            },
            GroffLine::Text(text) => {
                acc.push(text.clone());
                i += 1;
            },
            GroffLine::Macro { .. } | GroffLine::Blank | GroffLine::Comment => {
                return (acc.join(" "), i);
            },
        }
    }
    (acc.join(" "), i)
}

fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    match trimmed.split_once('.') {
        Some((head, _)) if !head.is_empty() => head.trim().to_owned(),
        _ => trimmed.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsers::manpage::groff::classify_line;

    fn commands_of(src: &str) -> Vec<(String, String)> {
        let lines: Vec<GroffLine> = src.split('\n').map(classify_line).collect();
        extract_subcommands_from_commands(&lines)
            .into_iter()
            .map(|sc| (sc.name, sc.desc))
            .collect()
    }

    #[test]
    fn tp_flat_command_list() {
        // widget-style `.SH COMMANDS`: `.TP` + `.B name` + one-line desc.
        let src = ".TP\n.B create\nCreate a new widget.\n.TP\n.B list\nList existing \
                   widgets.\n.TP\n.B remove\nRemove a widget by name.\n";
        let got = commands_of(src);
        assert_eq!(
            got.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["create", "list", "remove"]
        );
        assert_eq!(got[0].1, "Create a new widget");
    }

    #[test]
    fn nested_rs_entries_are_not_mined() {
        // a command's own option sublist lives in an `.RS` block; those `.TP`
        // tags must not be mined as sibling commands (bash's builtin flags).
        let src = ".TP\n.B complete\nGenerate completions.\n.RS\n.TP\n.B nospace\ninner option \
                   value\n.TP\n.B plusdirs\nanother option value\n.RE\n.TP\n.B alias\nDefine an \
                   alias.\n";
        let got = commands_of(src);
        assert_eq!(
            got.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["complete", "alias"],
            "nested .RS .TP option values leaked: {got:?}"
        );
    }

    #[test]
    fn duplicate_names_collapse_keeping_longest_desc() {
        // bash repeats `bind` once per flag; the parser keeps one entry with
        // the richest description.
        let src =
            ".TP\n.B bind\nshort.\n.TP\n.B bind\nDisplay current key and function bindings.\n";
        let got = commands_of(src);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "bind");
        assert_eq!(got[0].1, "Display current key and function bindings");
    }
}
