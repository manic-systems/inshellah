//! COMMANDS section subcommand extraction.
//!
//! some manpages (notably systemctl) have a dedicated COMMANDS section
//! listing subcommands with descriptions. these use .PP + bold name +
//! .RS/.RE blocks:
//!   .PP
//!   \fBstart\fR \fIUNIT\fR...
//!   .RS 4
//!   Start (activate) one or more units.
//!   .RE

use crate::parsers::manpage::ManpageSubcommand;
use crate::parsers::manpage::groff::{GroffLine, strip_groff_escapes, strip_inline_macro_args};

/// validate that the extracted name looks like a subcommand: lowercase,
/// at least 2 chars, no leading dash.
fn is_valid_subcmd(name: &str) -> bool {
    name.len() >= 2
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// extract subcommand name from a bold groff text like
///   "\fBlist\-units\fR [\fIPATTERN\fR...]" -> "list-units"
fn extract_bold_command_name(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.len() >= 4 && trimmed.starts_with("\\fB") {
        // look for \fB...\fR at the start: find the next '\\' and take
        // the segment between \fB and there.
        let after = &trimmed[3..];
        let segment_end = after.find('\\').unwrap_or(after.len());
        let name_part = &after[..segment_end];
        let reconstructed = format!("\\fB{name_part}\\fR");
        let name = normalize_command_token(strip_groff_escapes(&reconstructed).trim());
        if is_valid_subcmd(&name) {
            return Some(name);
        }
        return None;
    }
    // fallback: take the first whitespace-delimited word of the stripped text
    let stripped = strip_groff_escapes(trimmed);
    let first_word = stripped.split_whitespace().next().unwrap_or("");
    let name = normalize_command_token(first_word);
    if is_valid_subcmd(&name) {
        Some(name)
    } else {
        None
    }
}

fn normalize_command_token(token: &str) -> String {
    let token = token.trim();
    let token = token
        .find('(')
        .map(|idx| &token[..idx])
        .unwrap_or(token)
        .trim_end_matches(',');
    token.to_string()
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
        }
        _ => None,
    }
}

/// walk through commands section lines, extracting subcommand name+description
/// pairs from .PP + Text + .RS/.RE blocks.
pub fn extract_subcommands_from_commands(lines: &[GroffLine]) -> Vec<ManpageSubcommand> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if let GroffLine::Macro { name, .. } = &lines[i]
            && name == "PP"
        {
            i += 1;
            if i >= lines.len() {
                continue;
            }
            if let Some(name) = extract_command_name_from_line(&lines[i]) {
                let (desc, new_i) = collect_subcmd_desc(lines, i + 1);
                let short_desc = first_sentence(&desc);
                out.push(ManpageSubcommand {
                    name: name.to_ascii_lowercase(),
                    desc: short_desc,
                });
                i = new_i;
                continue;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// jj/clap group manpages list children in a `.SH SUBCOMMANDS` section as
/// `.TP` cross-references: a term line like `jj\-bookmark\-advance(1)`
/// followed by a description line. each child name is the xref with the
/// shared parent prefix (derived from all the xrefs) and the `(N)` section
/// suffix stripped, so multi-word names like `set-url` survive intact.
pub fn extract_subcommand_xrefs(lines: &[GroffLine]) -> Vec<ManpageSubcommand> {
    // pass 1: collect the raw xref tokens and their descriptions.
    let mut raw: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Macro { name, .. } if name == "TP" => {}
            _ => {
                i += 1;
                continue;
            }
        }
        i += 1;
        let Some(GroffLine::Text(term)) = lines.get(i) else {
            continue;
        };
        i += 1;
        // require a manpage cross-reference shape, "...(N)", so this can't
        // misfire on other tools' `.SH SUBCOMMAND` layouts.
        let term = strip_groff_escapes(term);
        let term = term.trim();
        let Some(token) = term.strip_suffix(')').and_then(|t| t.rsplit_once('(')) else {
            continue;
        };
        let (token, section) = token;
        if !section.bytes().all(|b| b.is_ascii_digit()) || section.is_empty() {
            continue;
        }
        let token = token.trim();
        if token.is_empty() || token.contains(char::is_whitespace) {
            continue;
        }
        let (desc, new_i) = collect_xref_desc(lines, i);
        i = new_i;
        raw.push((token.to_string(), first_sentence(&desc)));
    }
    // pass 2: strip the prefix shared by every xref (e.g. "jj-bookmark-").
    let prefix = shared_dash_prefix(raw.iter().map(|(t, _)| t.as_str()));
    raw.into_iter()
        .filter_map(|(token, desc)| {
            let child = token.strip_prefix(&prefix).unwrap_or(&token);
            is_valid_subcmd(child).then(|| ManpageSubcommand {
                name: child.to_ascii_lowercase(),
                desc,
            })
        })
        .collect()
}

/// longest common prefix of the tokens, truncated to the last `-` so the
/// remainder is a whole subcommand name. empty for fewer than two tokens.
fn shared_dash_prefix<'a>(tokens: impl Iterator<Item = &'a str>) -> String {
    let tokens: Vec<&str> = tokens.collect();
    let Some((first, rest)) = tokens.split_first() else {
        return String::new();
    };
    if rest.is_empty() {
        return String::new();
    }
    let mut len = first.len();
    for t in rest {
        len = first
            .bytes()
            .zip(t.bytes())
            .take(len)
            .take_while(|(a, b)| a == b)
            .count();
    }
    let common = &first[..len];
    match common.rfind('-') {
        Some(idx) => common[..=idx].to_string(),
        None => String::new(),
    }
}

/// collect an xref entry's description: text lines until the next
/// .TP/.SH/.SS boundary.
fn collect_xref_desc(lines: &[GroffLine], start: usize) -> (String, usize) {
    let mut acc: Vec<String> = Vec::new();
    let mut i = start;
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Text(t) => {
                acc.push(strip_groff_escapes(t));
                i += 1;
            }
            GroffLine::Macro { name, .. } if name == "TP" || name == "SH" || name == "SS" => break,
            _ => i += 1,
        }
    }
    (acc.join(" "), i)
}

/// collect the description for a subcommand entry. handles .RS/.RE blocks
/// and stops at the next .PP/.SH/.SS boundary.
fn collect_subcmd_desc(lines: &[GroffLine], start: usize) -> (String, usize) {
    let mut acc: Vec<String> = Vec::new();
    let mut i = start;
    while i < lines.len() {
        match &lines[i] {
            GroffLine::Macro { name, .. } if name == "RS" => {
                i += 1;
                // inside .RS — collect until .RE or boundary
                while i < lines.len() {
                    match &lines[i] {
                        GroffLine::Macro { name, .. } if name == "RE" => {
                            return (acc.join(" "), i + 1);
                        }
                        GroffLine::Text(t) => {
                            acc.push(t.clone());
                            i += 1;
                        }
                        GroffLine::Macro { name, .. }
                            if name == "PP" || name == "SH" || name == "SS" =>
                        {
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

/// take the first sentence (up to '.') as the description.
fn first_sentence(s: &str) -> String {
    let s = s.trim();
    match s.find('.') {
        Some(idx) if idx > 0 => s[..idx].trim().to_string(),
        _ => s.to_string(),
    }
}
