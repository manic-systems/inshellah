//! slice .SH sections (OPTIONS, NAME, SYNOPSIS, COMMANDS) out of a manpage.

use nom::{Parser, sequence::preceded};

use crate::parsers::help::{
    extract_usage_positionals as help_extract_usage_positionals, parse_usage_args,
    parse_usage_flags, skip_command_name,
};
use crate::parsers::manpage::desc;
use crate::parsers::manpage::groff::{
    GroffLine, strip_groff_escapes, strip_inline_macro_args, strip_space_macro_args,
};
use crate::parsers::manpage::{ManpageEntry, ManpageSubcommand, OwnedParam, OwnedSwitch};
use crate::types::Positional;

fn is_options_section(name: &str) -> bool {
    let upper = name.trim().to_ascii_uppercase();
    upper == "OPTIONS" || upper.contains("OPTION")
}

/// `also_ss` extends matching to `.SS` subsections.
fn section_heading(line: &GroffLine, also_ss: bool, header: impl Fn(&str) -> bool) -> bool {
    matches!(line, GroffLine::Macro { name, args }
        if (name == "SH" || (also_ss && name == "SS")) && header(args))
}

/// body of the first `.SH`/`.SS` section whose heading passes `header`, up to the
/// next `.SH` (and `.SS` when `also_ss`).
fn first_section_body(
    lines: &[GroffLine],
    also_ss: bool,
    header: impl Fn(&str) -> bool,
) -> Vec<GroffLine> {
    let mut i = 0;
    while i < lines.len() {
        if section_heading(&lines[i], also_ss, &header) {
            i += 1;
            return take_until_boundary(lines, &mut i, also_ss);
        }
        i += 1;
    }
    Vec::new()
}

/// concatenate the bodies of every `.SH` section passing `header`, for tools that
/// split one logical section across headings (git's COMMANDS groups).
fn all_section_bodies(lines: &[GroffLine], header: impl Fn(&str) -> bool) -> Vec<GroffLine> {
    let mut acc: Vec<GroffLine> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if section_heading(&lines[i], false, &header) {
            i += 1;
            acc.extend(take_until_boundary(lines, &mut i, false));
        } else {
            i += 1;
        }
    }
    acc
}

/// from `*i` (just past a heading), clone lines until the next `.SH` (and `.SS`
/// when `also_ss`), leaving `*i` on the boundary.
fn take_until_boundary(lines: &[GroffLine], i: &mut usize, also_ss: bool) -> Vec<GroffLine> {
    let mut acc: Vec<GroffLine> = Vec::new();
    while *i < lines.len() {
        if let GroffLine::Macro { name, .. } = &lines[*i]
            && (name == "SH" || (also_ss && name == "SS"))
        {
            break;
        }
        acc.push(lines[*i].clone());
        *i += 1;
    }
    acc
}

/// concatenate all option-like .SH sections (nix's "Options" + "Common
/// Options"), falling back to DESCRIPTION when none exist.
pub fn extract_options_section(lines: &[GroffLine]) -> Vec<GroffLine> {
    // synthetic empty .SH between sections so the description collector (which
    // stops on SH/SS) can't bleed one section's last description into the next.
    let mut acc: Vec<GroffLine> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if section_heading(&lines[i], false, is_options_section) {
            i += 1;
            if !acc.is_empty() {
                acc.push(GroffLine::Macro {
                    name: "SH".to_string(),
                    args: String::new(),
                });
            }
            acc.extend(take_until_boundary(lines, &mut i, false));
        } else {
            i += 1;
        }
    }
    if !acc.is_empty() {
        return acc;
    }
    extract_named_section(lines, "DESCRIPTION")
}

fn extract_named_section(lines: &[GroffLine], section_name: &str) -> Vec<GroffLine> {
    first_section_body(lines, false, |args| {
        args.trim().eq_ignore_ascii_case(section_name)
    })
}

