// SPDX-License-Identifier: EUPL-1.2
//! parse unix manpages (groff/mdoc) into a structured result.

mod commands;
mod desc;
mod groff;
mod mdoc;
mod sections;
mod strategies;

use std::io::{self, Read};
use std::path::Path;

use crate::types::Positional;

pub use self::groff::{GroffLine, classify_line, strip_groff_escapes};
pub use self::sections::{extract_subcommand_sections, extract_synopsis_command};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedSwitch {
    Short(char),
    Long(String),
    Both(char, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnedParam {
    Mandatory(String),
    Optional(String),
}

#[derive(Debug, Clone)]
pub struct ManpageEntry {
    pub switch: OwnedSwitch,
    pub param: Option<OwnedParam>,
    pub desc: String,
}

#[derive(Debug, Clone)]
pub struct ManpageSubcommand {
    pub name: String,
    pub desc: String,
}

#[derive(Debug, Clone, Default)]
pub struct ManpageResult {
    pub entries: Vec<ManpageEntry>,
    pub subcommands: Vec<ManpageSubcommand>,
    /// prose-mined positional-slot values, kept out of `subcommands` so they never
    /// flow into real-child paths (recursion, supplement, prefix stripping).
    pub positional_choices: Vec<ManpageSubcommand>,
    pub positionals: Vec<(String, Positional)>,
    pub description: String,
}

impl ManpageResult {
    /// canonicalise so the cache holds one shape. runs once at the end of every
    /// parse path.
    pub fn normalize(&mut self) {
        let entries = std::mem::take(&mut self.entries);
        self.entries = dedup_entries(merge_short_long_pairs(entries));
        for e in &mut self.entries {
            clamp_description(&mut e.desc);
        }
        for sc in self.subcommands.iter_mut().chain(&mut self.positional_choices) {
            clamp_description(&mut sc.desc);
        }
        clamp_description(&mut self.description);
    }
}

/// soft cap on a tooltip description; nushell truncates the menu anyway.
const MAX_DESC_LEN: usize = 256;

/// break on the first space at or after the cap so a word is never split; an
/// unbroken token is hard-cut.
pub(crate) fn clamp_description(desc: &mut String) {
    let Some((cut, _)) = desc.char_indices().nth(MAX_DESC_LEN) else {
        return;
    };
    let end = desc[cut..]
        .find(char::is_whitespace)
        .map(|off| cut + off)
        .unwrap_or(cut);
    desc.truncate(end);
    let trimmed = desc.trim_end().len();
    desc.truncate(trimmed);
    desc.push('…');
}

fn entry_key(e: &ManpageEntry) -> String {
    match &e.switch {
        OwnedSwitch::Short(c) => format!("-{c}"),
        OwnedSwitch::Long(l) | OwnedSwitch::Both(_, l) => format!("--{l}"),
    }
}

fn entry_score(e: &ManpageEntry) -> i32 {
    let switch_bonus = if matches!(e.switch, OwnedSwitch::Both(_, _)) {
        10
    } else {
        0
    };
    let param_bonus = if e.param.is_some() { 5 } else { 0 };
    let desc_bonus = (e.desc.len() / 10).min(5) as i32;
    switch_bonus + param_bonus + desc_bonus
}

type ShortAliasCandidate = (usize, char, Option<OwnedParam>);
type LongAliasCandidate<'a> = (usize, &'a str);

/// collapse duplicate entries for the same flag. manpages emit dups: clap lists
/// inherited globals, btrfs documents a flag as both a deprecated alias and a
/// global option, example blocks restate flags.
///
/// per key keep the highest-scoring entry, then strip standalone Shorts whose
/// char a surviving `Both` already covers. survivor sits at its key's first
/// occurrence so order is stable.
pub fn dedup_entries(entries: Vec<ManpageEntry>) -> Vec<ManpageEntry> {
    use std::collections::HashMap;
    use std::collections::HashSet;

    if entries.len() < 2 {
        return entries;
    }

    let mut best: HashMap<String, usize> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        let key = entry_key(e);
        match best.get(&key) {
            Some(&prev) if entry_score(&entries[prev]) >= entry_score(e) => {}
            _ => {
                best.insert(key, i);
            }
        }
    }

    let mut covered: HashSet<char> = HashSet::new();
    for &idx in best.values() {
        if let OwnedSwitch::Both(c, _) = &entries[idx].switch {
            covered.insert(*c);
        }
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<ManpageEntry> = Vec::with_capacity(entries.len());
    for e in entries.iter() {
        let key = entry_key(e);
        if seen.contains(&key) {
            continue;
        }
        if let OwnedSwitch::Short(c) = &e.switch
            && covered.contains(c)
        {
            continue;
        }
        seen.insert(key.clone());
        let best_idx = *best.get(&key).unwrap();
        out.push(entries[best_idx].clone());
    }
    out
}

/// merge non-adjacent Short/Long entries into a single `Both`. some styles emit
/// `-h` and `--help` as independent .TP/.IP blocks, not the comma-joined form
/// `combine_short_long_alternates` handles.
///
/// conservative since a wrong alias is worse than a missing one: the two must be
/// the only entries sharing that exact non-empty description, the short must
/// abbreviate the long, and tiny generic descriptions pair only for obvious
/// aliases.
pub fn merge_short_long_pairs(entries: Vec<ManpageEntry>) -> Vec<ManpageEntry> {
    use std::collections::HashMap;
    use std::collections::HashSet;

    // a flag restated across sections can yield all three forms (Both, standalone
    // Short, standalone Long); index existing Both so pairing doesn't emit a
    // second Both with the same (c, l).
    let mut existing_both: HashSet<(char, &str)> = HashSet::new();
    for e in entries.iter() {
        if let OwnedSwitch::Both(c, l) = &e.switch {
            existing_both.insert((*c, l.as_str()));
        }
    }

    let mut shorts_by_desc: HashMap<&str, Vec<ShortAliasCandidate>> = HashMap::new();
    let mut longs_by_desc: HashMap<&str, Vec<LongAliasCandidate<'_>>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        if let OwnedSwitch::Short(c) = &e.switch
            && !e.desc.is_empty()
        {
            shorts_by_desc
                .entry(e.desc.as_str())
                .or_default()
                .push((i, *c, e.param.clone()));
        }
        if let OwnedSwitch::Long(l) = &e.switch
            && !e.desc.is_empty()
        {
            longs_by_desc
                .entry(e.desc.as_str())
                .or_default()
                .push((i, l.as_str()));
        }
    }
    if shorts_by_desc.is_empty() || longs_by_desc.is_empty() {
        return entries;
    }

    let mut to_drop: HashSet<usize> = HashSet::new();
    let mut out: Vec<ManpageEntry> = Vec::with_capacity(entries.len());
    for (i, e) in entries.iter().enumerate() {
        if let OwnedSwitch::Long(l) = &e.switch
            && !e.desc.is_empty()
            && let Some(shorts) = shorts_by_desc.get(e.desc.as_str())
            && let Some(longs) = longs_by_desc.get(e.desc.as_str())
            && shorts.len() == 1
            && longs.len() == 1
        {
            let (s_idx, c, s_param) = &shorts[0];
            if existing_both.contains(&(*c, l.as_str())) {
                // matching Both already present; drop the redundant standalone pair.
                to_drop.insert(*s_idx);
                to_drop.insert(i);
                out.push(e.clone());
            } else if *s_idx != i
                && !to_drop.contains(s_idx)
                && plausible_description_alias(*c, l, &e.desc)
            {
                to_drop.insert(*s_idx);
                out.push(ManpageEntry {
                    switch: OwnedSwitch::Both(*c, l.clone()),
                    param: e.param.clone().or_else(|| s_param.clone()),
                    desc: e.desc.clone(),
                });
            } else {
                out.push(e.clone());
            }
        } else {
            out.push(e.clone());
        }
    }
    if to_drop.is_empty() {
        return out;
    }
    out.into_iter()
        .enumerate()
        .filter_map(|(i, e)| (!to_drop.contains(&i)).then_some(e))
        .collect()
}

fn plausible_description_alias(short: char, long: &str, desc: &str) -> bool {
    let short = short.to_ascii_lowercase();
    let first = long
        .chars()
        .find(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase());
    if first != Some(short) {
        return false;
    }
    if is_common_obvious_alias(short, long) {
        return true;
    }
    desc.split_whitespace().count() >= 4
}

fn is_common_obvious_alias(short: char, long: &str) -> bool {
    matches!(
        (short, long.to_ascii_lowercase().as_str()),
        ('h', "help") | ('v', "verbose") | ('v', "version")
    )
}

pub fn parse_manpage_lines(lines: &[GroffLine]) -> ManpageResult {
    let mut result = parse_manpage_lines_raw(lines);
    result.normalize();
    result
}

fn parse_manpage_lines_raw(lines: &[GroffLine]) -> ManpageResult {
    if mdoc::is_mdoc(lines) {
        mdoc::parse_mdoc_lines(lines)
    } else {
        let options_section = sections::extract_options_section(lines);
        let mut entries = strategies::extract_entries(&options_section);
        // flags declared only in the synopsis, never in OPTIONS. body wins on dup
        // names since it carries the description.
        let synopsis_flags = sections::extract_synopsis_flags(lines);
        if !synopsis_flags.is_empty() {
            let have_long: std::collections::HashSet<String> = entries
                .iter()
                .filter_map(|e| match &e.switch {
                    OwnedSwitch::Long(l) | OwnedSwitch::Both(_, l) => Some(l.to_ascii_lowercase()),
                    _ => None,
                })
                .collect();
            let have_short: std::collections::HashSet<char> = entries
                .iter()
                .filter_map(|e| match &e.switch {
                    OwnedSwitch::Short(c) | OwnedSwitch::Both(c, _) => Some(*c),
                    _ => None,
                })
                .collect();
            for e in synopsis_flags {
                let dup = match &e.switch {
                    OwnedSwitch::Long(l) => have_long.contains(&l.to_ascii_lowercase()),
                    OwnedSwitch::Short(c) => have_short.contains(c),
                    OwnedSwitch::Both(c, l) => {
                        have_short.contains(c) || have_long.contains(&l.to_ascii_lowercase())
                    }
                };
                if !dup {
                    entries.push(e);
                }
            }
        }
        let mut positionals = sections::extract_synopsis_positionals(lines);
        let commands_section = sections::extract_commands_section(lines);
        let mut subcommands = commands::extract_subcommands_from_commands(&commands_section);
        if subcommands.is_empty() {
            // jj/clap group pages list children as `.SH SUBCOMMANDS` xref entries
            // instead of an inline COMMANDS layout.
            let xref_section = sections::extract_subcommand_list_section(lines);
            if !xref_section.is_empty() {
                subcommands = commands::extract_subcommand_xrefs(&xref_section);
            }
        }
        if !subcommands.is_empty() {
            // drop the synopsis `<subcommands>` placeholder so it can't fall
            // through to file completion when a prefix matches no child.
            positionals.retain(|(name, _)| {
                !matches!(
                    name.to_ascii_lowercase().as_str(),
                    "subcommand" | "subcommands" | "command" | "commands"
                )
            });
        }
        // prose-mined choices go to their own channel; skip collisions with a real
        // subcommand name.
        let positional_choices = sections::extract_description_positionals(lines)
            .into_iter()
            .filter(|choice| {
                !subcommands
                    .iter()
                    .any(|sc| sc.name.eq_ignore_ascii_case(&choice.name))
            })
            .collect();
        ManpageResult {
            entries,
            subcommands,
            positional_choices,
            positionals,
            description: String::new(),
        }
    }
}