/// NAME reads "command \- short description"; return the part after the
/// separator. handles `\-` (groff) and ` - ` (plain text).
pub fn extract_name_description(lines: &[GroffLine]) -> Option<String> {
    let mut i = 0;
    while i < lines.len() {
        if let GroffLine::Macro { name, args } = &lines[i]
            && name == "SH"
            && args.trim().eq_ignore_ascii_case("NAME")
        {
            i += 1;
            let mut acc: Vec<String> = Vec::new();
            while i < lines.len() {
                if let GroffLine::Macro { name, .. } = &lines[i]
                    && name == "SH"
                {
                    break;
                }
                match &lines[i] {
                    GroffLine::Text(t) => acc.push(t.clone()),
                    GroffLine::Macro { name, args }
                        if matches!(name.as_str(), "B" | "BI" | "BR" | "I" | "IR") =>
                    {
                        let text = strip_groff_escapes(&strip_inline_macro_args(args));
                        let text = text.trim();
                        if !text.is_empty() {
                            acc.push(text.to_string());
                        }
                    }
                    GroffLine::Macro { name, args } if name == "Nm" => {
                        let text = strip_groff_escapes(args);
                        let text = text.trim();
                        if !text.is_empty() {
                            acc.push(text.to_string());
                        }
                    }
                    GroffLine::Macro { name, args } if name == "Nd" => {
                        let text = strip_groff_escapes(args);
                        let text = text.trim();
                        if !text.is_empty() {
                            acc.push(format!("\\- {text}"));
                        }
                    }
                    _ => (),
                }
                i += 1;
            }
            let full = acc.join(" ").trim().to_string();
            return split_name_separator(&full);
        }
        i += 1;
    }
    None
}

/// split on the earliest `\-` or ` - `, returning the trimmed part after it.
fn split_name_separator(full: &str) -> Option<String> {
    let groff_idx = find_padded(full, "\\-");
    let dash_idx = find_padded(full, " - ");
    let idx = match (groff_idx, dash_idx) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }?;
    let after = if full[idx..].starts_with("\\-") {
        &full[idx + 2..]
    } else {
        &full[idx + 3..]
    };
    let desc = after.trim().to_string();
    if desc.is_empty() { None } else { Some(desc) }
}

fn find_padded(s: &str, needle: &str) -> Option<usize> {
    s.find(needle)
}

/// command name from SYNOPSIS: leading word tokens until an argument-looking one
/// (starts with [, <, -, etc.).
pub fn extract_synopsis_command(contents: &str) -> Option<String> {
    // italic marks a param, not a command word; rewrite \fI...\fR to angle brackets
    // (in extract_cmd's stop set) before classification strips font info.
    let preprocessed: Vec<String> = contents
        .split('\n')
        .map(replace_italic_with_angles)
        .collect();
    let classified: Vec<GroffLine> = preprocessed
        .iter()
        .map(|line| crate::parsers::manpage::groff::classify_line(line))
        .collect();
    let mut i = 0;
    while i < classified.len() {
        if let Some((stop_on_ss, content_start)) = synopsis_heading_at(&classified, i) {
            i = content_start;
            while i < classified.len() {
                match &classified[i] {
                    GroffLine::Macro { name, .. }
                        if name == "SH" || (stop_on_ss && name == "SS") =>
                    {
                        return None;
                    }
                    GroffLine::Text(text) => {
                        let trimmed = text.trim();
                        if let Some(cmd) = synopsis_command_candidate(trimmed, true) {
                            return Some(cmd);
                        }
                        i += 1;
                    }
                    GroffLine::Macro { name, args } if name == "SY" => {
                        let text = strip_groff_escapes(args);
                        if let Some(cmd) = synopsis_command_candidate(text.trim(), false) {
                            return Some(cmd);
                        }
                        i += 1;
                    }
                    GroffLine::Macro { name, args }
                        if matches!(name.as_str(), "B" | "BI" | "BR") =>
                    {
                        let text = render_synopsis_command_macro(name, args);
                        if let Some(cmd) = synopsis_command_candidate(text.trim(), false) {
                            return Some(cmd);
                        }
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            return None;
        }
        i += 1;
    }
    None
}

fn synopsis_heading_at(lines: &[GroffLine], i: usize) -> Option<(bool, usize)> {
    let GroffLine::Macro { name, args } = &lines[i] else {
        return None;
    };
    if !matches!(name.as_str(), "SH" | "SS") {
        return None;
    }
    if args.trim().eq_ignore_ascii_case("SYNOPSIS") {
        return Some((name == "SS", i + 1));
    }
    if !args.trim().is_empty() {
        return None;
    }
    let mut j = i + 1;
    while j < lines.len() {
        match &lines[j] {
            GroffLine::Text(text) if text.trim().eq_ignore_ascii_case("SYNOPSIS") => {
                return Some((name == "SS", j + 1));
            }
            GroffLine::Blank | GroffLine::Comment => j += 1,
            _ => return None,
        }
    }
    None
}

fn render_synopsis_command_macro(name: &str, args: &str) -> String {
    match name {
        "B" | "I" => strip_space_macro_args(args),
        _ => strip_groff_escapes(&strip_inline_macro_args(args))
            .trim()
            .to_string(),
    }
}

fn synopsis_command_candidate(line: &str, reject_long_unmarked: bool) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.ends_with(':') {
        return None;
    }
    let cmd = extract_cmd(trimmed)?;
    if cmd.starts_with('.') {
        return None;
    }
    if looks_like_synopsis_prose(trimmed, &cmd, reject_long_unmarked) {
        None
    } else {
        Some(cmd)
    }
}

fn looks_like_synopsis_prose(line: &str, cmd: &str, reject_long_unmarked: bool) -> bool {
    let Some(first) = cmd.split_whitespace().next() else {
        return true;
    };
    if matches!(
        first.to_ascii_lowercase().as_str(),
        "a" | "an" | "and" | "or" | "the" | "this" | "these"
    ) {
        return true;
    }

    let line_has_invocation_marker = line.split_whitespace().any(|word| {
        word.starts_with('[')
            || word.starts_with('<')
            || word.starts_with('-')
            || word.starts_with('{')
    }) || line.contains('|');
    if line.ends_with('.') && !line_has_invocation_marker {
        return true;
    }
    if reject_long_unmarked && cmd.split_whitespace().count() > 3 && !line_has_invocation_marker {
        return true;
    }
    let looks_like_sentence_starter = first.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && first.chars().skip(1).all(|c| c.is_ascii_lowercase());
    looks_like_sentence_starter
        && line.split_whitespace().count() > 1
        && !line_has_invocation_marker
}

/// replace \fI...\f[RP] with <...> so extract_cmd sees italic params as non-word
/// tokens.
///
/// exception: some synopses italicise the command name itself (git-am.1's
/// `\fIgit am\fR`). when the first italic block sits at line start and looks like
/// a command word, leave it bare so extract_cmd takes it as the command.
fn replace_italic_with_angles(line: &str) -> String {
    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len);
    let mut i = 0;
    let mut command_consumed = false;
    while i < len {
        // byte-compare to avoid panicking on non-ASCII char boundaries
        if i + 3 <= len && &bytes[i..i + 3] == b"\\fI" {
            // scan to the closing \fR or \fP
            let inner_start = i + 3;
            let mut j = inner_start;
            while j < len && bytes[j] != b'\\' {
                j += 1;
            }
            if j + 3 <= len
                && bytes[j] == b'\\'
                && bytes[j + 1] == b'f'
                && (bytes[j + 2] == b'R' || bytes[j + 2] == b'P')
            {
                let inner = &line[inner_start..j];
                let at_line_start = !command_consumed && line[..i].chars().all(char::is_whitespace);
                if at_line_start && italic_looks_like_command(inner) {
                    out.push_str(inner);
                    command_consumed = true;
                } else {
                    out.push('<');
                    out.push_str(inner);
                    out.push('>');
                }
                i = j + 3;
                continue;
            }
        }
        let c = line[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// italic content looks like a command name, not a placeholder: lowercase,
/// digits, hyphens, underscores, dots, spaces only.
fn italic_looks_like_command(inner: &str) -> bool {
    let stripped = strip_groff_escapes(inner);
    let trimmed = stripped.trim();
    !trimmed.is_empty()
        && trimmed.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.' | ' ')
        })
}

fn extract_cmd(line: &str) -> Option<String> {
    let words: Vec<&str> = line.split(' ').filter(|w| !w.is_empty()).collect();
    let is_cmd_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
    let mut taken: Vec<&str> = Vec::new();
    for word in words {
        let first = word.chars().next().unwrap();
        if matches!(first, '[' | '-' | '<' | '(' | '{') {
            break;
        }
        if word.chars().all(is_cmd_char) {
            taken.push(word);
        } else {
            break;
        }
    }
    if taken.is_empty() {
        None
    } else {
        Some(taken.join(" "))
    }
}

/// the SYNOPSIS section lines. boundary follows the matched heading kind: `.SS
/// SYNOPSIS` ends at the next `.SS`, `.SH SYNOPSIS` runs through any `.SS` until
/// the next `.SH`.
fn extract_synopsis_section(lines: &[GroffLine]) -> Vec<GroffLine> {
    let mut i = 0;
    while i < lines.len() {
        if let GroffLine::Macro { name, args } = &lines[i]
            && matches!(name.as_str(), "SH" | "SS")
            && args.trim().eq_ignore_ascii_case("SYNOPSIS")
        {
            let stop_on_ss = name == "SS";
            i += 1;
            return take_until_boundary(lines, &mut i, stop_on_ss);
        }
        i += 1;
    }
    Vec::new()
}

pub fn extract_synopsis_positionals(lines: &[GroffLine]) -> Vec<(String, Positional)> {
    let full = join_synopsis_text(lines);
    if full.is_empty() {
        return Vec::new();
    }
    let result: nom::IResult<&str, Vec<(&str, Positional)>> =
        preceded(skip_command_name, parse_usage_args).parse(&full);
    match result {
        Ok((_, map)) => map
            .into_iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// positionals from clap-style `Usage:` text embedded in a section, mainly `.SH
/// SUBCOMMAND` bodies parsed as standalone fragments.
pub fn extract_usage_positionals_from_lines(lines: &[GroffLine]) -> Vec<(String, Positional)> {
    let text = render_plain_text_lines(lines);
    if text.trim().is_empty() {
        return Vec::new();
    }
    match help_extract_usage_positionals(&text) {
        Ok((_, positionals)) => positionals
            .into_iter()
            .map(|(name, positional)| (name.to_ascii_lowercase(), positional))
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn render_plain_text_lines(lines: &[GroffLine]) -> String {
    let mut out = String::new();
    for line in lines {
        match line {
            GroffLine::Text(text) => {
                out.push_str(text);
                out.push('\n');
            }
            GroffLine::Macro { name, args } if name == "B" || name == "I" || name == "SY" => {
                let text = strip_space_macro_args(args);
                if !text.is_empty() {
                    out.push_str(&text);
                    out.push('\n');
                }
            }
            GroffLine::Macro { name, args }
                if matches!(name.as_str(), "BI" | "BR" | "IB" | "IR" | "RB" | "RI") =>
            {
                let text = strip_groff_escapes(&strip_inline_macro_args(args));
                let text = text.trim();
                if !text.is_empty() {
                    out.push_str(text);
                    out.push('\n');
                }
            }
            GroffLine::Blank => out.push('\n'),
            GroffLine::Macro { name, .. } if name == "br" => out.push('\n'),
            _ => (),
        }
    }
    out
}

/// join the SYNOPSIS section into one line of plain text, stripping groff escapes
/// and inline font macros. shared by the positional and flag extractors so both
/// see identical input.
fn join_synopsis_text(lines: &[GroffLine]) -> String {
    let section = extract_synopsis_section(lines);
    let mut acc: Vec<String> = Vec::new();
    for line in section {
        match line {
            GroffLine::Macro { name, .. } if name == "SS" || name == "br" => break,
            GroffLine::Macro { name, args } if name == "SY" => {
                let text = strip_groff_escapes(&args).trim().to_string();
                if !text.is_empty() {
                    acc.push(text);
                }
            }
            GroffLine::Macro { name, args } if name == "I" => {
                let text = strip_groff_escapes(&args).trim().to_string();
                if !text.is_empty() {
                    acc.push(format!("<{text}>"));
                }
            }
            GroffLine::Macro { name, args } if name == "IR" => {
                let text = render_leading_italic_arg(&args);
                if !text.is_empty() {
                    acc.push(text);
                }
            }
            GroffLine::Text(t) => {
                let text = strip_groff_escapes(&t).trim().to_string();
                if !text.is_empty() {
                    acc.push(text);
                }
            }
            GroffLine::Macro { name, args } if name == "B" => {
                let text = strip_space_macro_args(&args);
                if !text.is_empty() {
                    acc.push(text);
                }
            }
            GroffLine::Macro { name, args }
                if matches!(name.as_str(), "B" | "BI" | "BR" | "IB" | "RB" | "RI") =>
            {
                let text = strip_groff_escapes(&strip_inline_macro_args(&args));
                let text = text.trim();
                if !text.is_empty() {
                    acc.push(text.to_string());
                }
            }
            _ => (),
        }
    }
    acc.join(" ").trim().to_string()
}

fn render_leading_italic_arg(args: &str) -> String {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let (first, rest) = match trimmed.find(char::is_whitespace) {
        Some(idx) => (&trimmed[..idx], trimmed[idx..].trim()),
        None => (trimmed, ""),
    };
    let first = strip_groff_escapes(first).trim().to_string();
    if first.is_empty() {
        return String::new();
    }
    let rest = strip_groff_escapes(&strip_inline_macro_args(rest));
    let rest = rest.trim();
    if rest.is_empty() {
        format!("<{first}>")
    } else {
        format!("<{first}> {rest}")
    }
}

/// flag-tagged entries from the SYNOPSIS line. some manpages (nix-env, sed)
/// declare flags only in the synopsis, never in OPTIONS, so the body-only pass
/// misses them. callers merge with body entries; body wins on dup names since its
/// descriptions are richer.
pub fn extract_synopsis_flags(lines: &[GroffLine]) -> Vec<ManpageEntry> {
    let full = join_synopsis_text(lines);
    if full.is_empty() {
        return Vec::new();
    }
    let result: nom::IResult<&str, Vec<(OwnedSwitch, Option<OwnedParam>)>> =
        preceded(skip_command_name, parse_usage_flags).parse(&full);
    match result {
        Ok((_, pairs)) => pairs
            .into_iter()
            .map(|(switch, param)| ManpageEntry {
                switch,
                param,
                desc: String::new(),
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// first-positional choices from prose lists in DESCRIPTION, returned as
/// subcommand-like candidates.
///
/// getent(1) is the motivating shape: a `database` positional in the synopsis,
/// the database names documented as a tagged list under DESCRIPTION rather than
/// as subcommands.
pub fn extract_description_positionals(lines: &[GroffLine]) -> Vec<ManpageSubcommand> {
    let description = extract_named_section(lines, "DESCRIPTION");
    if description.is_empty() || !description_mentions_listed_database(&description) {
        return Vec::new();
    }

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut i = 0;
    let mut in_database_list = false;
    while i < description.len() {
        match &description[i] {
            GroffLine::Text(text)
                if text.to_ascii_lowercase().contains("listed below")
                    || text.to_ascii_lowercase().contains("may be any of") =>
            {
                in_database_list = true;
                i += 1;
            }
            GroffLine::Macro { name, .. } if name == "TP" && in_database_list => {
                if i + 1 >= description.len() {
                    break;
                }
                let Some(name) = description_tag_name(&description[i + 1]) else {
                    i += 1;
                    continue;
                };
                if !is_description_choice_name(&name) {
                    i += 1;
                    continue;
                }
                let (desc, new_i) = collect_description_choice_desc(&description, i + 2);
                if seen.insert(name.clone()) {
                    out.push(ManpageSubcommand::new(name, desc));
                }
                i = new_i;
            }
            _ => {
                i += 1;
            }
        }
    }
    out
}

fn description_mentions_listed_database(lines: &[GroffLine]) -> bool {
    let mut saw_database = false;
    let mut saw_list = false;
    for line in lines {
        let text = match line {
            GroffLine::Text(text) => text.clone(),
            GroffLine::Macro { name, args }
                if matches!(name.as_str(), "B" | "BI" | "BR" | "I" | "IR" | "RI") =>
            {
                strip_groff_escapes(&strip_inline_macro_args(args))
            }
            _ => String::new(),
        };
        let lower = text.to_ascii_lowercase();
        saw_database |= lower.contains("database");
        saw_list |= lower.contains("listed below") || lower.contains("may be any of");
    }
    saw_database && saw_list
}

fn description_tag_name(line: &GroffLine) -> Option<String> {
    match line {
        GroffLine::Text(text) => Some(text.trim().to_string()),
        GroffLine::Macro { name, args }
            if matches!(name.as_str(), "B" | "BI" | "BR" | "I" | "IR") =>
        {
            Some(
                strip_groff_escapes(&strip_inline_macro_args(args))
                    .trim()
                    .to_string(),
            )
        }
        _ => None,
    }
}

fn is_description_choice_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn collect_description_choice_desc(lines: &[GroffLine], start: usize) -> (String, usize) {
    let (body, i) = desc::collect(
        lines,
        start,
        desc::DescOpts {
            boundaries: &["TP", "SH", "SS"],
            skip_rs: false,
            stop_on_blank: false,
            tags: desc::TagMacros::Common,
        },
    );
    (first_sentence(&body), i)
}

fn first_sentence(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for marker in [". ", ".) "] {
        if let Some(idx) = text.find(marker) {
            return text[..idx + 1].trim().to_string();
        }
    }
    text.trim().to_string()
}

fn is_commands_section(name: &str) -> bool {
    let trimmed = name.trim();
    // strip a trailing parenthetical so git.1's "HIGH-LEVEL COMMANDS (PORCELAIN)"
    // is treated as "HIGH-LEVEL COMMANDS".
    let core = match (trimmed.rfind('('), trimmed.ends_with(')')) {
        (Some(open), true) => trimmed[..open].trim(),
        _ => trimmed,
    };
    let upper = core.to_ascii_uppercase();
    if upper == "COMMAND" || upper == "COMMANDS" {
        return true;
    }
    // headings ending in " COMMANDS" ("GIT COMMANDS", ...). the leading space
    // rejects "COMMAND LINE OPTIONS".
    upper.ends_with(" COMMANDS")
}

pub fn extract_commands_section(lines: &[GroffLine]) -> Vec<GroffLine> {
    all_section_bodies(lines, is_commands_section)
}

/// body of a `.SH SUBCOMMAND(S)` section. jj/clap group manpages enumerate
/// children there as `.TP` cross-references, not the inline `*COMMANDS` layout
/// `extract_commands_section` handles.
pub fn extract_subcommand_list_section(lines: &[GroffLine]) -> Vec<GroffLine> {
    all_section_bodies(lines, |args| {
        matches!(
            args.trim().to_ascii_uppercase().as_str(),
            "SUBCOMMAND" | "SUBCOMMANDS"
        )
    })
}

/// SUBCOMMAND-style sections (clap-generated manpages put each subcommand under
/// its own .SH SUBCOMMAND header with a Usage: line). (name, description, lines)
/// triples so the caller can re-parse each as its own help_result.
pub fn extract_subcommand_sections(lines: &[GroffLine]) -> Vec<(String, String, Vec<GroffLine>)> {
    let mut sections: Vec<Vec<GroffLine>> = Vec::new();
    let mut current_name: Option<String> = None;
    let mut current: Vec<GroffLine> = Vec::new();
    for line in lines {
        if let GroffLine::Macro { name, args } = line
            && name == "SH"
        {
            if current_name.is_some() {
                sections.push(std::mem::take(&mut current));
            }
            let n = args.trim().to_ascii_uppercase();
            if n == "SUBCOMMAND" || n == "SUBCOMMANDS" {
                current_name = Some(n);
            } else {
                current_name = None;
            }
            continue;
        }
        if current_name.is_some() {
            current.push(line.clone());
        }
    }
    if current_name.is_some() {
        sections.push(current);
    }

    let mut out = Vec::new();
    for section in sections {
        let mut subcmd_name: Option<String> = None;
        let mut desc_lines: Vec<String> = Vec::new();
        for line in &section {
            if subcmd_name.is_some() {
                break;
            }
            match line {
                GroffLine::Text(t) => match find_usage_name(t) {
                    Some(name) => subcmd_name = Some(name),
                    None => desc_lines.push(t.clone()),
                },
                GroffLine::Macro { name, args }
                    if matches!(name.as_str(), "TP" | "B" | "BI" | "BR") =>
                {
                    let text = strip_groff_escapes(&strip_inline_macro_args(args));
                    let text = text.trim();
                    subcmd_name = find_usage_name(text);
                }
                _ => (),
            }
        }
        if let Some(name) = subcmd_name {
            let desc_raw = desc_lines.join(" ");
            let desc = strip_groff_escapes(&desc_raw).trim().to_string();
            let desc = strip_backtick_words(&desc);
            out.push((name, desc, section));
        }
    }
    out
}

fn find_usage_name(text: &str) -> Option<String> {
    const MARKER: &str = "Usage: ";
    let idx = text.find(MARKER)?;
    let after = &text[idx + MARKER.len()..];
    let end = after
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(after.len());
    if end == 0 {
        None
    } else {
        Some(after[..end].to_string())
    }
}

fn strip_backtick_words(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'`'
            && let Some(end) = s[i + 1..].find('`')
        {
            out.push_str(&s[i + 1..i + 1 + end]);
            i += end + 2;
            continue;
        }
        let c = s[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}