pub fn parse_manpage_string(contents: &str) -> ManpageResult {
    let lines: Vec<GroffLine> = contents.split('\n').map(classify_line).collect();
    let mut result = parse_manpage_lines(&lines);
    if let Some(desc) = sections::extract_name_description(&lines) {
        result.description = desc;
    }
    result
}

/// also pull clap-style `.SH SUBCOMMAND` sections out as separate per-subcommand
/// results; the parent's subcommand list comes from their names. each sub keyed
/// by full command ("nh os").
pub fn parse_manpage_with_subs(contents: &str) -> (ManpageResult, Vec<(String, ManpageResult)>) {
    let lines: Vec<GroffLine> = contents.split('\n').map(classify_line).collect();
    let mut result = parse_manpage_lines(&lines);
    if let Some(desc) = sections::extract_name_description(&lines) {
        result.description = desc;
    }
    let sub_sections = sections::extract_subcommand_sections(&lines);
    if !sub_sections.is_empty() {
        // SUBCOMMAND-section names are authoritative for clap manpages.
        result.subcommands = sub_sections
            .iter()
            .map(|(name, desc, _)| {
                let mut desc = desc.clone();
                clamp_description(&mut desc);
                ManpageSubcommand {
                    name: name.to_ascii_lowercase(),
                    desc,
                }
            })
            .collect();
    }
    // clap puts flags directly under the .SH SUBCOMMAND header with no inner .SH,
    // so parse_manpage_lines (which wants a child OPTIONS) comes back empty; parse
    // each body with the same strategy-picker as top-level OPTIONS.
    let subs: Vec<(String, ManpageResult)> = sub_sections
        .into_iter()
        .map(|(name, desc, lines)| {
            let mut positionals = sections::extract_synopsis_positionals(&lines);
            if positionals.is_empty() {
                positionals = sections::extract_usage_positionals_from_lines(&lines);
            }
            let mut sub_result = ManpageResult {
                entries: strategies::extract_entries(&lines),
                subcommands: Vec::new(),
                positional_choices: Vec::new(),
                positionals,
                description: desc,
            };
            sub_result.normalize();
            (name, sub_result)
        })
        .collect();
    (result, subs)
}

/// decompresses .gz (most installed manpages).
pub fn read_manpage_file<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let path = path.as_ref();
    let bytes = std::fs::read(path)?;
    if path.extension().and_then(|e| e.to_str()) == Some("gz") {
        let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
        let mut out = String::new();
        decoder.read_to_string(&mut out)?;
        Ok(out)
    } else {
        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

pub fn parse_manpage_file<P: AsRef<Path>>(path: P) -> io::Result<ManpageResult> {
    let contents = read_manpage_file(path)?;
    Ok(parse_manpage_string(&contents))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_leaves_short_descriptions_untouched() {
        let mut d = "a short description".to_string();
        clamp_description(&mut d);
        assert_eq!(d, "a short description");
        assert!(!d.ends_with('…'));
    }

    #[test]
    fn clamp_breaks_long_descriptions_on_a_word_boundary() {
        let word = "lorem ipsum ";
        let mut d = word.repeat(40); // ~480 chars
        clamp_description(&mut d);
        assert!(d.ends_with('…'));
        assert!(d.chars().count() <= MAX_DESC_LEN + 16);
        let body = d.trim_end_matches('…');
        assert!(!body.ends_with(' '));
        assert!(body.split(' ').all(|w| w.is_empty() || w == "lorem" || w == "ipsum"));
    }

    #[test]
    fn clamp_hard_cuts_an_unbroken_token_with_no_trailing_space() {
        let mut d = "x".repeat(400);
        clamp_description(&mut d);
        assert_eq!(d.chars().count(), MAX_DESC_LEN + 1);
        assert!(d.ends_with('…'));
    }

    #[test]
    fn clamp_respects_utf8_char_boundaries() {
        // multibyte chars straddling the cap must not split a code point.
        let mut d = "é".repeat(400);
        clamp_description(&mut d);
        assert!(d.ends_with('…'));
        assert!(d.chars().all(|c| c == 'é' || c == '…'));
    }

    const TP_MANPAGE: &str = r#".TH FOO 1 "2024" "1.0" "User Commands"
.SH NAME
foo \- a synthetic test command
.SH SYNOPSIS
.B foo
[\fIOPTIONS\fR] <input> [output]
.SH OPTIONS
.TP
\fB\-v\fR, \fB\-\-verbose\fR
increase output verbosity
.TP
\fB\-o\fR \fIFILE\fR, \fB\-\-output\fR=\fIFILE\fR
write to FILE
.TP
\fB\-h\fR, \fB\-\-help\fR
show this help and exit
"#;

    const HP_MANPAGE: &str = r#".TH BAT "1"
.SH NAME
bat \- demo
.SH "OPTIONS"
.HP
\fB\-A\fR, \fB\-\-show\-all\fR
.IP
Show non-printable characters.
.HP
\fB\-\-nonprintable\-notation\fR <notation>
.IP
Specify how to display non-printable characters.

Possible values:
.RS
.IP "caret"
Use character sequences like ^G ...
.IP "unicode"
Use special Unicode code points ...
.RE
.HP
\fB\-l\fR, \fB\-\-language\fR <language>
.IP
Set the language.
"#;

    #[test]
    fn hp_strategy_extracts_flags_and_skips_rs_example_values() {
        // bat uses .HP for flag tags and nests example values in .RS/.RE; the inner
        // .IP "caret"/"unicode" tags are not flags.
        let r = parse_manpage_string(HP_MANPAGE);
        let names: Vec<String> = r
            .entries
            .iter()
            .map(|e| match &e.switch {
                OwnedSwitch::Long(l) | OwnedSwitch::Both(_, l) => l.clone(),
                OwnedSwitch::Short(c) => c.to_string(),
            })
            .collect();
        assert_eq!(
            names,
            vec!["show-all", "nonprintable-notation", "language"],
            "expected 3 flags, got {names:?}"
        );
        assert!(
            !r.entries.iter().any(|e| matches!(
                &e.switch,
                OwnedSwitch::Long(l) if l == "caret" || l == "unicode"
            )),
            "inner .RS .IP example values must not be picked up as flags: {:?}",
            r.entries
        );
        assert!(matches!(
            r.entries[0].switch,
            OwnedSwitch::Both('A', ref l) if l == "show-all"
        ));
        assert!(matches!(
            r.entries[2].switch,
            OwnedSwitch::Both('l', ref l) if l == "language"
        ));
    }

    const TEXT_RS_NESTED_MANPAGE: &str = r#".TH TOOL "1"
.SH NAME
tool \- demo
.SH "OPTIONS"
.SS INPUT
\fB\-x\fR, \fB\-\-foo\fR
.RS 4
First flag desc. Possible values:
.RS
some value
.RE
After the inner block.
.RE
.sp
\fB\-y\fR, \fB\-\-bar\fR
.RS 4
Second flag desc.
.RE
"#;

    #[test]
    fn text_rs_strategy_handles_nested_rs_in_description() {
        // a flag's `.RS` body nesting another `.RS/.RE` must not end early at the
        // inner `.RE`, else the next flag's tag is misread as top-level text or the
        // first desc is truncated.
        let r = parse_manpage_string(TEXT_RS_NESTED_MANPAGE);
        assert_eq!(
            r.entries.len(),
            2,
            "expected exactly 2 flags, got {}",
            r.entries.len()
        );
        assert!(matches!(
            r.entries[0].switch,
            OwnedSwitch::Both('x', ref l) if l == "foo"
        ));
        assert!(
            r.entries[0].desc.contains("First flag desc"),
            "outer .RS body should be captured, got: {:?}",
            r.entries[0].desc
        );
        assert!(
            r.entries[0].desc.contains("After the inner block"),
            "text after the nested .RE must still belong to the outer block, got: {:?}",
            r.entries[0].desc
        );
        assert!(
            !r.entries[0].desc.contains("some value"),
            "inner .RS sub-value text should be skipped, got: {:?}",
            r.entries[0].desc
        );
        assert!(matches!(
            r.entries[1].switch,
            OwnedSwitch::Both('y', ref l) if l == "bar"
        ));
        assert!(r.entries[1].desc.contains("Second flag desc"));
    }

    const TEXT_RS_MANPAGE: &str = r#".TH RG "1"
.SH NAME
rg \- demo
.SH "OPTIONS"
.SS INPUT OPTIONS
\fB\-e\fR \fIPATTERN\fR, \fB\-\-regexp\fR=\fIPATTERN\fR
.RS 4
A pattern to search for. This option can be provided multiple times.
.RE
.sp
\fB\-f\fR \fIPATTERNFILE\fR, \fB\-\-file\fR=\fIPATTERNFILE\fR
.RS 4
Search for patterns from the given file.
.RE
.sp
\fB\-x\fR, \fB\-\-line\-regexp\fR
.RS 4
Only show matches surrounded by line boundaries.
.RE
"#;

    #[test]
    fn text_rs_strategy_extracts_ripgrep_style_flags() {
        // rg's layout: bare Text tag immediately followed by `.RS/.RE`, separated
        // by `.sp`, no `.PP` to anchor on.
        let r = parse_manpage_string(TEXT_RS_MANPAGE);
        assert_eq!(
            r.entries.len(),
            3,
            "expected 3 entries, got {}",
            r.entries.len()
        );
        // PARAM between short and comma
        assert!(matches!(
            r.entries[0].switch,
            OwnedSwitch::Both('e', ref l) if l == "regexp"
        ));
        assert!(matches!(
            r.entries[0].param,
            Some(OwnedParam::Mandatory(ref p)) if p == "PATTERN"
        ));
        assert!(r.entries[0].desc.starts_with("A pattern to search for"));
        assert!(matches!(
            r.entries[1].switch,
            OwnedSwitch::Both('f', ref l) if l == "file"
        ));
        // plain comma form, no PARAM
        assert!(matches!(
            r.entries[2].switch,
            OwnedSwitch::Both('x', ref l) if l == "line-regexp"
        ));
    }

    const TP_CLAP_DUAL_PARAGRAPH: &str = r#".TH JJ "1"
.SH NAME
jj \- demo
.SH OPTIONS
.TP
\fB\-\-at\-operation\fR <OP>
Operation to load the repo at

Operation to load the repo at. By default, Jujutsu loads the repo at the most recent operation, and lots of additional sentences that go on for paragraphs.
.TP
\fB\-h\fR, \fB\-\-help\fR
Print help
"#;

    #[test]
    fn tp_strategy_stops_description_at_blank_line() {
        // clap emits "summary\n\nexpanded body"; keep just the summary. leading
        // blanks (tag to first body line) skip; blanks only terminate once text is
        // collected.
        let r = parse_manpage_string(TP_CLAP_DUAL_PARAGRAPH);
        let at_op = r
            .entries
            .iter()
            .find(|e| matches!(&e.switch, OwnedSwitch::Long(l) if l == "at-operation"))
            .expect("--at-operation entry");
        assert_eq!(
            at_op.desc, "Operation to load the repo at",
            "expected only the summary line, got: {:?}",
            at_op.desc
        );
        // the second .TP block still parses (next entry not swallowed).
        assert!(r.entries.iter().any(|e| matches!(
            &e.switch,
            OwnedSwitch::Both('h', l) if l == "help"
        )));
    }

    #[test]
    fn tp_strategy_extracts_flags() {
        let r = parse_manpage_string(TP_MANPAGE);
        assert_eq!(
            r.entries.len(),
            3,
            "expected 3 entries, got {:?}",
            r.entries
        );
        assert_eq!(r.description, "a synthetic test command");
        assert!(matches!(
            r.entries[0].switch,
            OwnedSwitch::Both('v', ref l) if l == "verbose"
        ));
        assert!(matches!(
            r.entries[2].switch,
            OwnedSwitch::Both('h', ref l) if l == "help"
        ));
        assert!(r.entries[0].desc.contains("verbosity"));
    }

    // jj/clap group pages enumerate children as `.SH SUBCOMMANDS` xref entries
    // (`jj\-bookmark\-advance(1)` + desc), not an inline COMMANDS layout.
    const JJ_XREF_MANPAGE: &str = r#".TH "JJ-BOOKMARK" "1"
.SH NAME
jj\-bookmark \- Manage bookmarks
.SH SYNOPSIS
\fBjj bookmark\fR [\fB\-h\fR|\fB\-\-help\fR] <\fIsubcommands\fR>
.SH SUBCOMMANDS
.TP
jj\-bookmark\-create(1)
Create a new bookmark
.TP
jj\-bookmark\-set\-url(1)
Update a bookmark's url
.TP
jj\-bookmark\-untrack(1)
Stop tracking given remote bookmarks
"#;

    #[test]
    fn subcommand_xrefs_populate_subcommands() {
        let r = parse_manpage_string(JJ_XREF_MANPAGE);
        let names: Vec<&str> = r.subcommands.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["create", "set-url", "untrack"], "got {names:?}");
        // shared "jj-bookmark-" prefix stripped, multi-word child intact.
        let set_url = r
            .subcommands
            .iter()
            .find(|s| s.name == "set-url")
            .expect("set-url child");
        assert_eq!(set_url.desc, "Update a bookmark's url");
    }

    #[test]
    fn mixed_option_subsections_keep_local_strategy_winners() {
        let groff = r#".TH MIXED "1"
.SH NAME
mixed \- demo
.SH OPTIONS
.SS GENERAL OPTIONS
.TP
\fB\-a\fR, \fB\-\-all\fR
Show all entries.
.SS SEARCH OPTIONS
\fB\-e\fR \fIPATTERN\fR, \fB\-\-regexp\fR=\fIPATTERN\fR
.RS 4
Search for a pattern.
.RE
.sp
\fB\-f\fR \fIFILE\fR, \fB\-\-file\fR=\fIFILE\fR
.RS 4
Read patterns from a file.
.RE
"#;
        let r = parse_manpage_string(groff);
        assert_eq!(r.entries.len(), 3, "entries: {:?}", r.entries);
        assert!(r.entries.iter().any(|e| matches!(
            &e.switch,
            OwnedSwitch::Both('a', l) if l == "all"
        )));
        assert!(r.entries.iter().any(|e| matches!(
            &e.switch,
            OwnedSwitch::Both('e', l) if l == "regexp"
        )));
        assert!(r.entries.iter().any(|e| matches!(
            &e.switch,
            OwnedSwitch::Both('f', l) if l == "file"
        )));
    }

    #[test]
    fn description_only_alias_merge_rejects_generic_descriptions() {
        let groff = r#".TH ALIASES "1"
.SH NAME
aliases \- demo
.SH OPTIONS
.TP
\fB\-a\fR
Enable output
.TP
\fB\-\-all\fR
Enable output
"#;
        let r = parse_manpage_string(groff);
        assert_eq!(r.entries.len(), 2, "entries: {:?}", r.entries);
        assert!(
            !r.entries
                .iter()
                .any(|e| matches!(&e.switch, OwnedSwitch::Both('a', l) if l == "all")),
            "generic identical descriptions should not synthesize aliases: {:?}",
            r.entries
        );
        assert!(
            r.entries
                .iter()
                .any(|e| matches!(e.switch, OwnedSwitch::Short('a')))
        );
        assert!(
            r.entries
                .iter()
                .any(|e| matches!(&e.switch, OwnedSwitch::Long(l) if l == "all"))
        );
    }

    #[test]
    fn clap_subcommand_sections_keep_usage_positionals() {
        let groff = r#".TH APP "1"
.SH NAME
app \- demo
.SH SYNOPSIS
app [OPTIONS] <COMMAND>
.SH SUBCOMMAND
Clone a repository.
Usage: clone [OPTIONS] <repository> [directory]
.TP
\fB\-\-depth\fR \fIDEPTH\fR
Limit history depth.
"#;
        let (_parent, subs) = parse_manpage_with_subs(groff);
        assert_eq!(subs.len(), 1, "subs: {:?}", subs);
        let (name, result) = &subs[0];
        assert_eq!(name, "clone");
        assert_eq!(
            result
                .positionals
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["repository", "directory"],
            "positionals: {:?}",
            result.positionals
        );
        assert!(result.entries.iter().any(|e| matches!(
            &e.switch,
            OwnedSwitch::Long(l) if l == "depth"
        )));
    }

    #[test]
    fn mdoc_format_detected() {
        let src = ".Sh NAME\n.Nm test\n.Nd a test\n.Sh DESCRIPTION\nstuff\n";
        let lines: Vec<GroffLine> = src.split('\n').map(classify_line).collect();
        assert!(mdoc::is_mdoc(&lines));
    }

    #[test]
    fn groff_escapes_stripped() {
        let stripped = groff::strip_groff_escapes("\\fB\\-v\\fR \\fIfile\\fR");
        assert_eq!(stripped.trim(), "-v file");
    }

    fn entry(switch: OwnedSwitch, desc: &str) -> ManpageEntry {
        ManpageEntry {
            switch,
            param: None,
            desc: desc.to_string(),
        }
    }

    #[test]
    fn merges_non_adjacent_short_long_with_identical_desc() {
        let entries = vec![
            entry(OwnedSwitch::Short('h'), "show help"),
            entry(OwnedSwitch::Long("verbose".to_string()), "be verbose"),
            entry(OwnedSwitch::Long("help".to_string()), "show help"),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(merged.len(), 2, "expected the short to fold into the long");
        assert!(matches!(
            merged[0].switch,
            OwnedSwitch::Long(ref l) if l == "verbose"
        ));
        assert!(matches!(
            merged[1].switch,
            OwnedSwitch::Both('h', ref l) if l == "help"
        ));
    }

    #[test]
    fn merge_skips_empty_or_mismatched_descriptions() {
        let entries = vec![
            entry(OwnedSwitch::Short('q'), ""),
            entry(OwnedSwitch::Long("quiet".to_string()), ""),
            entry(OwnedSwitch::Short('n'), "number of results"),
            entry(OwnedSwitch::Long("no-color".to_string()), "disable colors"),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(merged.len(), 4, "no pair should merge");
        assert!(matches!(merged[0].switch, OwnedSwitch::Short('q')));
        assert!(matches!(merged[1].switch, OwnedSwitch::Long(ref l) if l == "quiet"));
        assert!(matches!(merged[2].switch, OwnedSwitch::Short('n')));
        assert!(matches!(merged[3].switch, OwnedSwitch::Long(ref l) if l == "no-color"));
    }

    #[test]
    fn merge_rejects_ambiguous_repeated_descriptions() {
        // two longs share a description with one short, so description equality
        // alone is not enough to synthesize either alias.
        let entries = vec![
            entry(OwnedSwitch::Short('h'), "show help"),
            entry(OwnedSwitch::Long("help".to_string()), "show help"),
            entry(OwnedSwitch::Long("usage".to_string()), "show help"),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(merged.len(), 3);
        assert!(matches!(merged[0].switch, OwnedSwitch::Short('h')));
        assert!(matches!(
            merged[1].switch,
            OwnedSwitch::Long(ref l) if l == "help"
        ));
        assert!(matches!(
            merged[2].switch,
            OwnedSwitch::Long(ref l) if l == "usage"
        ));
    }

    #[test]
    fn merge_drops_redundant_pair_when_both_already_present() {
        let entries = vec![
            ManpageEntry {
                switch: OwnedSwitch::Both('h', "help".to_string()),
                param: None,
                desc: "show help".to_string(),
            },
            entry(OwnedSwitch::Short('h'), "show help"),
            entry(OwnedSwitch::Long("help".to_string()), "show help"),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(
            merged.len(),
            1,
            "standalone short+long should drop when a Both with matching (c, l) already exists"
        );
        assert!(matches!(
            merged[0].switch,
            OwnedSwitch::Both('h', ref l) if l == "help"
        ));
    }

    #[test]
    fn merge_still_pairs_when_existing_both_has_different_long() {
        // a Both('v', "verbose") is in scope, but the standalone pair is
        // ('v', "version"), a different long, so it should merge into
        // Both('v', "version") rather than drop as redundant.
        let entries = vec![
            ManpageEntry {
                switch: OwnedSwitch::Both('v', "verbose".to_string()),
                param: None,
                desc: "be verbose".to_string(),
            },
            entry(OwnedSwitch::Short('v'), "print version"),
            entry(OwnedSwitch::Long("version".to_string()), "print version"),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(merged.len(), 2);
        assert!(matches!(
            merged[0].switch,
            OwnedSwitch::Both('v', ref l) if l == "verbose"
        ));
        assert!(matches!(
            merged[1].switch,
            OwnedSwitch::Both('v', ref l) if l == "version"
        ));
    }

    #[test]
    fn merge_carries_short_param_when_long_has_none() {
        let entries = vec![
            ManpageEntry {
                switch: OwnedSwitch::Short('o'),
                param: Some(OwnedParam::Mandatory("FILE".to_string())),
                desc: "write command output to file".to_string(),
            },
            entry(
                OwnedSwitch::Long("output".to_string()),
                "write command output to file",
            ),
        ];
        let merged = merge_short_long_pairs(entries);
        assert_eq!(merged.len(), 1);
        match &merged[0].param {
            Some(OwnedParam::Mandatory(p)) => assert_eq!(p, "FILE"),
            other => panic!("expected param carried over, got {other:?}"),
        }
    }
}
